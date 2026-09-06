//! [`Server`] binds a TCP listener on the configured address and spawns a
//! dedicated OS thread running a single-threaded Tokio runtime. Within that
//! runtime, accepted TCP connections are upgraded to WebSocket sessions via
//! [`tokio_tungstenite`] and each client is handled in its own Tokio task.
//!
//! A dedicated serialiser task subscribes to a typed
//! [`tokio::sync::watch`] channel of [`crate::dsp::DisplayPayload`],
//! serialises consumed snapshots into [`Utf8Bytes`], and publishes to a private
//! watch channel that all client tasks subscribe to.
//!
//! Connected clients receive the current payload immediately after handshake,
//! then the latest available updates. Intermediate snapshots can be skipped.
//! Graceful shutdown attempts to send an RFC 6455 close frame within a bounded
//! task shutdown period. Phase4 services Ping,
//! Pong, and Close control frames but rejects inbound Text and Binary messages,
//! preserving the outbound-only application protocol.
//! Writes and flushes have a one-second deadline. A stalled client is dropped
//! and its connection slot released when that deadline expires.
//!
//! When `no_browser_origin` is set, the server rejects handshakes that carry
//! an `Origin` header. This blocks browsers (which the Fetch spec requires to
//! send Origin) but does not block native clients that omit it. It is not an
//! authentication mechanism.

use crate::app::AppState;
use crate::dsp::DisplayPayload;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{atomic::Ordering, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::{
    self,
    handshake::server::{Callback, ErrorResponse, Request, Response},
    protocol::{frame::coding::CloseCode, CloseFrame, WebSocketConfig},
    Message, Utf8Bytes,
};

/// How long the TCP listener blocks before yielding to check the `keep_running` flag.
/// Matches `Controller::POLL_RATE_MS`, the app's existing threshold for "responsive
/// enough to feel instant, cheap enough to poll continuously".
const ACCEPT_TIMEOUT_MS: u64 = 100;

/// How long a newly accepted TCP client has to complete the WebSocket handshake.
const HANDSHAKE_TIMEOUT_MS: u64 = 1_000;

/// Maximum time for one client write or flush before dropping the connection.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// How long to wait for in-flight client tasks to flush a close frame
/// before the server thread exits.
const CLIENT_SHUTDOWN_TIMEOUT_MS: u64 = 500;

/// RFC 6455 limits control-frame payloads to 125 bytes. Phase4 accepts no
/// application messages, so this also bounds client-controlled data before it
/// is rejected without restricting valid Ping, Pong, or Close frames.
const MAX_INBOUND_FRAME_BYTES: usize = 125;

/// Close reason returned when a client violates the outbound-only application
/// protocol by sending a Text or Binary message.
const INBOUND_APPLICATION_MESSAGE_CLOSE_REASON: &str =
    "Phase4 does not accept application messages";

/// Number of `serialised_rx` handles the server retains for itself. The
/// accept loop holds exactly one receiver, which it clones for each new
/// client, so a receiver count at or below this value means no clients
/// are connected.
const RETAINED_SERIALISED_RECEIVERS: usize = 1;

/// A single request is sufficient because the accept loop waits for each
/// snapshot refresh to complete before accepting another client.
const SNAPSHOT_REFRESH_QUEUE_CAPACITY: usize = 1;

type SnapshotRefreshRequest = oneshot::Sender<()>;

/// Returns whether the serialised watch channel has any client receivers
/// beyond the handle the server retains for cloning.
fn has_connected_clients(receiver_count: usize) -> bool {
    receiver_count > RETAINED_SERIALISED_RECEIVERS
}

fn reap_completed_tasks(join_set: &mut JoinSet<()>) {
    while let Some(result) = join_set.try_join_next() {
        if let Err(error) = result {
            log::error!("WebSocket server task failed: {error}");
        }
    }
}

async fn refresh_serialised_snapshot(requests: &mpsc::Sender<SnapshotRefreshRequest>) {
    let (completion_tx, completion_rx) = oneshot::channel();
    if requests.send(completion_tx).await.is_ok() {
        let _ = completion_rx.await;
    }
}

/// A failed or cancelled write leaves the connection unusable. Callers must
/// drop the client on false rather than retry a partially written frame.
async fn complete_client_write(
    write: impl Future<Output = tungstenite::Result<()>>,
    address: SocketAddr,
) -> bool {
    match tokio::time::timeout(CLIENT_WRITE_TIMEOUT, write).await {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            log::debug!("WebSocket client disconnected: {address}, {error}");
            false
        }
        Err(_) => {
            log::warn!("WebSocket client write timed out, disconnecting: {address}");
            false
        }
    }
}

