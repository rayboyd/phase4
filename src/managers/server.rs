//! [`Server`] binds a TCP listener on the configured address and spawns a
//! dedicated OS thread running a single-threaded Tokio runtime. Within that
//! runtime, accepted TCP connections are upgraded to WebSocket sessions via
//! [`tokio_tungstenite`] and each client is handled in its own Tokio task.
//!
//! Each task subscribes to a [`tokio::sync::watch`] channel carrying
//! pre-serialised JSON as a [`Utf8Bytes`]. Serialisation is performed once per
//! frame by the mapper thread, so the server tasks are pure I/O forwarders.
//!
//! Connected clients receive the current payload immediately after handshake,
//! then subsequent updates as the mapper publishes them.  Connected clients
//! receive an RFC 6455 close frame on graceful shutdown.

use crate::app::AppState;
use anyhow::{Context, Result};
use futures_util::SinkExt;
use std::net::SocketAddr;
use std::sync::{atomic::Ordering, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::{watch, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::{
    handshake::server::{Callback, ErrorResponse, Request, Response},
    Message, Utf8Bytes,
};

/// How long the TCP listener blocks before yielding to check the `keep_running` flag.
const ACCEPT_TIMEOUT_MS: u64 = 500;

/// How long a newly accepted TCP client has to complete the WebSocket handshake.
const HANDSHAKE_TIMEOUT_MS: u64 = 1_000;

/// How long to wait for in-flight client tasks to flush a close frame
/// before the server thread exits.
const CLIENT_SHUTDOWN_TIMEOUT_MS: u64 = 500;

fn reject_browser_origin(origin: &str) -> ErrorResponse {
    Response::builder()
        .status(403)
        .body(Some(format!(
            "Browser-origin WebSocket clients are not supported, got {origin}"
        )))
        .expect("failed to build origin rejection response")
}

struct OriginPolicyCallback {
    addr: SocketAddr,
    reject_browser_origin: bool,
}

impl Callback for OriginPolicyCallback {
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

pub struct Server {
    address: SocketAddr,
    no_browser_origin: bool,
    max_clients: usize,
}

impl Server {
    #[must_use]
    pub fn new(address: SocketAddr, no_browser_origin: bool, max_clients: usize) -> Self {
        Self {
            address,
            no_browser_origin,
            max_clients,
        }
    }

    /// Spawns the WebSocket broadcast server on a dedicated OS thread.
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
        watch_rx: watch::Receiver<Utf8Bytes>,
        state: Arc<AppState>,
    ) -> Result<JoinHandle<()>> {
        // Bind eagerly so a port-in-use error is returned to the caller as a
        // Result, rather than panicking inside the spawned thread.
        let std_listener = std::net::TcpListener::bind(self.address)
            .with_context(|| format!("Failed to bind WebSocket server to {}", self.address))?;
        std_listener
            .set_nonblocking(true)
            .context("Failed to set listener to non-blocking")?;

        let addr = self.address;
        let no_browser_origin = self.no_browser_origin;
        let max_clients = self.max_clients;
        let handle = thread::Builder::new()
            .name("websocket-server".into())
            .spawn(move || {
                // Build the async runtime inside this dedicated OS thread.
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime")
                    .block_on(Self::run(
                        std_listener,
                        addr,
                        watch_rx,
                        state,
                        no_browser_origin,
                        max_clients,
                    ));
            })
            .expect("failed to spawn server thread");

        Ok(handle)
    }

    async fn run(
        std_listener: std::net::TcpListener,
        addr: SocketAddr,
        watch_rx: watch::Receiver<Utf8Bytes>,
        state: Arc<AppState>,
        no_browser_origin: bool,
        max_clients: usize,
    ) {
        // from_std is infallible when called from within a Tokio runtime context,
        // which is always true here since we're inside block_on.
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .expect("failed to convert std listener to tokio");
        log::info!("WebSocket server listening on ws://{addr}");

        let shutdown_notify = Arc::new(Notify::new());
        let client_slots = Arc::new(Semaphore::new(max_clients));
        let mut join_set: JoinSet<()> = JoinSet::new();

        loop {
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

                    log::info!("WebSocket client connected from {client_addr}");
                    join_set.spawn(Self::handle_client(
                        stream,
                        client_addr,
                        watch_rx.clone(),
                        shutdown_notify.clone(),
                        client_slot,
                        no_browser_origin,
                    ));
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
            tokio_tungstenite::accept_hdr_async(
                stream,
                OriginPolicyCallback {
                    addr,
                    reject_browser_origin: no_browser_origin,
                },
            ),
        )
        .await;

        let Ok(handshake_result) = handshake else {
            log::warn!("WebSocket handshake timed out: {addr}");
            return;
        };

        let Ok(mut ws_stream) = handshake_result else {
            log::info!("WebSocket handshake failed: {addr}");
            return;
        };

        // Send the current snapshot immediately so new clients do not wait for
        // the next mapper publish before rendering.
        let initial_json: Utf8Bytes = watch_rx.borrow_and_update().clone();
        if ws_stream.send(Message::Text(initial_json)).await.is_err() {
            log::info!("WebSocket client disconnected: {addr}");
            return;
        }

        loop {
            tokio::select! {
                biased;
                () = shutdown.notified() => break,
                changed = watch_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }

            // The mapper has already serialised the payload to JSON. Clone the
            // shared text buffer and forward it directly.
            let json: Utf8Bytes = watch_rx.borrow_and_update().clone();
            let msg = Message::Text(json);

            if ws_stream.send(msg).await.is_err() {
                log::info!("WebSocket client disconnected: {addr}");
                return;
            }
        }

        // RFC 6455 close frame on graceful shutdown or upstream channel closure.
        ws_stream.close(None).await.ok();
    }
}