#[cfg(test)]
fn advance_serialiser_progress(observer: Option<&watch::Sender<usize>>) {
    if let Some(observer) = observer {
        observer.send_modify(|progress| *progress += 1);
    }
}

fn reject_browser_origin(origin: &str) -> ErrorResponse {
    Response::builder()
        .status(403)
        .body(Some(format!(
            "Browser-origin WebSocket clients are not supported, got {origin}"
        )))
        .expect("failed to build origin rejection response")
}

/// Handshake callback that enforces the server's browser-origin policy.
struct OriginPolicyCallback {
    /// Address of the connecting client, used only for logging.
    addr: SocketAddr,

    /// Whether to reject handshakes carrying an `Origin` header.
    reject_browser_origin: bool,
}

impl Callback for OriginPolicyCallback {
    // The Err type (ErrorResponse) and its size are fixed by tungstenite's
    // Callback trait signature; boxing it is not an option here.
    #[allow(clippy::result_large_err)]
    fn on_request(
        self,
        request: &Request,
        response: Response,
    ) -> std::result::Result<Response, ErrorResponse> {
        if !self.reject_browser_origin {
            return Ok(response);
        }

        let Some(origin) = request.headers().get("Origin") else {
            return Ok(response);
        };

        let origin = origin.to_str().unwrap_or("<invalid origin header>");
        log::warn!(
            "WebSocket client rejected by origin policy: {}, origin={origin}",
            self.addr
        );
        Err(reject_browser_origin(origin))
    }
}

/// Serialises `payload` to JSON, logging failures at most once per run of
/// consecutive failures rather than on every call. `already_logged` tracks
/// whether the current failure streak has already produced a log line, and
/// is reset to `false` the next time serialisation succeeds.
///
/// Non-finite values (`NaN`, `Infinity`) are checked explicitly before
/// calling `serde_json`, rather than relied upon to surface as an `Err`.
/// `serde_json` (as pinned in this repo) silently encodes non-finite floats
/// as JSON `null` instead of returning an error, so without this check a
/// `NaN` bin value would broadcast to every client unnoticed rather than
/// being caught here.
///
/// Returns `None` when the payload is rejected. The caller is expected to
/// retain the previously broadcast snapshot without publishing an update
/// for the rejected frame. Existing clients receive no new frame until
/// another valid snapshot is published.
fn serialise_display_payload(
    payload: &DisplayPayload,
    already_logged: &mut bool,
) -> Option<Utf8Bytes> {
    let has_non_finite = payload.channels.iter().any(|channel| {
        !channel.peak.is_finite() || channel.bins.iter().any(|bin| !bin.is_finite())
    });

    if has_non_finite {
        if !*already_logged {
            log::error!(
                "DisplayPayload contains a non-finite value (NaN or Infinity), refusing \
                 to broadcast this frame. Further occurrences until the next valid frame \
                 will not be logged individually. Clients continue to receive the last \
                 valid frame."
            );
            *already_logged = true;
        }
        return None;
    }

    *already_logged = false;

    match serde_json::to_string(payload) {
        Ok(json) => Some(Utf8Bytes::from(json)),
        Err(error) => {
            // Retained as a defensive fallback for any other serialisation
            // failure mode. Non-finite floats are caught above and never
            // reach this call.
            log::error!("Failed to serialise display payload: {error}");
            None
        }
    }
}

/// Fallback seed for the initial WebSocket snapshot. Kept as a named constant
/// so the wire contract shape (a `channels` array, never a bare `{}`) is
/// documented in one place. The test
/// `empty_display_payload_serialises_to_channels_array` pins this constant
/// to the wire format that `DisplayPayload::default()` produces.
const EMPTY_DISPLAY_PAYLOAD_JSON: &str = r#"{"channels":[]}"#;

/// Produces the seed value for the serialised watch channel from the initial
/// display payload, routing it through the same validation path used for
/// every subsequent frame rather than serialising it directly.
///
/// Shares `already_logged` with the frame loop's serialisation task, so a
/// rejected initial payload counts towards the same once-per-streak logging
/// streak as subsequent frames rather than always logging at startup.
///
/// Falls back to [`EMPTY_DISPLAY_PAYLOAD_JSON`] when the initial payload is
/// rejected. The test `empty_display_payload_serialises_to_channels_array`
/// pins that constant to the wire format that `DisplayPayload::default()`
/// produces, ensuring the fallback remains shaped like every other frame (a
/// `channels` array, empty rather than absent).
fn initial_serialised_snapshot(payload: &DisplayPayload, already_logged: &mut bool) -> Utf8Bytes {
    if let Some(json) = serialise_display_payload(payload, already_logged) {
        return json;
    }

    log::error!("Falling back to an empty display payload for the initial WebSocket snapshot");

    Utf8Bytes::from(EMPTY_DISPLAY_PAYLOAD_JSON.to_owned())
}

/// Owns the WebSocket broadcast server configuration.
pub struct Server {
    /// Address the TCP listener binds to.
    address: SocketAddr,

    /// Whether to reject handshakes carrying an `Origin` header.
    no_browser_origin: bool,

    /// Maximum number of concurrently connected clients.
    max_clients: usize,

    /// Test-only visibility into retained tasks avoids platform-dependent RSS assertions.
    #[cfg(test)]
    task_count_observer: Option<watch::Sender<usize>>,

    /// Test-only visibility into serialiser progress avoids timing-based frame assertions.
    #[cfg(test)]
    serialiser_progress_observer: Option<watch::Sender<usize>>,
}

impl Server {
    #[must_use]
    pub fn new(address: SocketAddr, no_browser_origin: bool, max_clients: usize) -> Self {
        Self {
            address,
            no_browser_origin,
            max_clients,
            #[cfg(test)]
            task_count_observer: None,
            #[cfg(test)]
            serialiser_progress_observer: None,
        }
    }

    #[cfg(test)]
    fn with_task_count_observer(mut self, observer: watch::Sender<usize>) -> Self {
        self.task_count_observer = Some(observer);
        self
    }

    #[cfg(test)]
    fn with_serialiser_progress_observer(mut self, observer: watch::Sender<usize>) -> Self {
        self.serialiser_progress_observer = Some(observer);
        self
    }

    /// Spawns the WebSocket broadcast server on a dedicated OS thread.
    ///
    /// Returns the address the listener actually bound to alongside the
    /// thread handle. This is the configured address unless it used port 0,
    /// in which case it's the OS-assigned port, obtained from the listener
    /// itself rather than echoing back the configured value.
    ///
    /// # Errors
    ///
    /// Returns an error if the TCP listener cannot bind to the configured
    /// address (e.g. port already in use or insufficient permissions).
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned or if the single-threaded
    /// Tokio runtime cannot be built.
    pub fn spawn(
        self,
        display_rx: watch::Receiver<DisplayPayload>,
        state: Arc<AppState>,
    ) -> Result<(SocketAddr, JoinHandle<()>)> {
        // Bind eagerly so a port-in-use error is returned to the caller as a
        // Result, rather than panicking inside the spawned thread.
        let std_listener = std::net::TcpListener::bind(self.address)
            .with_context(|| format!("Failed to bind WebSocket server to {}", self.address))?;
        std_listener
            .set_nonblocking(true)
            .context("Failed to set listener to non-blocking")?;
        let bound_addr = std_listener
            .local_addr()
            .context("Failed to read the bound WebSocket listener address")?;

        let no_browser_origin = self.no_browser_origin;
        let max_clients = self.max_clients;
        #[cfg(test)]
        let task_count_observer = self.task_count_observer;
        #[cfg(test)]
        let serialiser_progress_observer = self.serialiser_progress_observer;
        let handle = super::spawn_async_worker(
            "websocket-server",
            Self::run(
                std_listener,
                display_rx,
                state,
                no_browser_origin,
                max_clients,
                #[cfg(test)]
                task_count_observer,
                #[cfg(test)]
                serialiser_progress_observer,
            ),
        );

        Ok((bound_addr, handle))
    }

    async fn run(
        std_listener: std::net::TcpListener,
        mut display_rx: watch::Receiver<DisplayPayload>,
        state: Arc<AppState>,
        no_browser_origin: bool,
        max_clients: usize,
        #[cfg(test)] task_count_observer: Option<watch::Sender<usize>>,
        #[cfg(test)] serialiser_progress_observer: Option<watch::Sender<usize>>,
    ) {
        // from_std is infallible when called from within a Tokio runtime context,
        // which is always true here since we're inside block_on.
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .expect("failed to convert std listener to tokio");

        let shutdown_notify = Arc::new(Notify::new());
        let client_slots = Arc::new(Semaphore::new(max_clients));
        let mut join_set: JoinSet<()> = JoinSet::new();

        // One task owns JSON serialisation and fan-out to connected clients,
        // skipping serialisation while no clients are connected.
        // The failure-logged flag is shared with the frame loop below, so a
        // rejected initial payload and a rejected first frame count as one
        // logging streak rather than two.
        let mut serialise_failure_logged = false;
        let initial_serialised =
            initial_serialised_snapshot(&display_rx.borrow(), &mut serialise_failure_logged);
        let (serialised_tx, serialised_rx) = watch::channel(initial_serialised);
        let (snapshot_refresh_tx, mut snapshot_refresh_rx) =
            mpsc::channel::<SnapshotRefreshRequest>(SNAPSHOT_REFRESH_QUEUE_CAPACITY);
        #[cfg(test)]
        advance_serialiser_progress(serialiser_progress_observer.as_ref());
        join_set.spawn(async move {
            loop {
                let refresh_completion = tokio::select! {
                    changed = display_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        None
                    }
                    request = snapshot_refresh_rx.recv() => {
                        let Some(completion) = request else {
                            break;
                        };
                        Some(completion)
                    }
                };

                if refresh_completion.is_none()
                    && !has_connected_clients(serialised_tx.receiver_count())
                {
                    // Mark the frame consumed and skip the encode, nobody is
                    // listening. A new connection requests a current snapshot
                    // before its client task starts.
                    let _ = display_rx.borrow_and_update();
                    #[cfg(test)]
                    advance_serialiser_progress(serialiser_progress_observer.as_ref());
                    continue;
                }

                // Avoid a per-frame DisplayPayload clone, the serialised output still needs
                // a fresh owned buffer per frame for fan-out to client tasks.
                if let Some(json) = serialise_display_payload(
                    &display_rx.borrow_and_update(),
                    &mut serialise_failure_logged,
                ) {
                    let snapshot_changed = refresh_completion.is_none() || {
                        let current_snapshot = serialised_tx.borrow();
                        current_snapshot.as_str() != json.as_str()
                    };
                    if snapshot_changed {
                        serialised_tx.send_replace(json);
                    }
                }

                if let Some(completion) = refresh_completion {
                    let _ = completion.send(());
                }
                #[cfg(test)]
                advance_serialiser_progress(serialiser_progress_observer.as_ref());
            }
        });

        loop {
            reap_completed_tasks(&mut join_set);

            if !state.keep_running.load(Ordering::Acquire) {
                break;
            }

            let Ok(accepted) =
                tokio::time::timeout(Duration::from_millis(ACCEPT_TIMEOUT_MS), listener.accept())
                    .await
            else {
                continue;
            };

            match accepted {
                Ok((stream, client_addr)) => {
                    let Ok(client_slot) = Arc::clone(&client_slots).try_acquire_owned() else {
                        log::warn!("WebSocket client rejected, at capacity: {client_addr}");
                        continue;
                    };

                    log::debug!("WebSocket client connected from {client_addr}");
                    refresh_serialised_snapshot(&snapshot_refresh_tx).await;
                    reap_completed_tasks(&mut join_set);
                    join_set.spawn(Self::handle_client(
                        stream,
                        client_addr,
                        serialised_rx.clone(),
                        shutdown_notify.clone(),
                        client_slot,
                        no_browser_origin,
                    ));

                    #[cfg(test)]
                    if let Some(observer) = &task_count_observer {
                        observer.send_replace(join_set.len());
                    }
                }
                Err(e) => log::error!("TCP Accept error: {e}"),
            }
        }

        // Wake every client task so it can flush a close frame and return.
        shutdown_notify.notify_waiters();

        // Bounded join. Tasks still running after the shutdown timeout budget are dropped when
        // join_set falls out of scope.
        let _ = tokio::time::timeout(Duration::from_millis(CLIENT_SHUTDOWN_TIMEOUT_MS), async {
            while join_set.join_next().await.is_some() {}
        })
        .await;
    }

    async fn handle_client(
        stream: tokio::net::TcpStream,
        addr: std::net::SocketAddr,
        mut watch_rx: watch::Receiver<Utf8Bytes>,
        shutdown: Arc<Notify>,
        _client_slot: OwnedSemaphorePermit,
        no_browser_origin: bool,
    ) {
        let handshake = tokio::time::timeout(
            Duration::from_millis(HANDSHAKE_TIMEOUT_MS),
            tokio_tungstenite::accept_hdr_async_with_config(
                stream,
                OriginPolicyCallback {
                    addr,
                    reject_browser_origin: no_browser_origin,
                },
                Some(
                    WebSocketConfig::default()
                        .max_message_size(Some(MAX_INBOUND_FRAME_BYTES))
                        .max_frame_size(Some(MAX_INBOUND_FRAME_BYTES)),
                ),
            ),
        )
        .await;

        let Ok(handshake_result) = handshake else {
            log::warn!("WebSocket handshake timed out: {addr}");
            return;
        };

        let Ok(mut ws_stream) = handshake_result else {
            log::debug!("WebSocket handshake failed: {addr}");
            return;
        };

        // Send the current snapshot immediately so new clients do not wait for
        // the next mapper publish before rendering.
        let initial_json: Utf8Bytes = watch_rx.borrow_and_update().clone();
        if !complete_client_write(ws_stream.send(Message::Text(initial_json)), addr).await {
            return;
        }

        loop {
            tokio::select! {
                biased;
                () = shutdown.notified() => break,
                incoming = ws_stream.next() => {
                    match incoming {
                        Some(Ok(Message::Ping(_))) => {
                            if !complete_client_write(ws_stream.flush(), addr).await {
                                return;
                            }
                        }
                        Some(Ok(Message::Pong(_) | Message::Frame(_))) => {}
                        Some(Ok(Message::Close(_))) => {
                            complete_client_write(ws_stream.flush(), addr).await;
                            return;
                        }
                        Some(Ok(Message::Text(_) | Message::Binary(_))) => {
                            log::warn!(
                                "WebSocket application message rejected from {addr}"
                            );
                            let close_frame = CloseFrame {
                                code: CloseCode::Policy,
                                reason: Utf8Bytes::from_static(
                                    INBOUND_APPLICATION_MESSAGE_CLOSE_REASON,
                                ),
                            };
                            complete_client_write(
                                ws_stream.send(Message::Close(Some(close_frame))), addr,
                            ).await;
                            return;
                        }
                        Some(Err(error)) => {
                            log::debug!("WebSocket client disconnected: {addr}, {error}");
                            return;
                        }
                        None => return,
                    }
                }
                changed = watch_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }

                    // The server serialiser owns JSON encoding. Clone its shared
                    // text buffer for this client without encoding again.
                    let json: Utf8Bytes = watch_rx.borrow_and_update().clone();
                    let msg = Message::Text(json);

                    if !complete_client_write(ws_stream.send(msg), addr).await {
                        return;
                    }
                }
            }
        }

        // RFC 6455 close frame on graceful shutdown or upstream channel closure.
        complete_client_write(ws_stream.close(None), addr).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    struct BackpressureClient {
        snapshots: watch::Sender<Utf8Bytes>,
        client: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        task: tokio::task::JoinHandle<()>,
        slots: Arc<Semaphore>,
    }

    const CLIENT_RECONNECT_ATTEMPTS: usize = 50;
    const CLIENT_RECONNECT_DELAY: Duration = Duration::from_millis(20);
    const TEST_TIMEOUT: Duration = Duration::from_secs(2);
    const BACKPRESSURE_SOCKET_BUFFER_BYTES: u32 = 1_024;
    const BACKPRESSURE_PAYLOAD_BYTES: usize = 256 * 1_024;

    async fn backpressure_client(initial_snapshot: Utf8Bytes) -> BackpressureClient {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have an address");
        let client_socket = tokio::net::TcpSocket::new_v4().expect("client socket should open");
        client_socket
            .set_recv_buffer_size(BACKPRESSURE_SOCKET_BUFFER_BYTES)
            .expect("client receive buffer should be configurable");
        let client_stream = client_socket
            .connect(address)
            .await
            .expect("TCP should connect");
        let (server_stream, client_address) = listener.accept().await.expect("TCP should accept");
        socket2::SockRef::from(&server_stream)
            .set_send_buffer_size(BACKPRESSURE_SOCKET_BUFFER_BYTES as usize)
            .expect("server send buffer should be configurable");
        let (snapshots, receiver) = watch::channel(initial_snapshot);
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots
            .clone()
            .try_acquire_owned()
            .expect("slot should be free");
        let task = tokio::spawn(Server::handle_client(
            server_stream,
            client_address,
            receiver,
            Arc::new(Notify::new()),
            permit,
            false,
        ));
        let (client, _) = tokio_tungstenite::client_async(format!("ws://{address}"), client_stream)
            .await
            .expect("WebSocket handshake should complete");

        BackpressureClient {
            snapshots,
            client,
            task,
            slots,
        }
    }

    #[tokio::test]
    async fn stalled_initial_write_releases_the_client_slot() {
        let connection =
            backpressure_client(Utf8Bytes::from("x".repeat(BACKPRESSURE_PAYLOAD_BYTES))).await;

        tokio::time::timeout(TEST_TIMEOUT, connection.task)
            .await
            .expect("stalled initial write should terminate")
            .expect("client task should not panic");
        assert_eq!(connection.slots.available_permits(), 1);
        drop(connection.client);
    }

    #[tokio::test]
    async fn stalled_update_releases_the_client_slot() {
        let mut connection =
            backpressure_client(Utf8Bytes::from_static(EMPTY_DISPLAY_PAYLOAD_JSON)).await;
        tokio::time::timeout(TEST_TIMEOUT, connection.client.next())
            .await
            .expect("initial snapshot should arrive")
            .expect("client should remain connected")
            .expect("initial snapshot should be valid");

        connection
            .snapshots
            .send_replace(Utf8Bytes::from("x".repeat(BACKPRESSURE_PAYLOAD_BYTES)));
        tokio::time::timeout(TEST_TIMEOUT, connection.task)
            .await
            .expect("stalled update should terminate")
            .expect("client task should not panic");
        assert_eq!(connection.slots.available_permits(), 1);
        drop(connection.client);
    }

    async fn next_observed_server_count(observer: &mut watch::Receiver<usize>) -> usize {
        tokio::time::timeout(TEST_TIMEOUT, observer.changed())
            .await
            .expect("timed out waiting for an observed server count to change")
            .expect("server dropped a count observer unexpectedly");
        *observer.borrow_and_update()
    }

    async fn join_server(handle: JoinHandle<()>) {
        let join_task = tokio::task::spawn_blocking(move || handle.join());
        tokio::time::timeout(TEST_TIMEOUT, join_task)
            .await
            .expect("server thread did not stop within the test timeout")
            .expect("blocking join task panicked")
            .expect("server thread panicked");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn server_does_not_retain_completed_client_tasks() {
        const EXPECTED_TRACKED_TASKS: usize = 2;

        let (display_tx, display_rx) = watch::channel(DisplayPayload::new(0));
        let state = Arc::new(AppState::new());
        let (task_count_tx, mut task_count_rx) = watch::channel(0);
        let server = Server::new("127.0.0.1:0".parse().expect("valid test address"), false, 1)
            .with_task_count_observer(task_count_tx);
        let (bound_addr, server_handle) = server
            .spawn(display_rx, Arc::clone(&state))
            .expect("server should bind to a loopback test address");
        let url = format!("ws://{bound_addr}");

        let (mut first_client, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("first client should connect");
        tokio::time::timeout(TEST_TIMEOUT, first_client.next())
            .await
            .expect("first client timed out waiting for its initial payload")
            .expect("first client disconnected before its initial payload")
            .expect("first client received an invalid WebSocket frame");
        assert_eq!(
            next_observed_server_count(&mut task_count_rx).await,
            EXPECTED_TRACKED_TASKS,
            "the server should initially retain the serialiser and first client tasks"
        );

        drop(first_client);

        let mut second_client = None;
        for _ in 0..CLIENT_RECONNECT_ATTEMPTS {
            display_tx.send_replace(DisplayPayload::new(0));
            tokio::time::sleep(CLIENT_RECONNECT_DELAY).await;

            if let Ok((client, _)) = tokio_tungstenite::connect_async(&url).await {
                second_client = Some(client);
                break;
            }
        }
        let second_client =
            second_client.expect("second client should connect after the first exits");
        let retained_task_count = next_observed_server_count(&mut task_count_rx).await;

        state.keep_running.store(false, Ordering::Release);
        drop(display_tx);
        join_server(server_handle).await;
        drop(second_client);

        assert_eq!(
            retained_task_count,
            EXPECTED_TRACKED_TASKS,
            "the server should retain only the serialiser and current client tasks, but a completed client task remains in the JoinSet"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn first_client_receives_latest_payload_after_frames_without_clients() {
        const INITIAL_PEAK: f32 = 0.25;
        const LATEST_PEAK: f32 = 0.75;

        let mut initial_payload = DisplayPayload::new(1);
        initial_payload.channels[0].peak = INITIAL_PEAK;
        let (display_tx, display_rx) = watch::channel(initial_payload);
        let state = Arc::new(AppState::new());
        let (serialiser_progress_tx, mut serialiser_progress_rx) = watch::channel(0);
        let server = Server::new("127.0.0.1:0".parse().expect("valid test address"), false, 1)
            .with_serialiser_progress_observer(serialiser_progress_tx);
        let (bound_addr, server_handle) = server
            .spawn(display_rx, Arc::clone(&state))
            .expect("server should bind to a loopback test address");

        assert_eq!(
            next_observed_server_count(&mut serialiser_progress_rx).await,
            1,
            "the serialiser should be ready before the test publishes a newer frame"
        );

        let mut latest_payload = DisplayPayload::new(1);
        latest_payload.channels[0].peak = LATEST_PEAK;
        display_tx.send_replace(latest_payload);
        assert_eq!(
            next_observed_server_count(&mut serialiser_progress_rx).await,
            2,
            "the serialiser should consume the newer frame while no clients are connected"
        );

        let url = format!("ws://{bound_addr}");
        let (mut client, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("first client should connect");
        let initial_message = tokio::time::timeout(TEST_TIMEOUT, client.next())
            .await
            .expect("first client timed out waiting for its initial payload")
            .expect("first client disconnected before its initial payload")
            .expect("first client received an invalid WebSocket frame");
        let initial_json = initial_message
            .into_text()
            .expect("first client should receive a text payload");
        let initial_value: serde_json::Value =
            serde_json::from_str(&initial_json).expect("first client should receive valid JSON");
        let received_peak = initial_value["channels"][0]["peak"]
            .as_f64()
            .expect("first client payload should contain a numeric peak");

        state.keep_running.store(false, Ordering::Release);
        drop(display_tx);
        join_server(server_handle).await;
        drop(client);

        assert!(
            (received_peak - f64::from(LATEST_PEAK)).abs() < f64::EPSILON,
            "the first client must receive the latest consumed frame rather than the startup snapshot"
        );
    }

    #[test]
    fn has_connected_clients_is_false_for_the_retained_receiver_alone() {
        assert!(!has_connected_clients(RETAINED_SERIALISED_RECEIVERS));
    }

    #[test]
    fn has_connected_clients_is_true_above_the_retained_count() {
        assert!(has_connected_clients(RETAINED_SERIALISED_RECEIVERS + 1));
    }

    #[test]
    fn serialise_display_payload_rejects_non_finite_values_once_per_streak() {
        testing_logger::setup();

        let mut payload = DisplayPayload::new(1);
        payload.channels[0].peak = f32::NAN;
        let mut already_logged = false;

        let first = serialise_display_payload(&payload, &mut already_logged);
        let second = serialise_display_payload(&payload, &mut already_logged);

        assert!(first.is_none(), "a non-finite payload should be rejected");
        assert!(second.is_none(), "a non-finite payload should be rejected");

        testing_logger::validate(|captured_logs| {
            assert_eq!(
                captured_logs.len(),
                1,
                "a second consecutive rejection should not log again"
            );
            assert_eq!(captured_logs[0].level, log::Level::Error);
        });
    }

    #[test]
    fn serialise_display_payload_logs_again_after_a_success() {
        testing_logger::setup();

        let mut failing = DisplayPayload::new(1);
        failing.channels[0].peak = f32::NAN;
        let healthy = DisplayPayload::new(1);
        let mut already_logged = false;

        serialise_display_payload(&failing, &mut already_logged);
        serialise_display_payload(&healthy, &mut already_logged);
        serialise_display_payload(&failing, &mut already_logged);

        testing_logger::validate(|captured_logs| {
            assert_eq!(
                captured_logs.len(),
                2,
                "a fresh rejection after a valid frame should log again"
            );
        });
    }

    /// An empty `DisplayPayload` (zero channels) must serialise to exactly
    /// `{"channels":[]}`. This is the fallback shape the wire contract uses
    /// when the initial snapshot cannot be validated, and must never regress
    /// to a bare `{}`.
    #[test]
    fn empty_display_payload_serialises_to_channels_array() {
        let json = serde_json::to_string(&DisplayPayload::default())
            .expect("infallible for an empty, finite payload");

        assert_eq!(json, r#"{"channels":[]}"#);
    }

    #[test]
    fn initial_serialised_snapshot_passes_through_a_valid_payload_unchanged() {
        let payload = DisplayPayload::new(1);
        let mut already_logged = false;

        let seeded = initial_serialised_snapshot(&payload, &mut already_logged);

        let expected = serde_json::to_string(&payload).expect("failed to serialise payload");
        assert_eq!(seeded.as_str(), expected);
        assert!(
            !already_logged,
            "a valid payload should not mark a logging streak"
        );
    }

    #[test]
    fn initial_serialised_snapshot_falls_back_to_empty_channels_on_non_finite_value() {
        testing_logger::setup();

        let mut payload = DisplayPayload::new(1);
        payload.channels[0].peak = f32::NAN;
        let mut already_logged = false;

        let seeded = initial_serialised_snapshot(&payload, &mut already_logged);

        assert_eq!(
            seeded.as_str(),
            r#"{"channels":[]}"#,
            "a rejected initial payload must fall back to an empty channels array, not {{}}"
        );
        assert!(
            already_logged,
            "a rejected initial payload should mark the logging streak"
        );
    }

    #[test]
    fn initial_serialised_snapshot_shares_the_logging_streak_with_the_frame_loop() {
        testing_logger::setup();

        let mut failing = DisplayPayload::new(1);
        failing.channels[0].peak = f32::NAN;
        let mut already_logged = false;

        // The initial snapshot and the first frame both reject the same
        // non-finite payload; sharing already_logged across both should
        // count as a single streak, not two.
        initial_serialised_snapshot(&failing, &mut already_logged);
        serialise_display_payload(&failing, &mut already_logged);

        testing_logger::validate(|captured_logs| {
            let rejection_logs = captured_logs
                .iter()
                .filter(|log| log.body.contains("non-finite value"))
                .count();
            assert_eq!(
                rejection_logs, 1,
                "the initial snapshot and the first frame share one logging streak"
            );
        });
    }
}
