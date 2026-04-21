use futures_util::StreamExt;
use phase4::app::AppState;
use phase4::dsp::DisplayPayload;
use phase4::managers::{server::MAX_CLIENTS, Server};
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{
    tungstenite::{self, client::IntoClientRequest, Message, Utf8Bytes},
    MaybeTlsStream, WebSocketStream,
};

fn free_local_address() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    address
}

async fn connect_client(
    address: SocketAddr,
) -> WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://{address}");

    for _ in 0..50 {
        if let Ok((stream, _response)) = tokio_tungstenite::connect_async(&url).await {
            return stream;
        }

        sleep(Duration::from_millis(10)).await;
    }

    panic!("failed to connect to review server at {address}");
}

async fn connect_raw_client(address: SocketAddr) -> tokio::net::TcpStream {
    tokio::net::TcpStream::connect(address)
        .await
        .unwrap_or_else(|e| panic!("failed to connect raw TCP client to {address}: {e}"))
}

/// Join the server thread with a hard deadline.
///
/// Wraps the blocking `JoinHandle::join` in `spawn_blocking` so the Tokio
/// runtime can drive a `timeout` alongside it. A hang in the server thread
/// surfaces as a fast, descriptive panic rather than a test-binary timeout.
async fn join_server_bounded(handle: std::thread::JoinHandle<()>) {
    const SHUTDOWN_BUDGET: Duration = Duration::from_secs(2);

    let join_task = tokio::task::spawn_blocking(move || handle.join());

    #[allow(clippy::match_wild_err_arm)]
    match tokio::time::timeout(SHUTDOWN_BUDGET, join_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(panic))) => std::panic::resume_unwind(panic),
        Ok(Err(join_err)) => panic!("spawn_blocking task failed: {join_err}"),
        Err(_) => panic!("server thread did not shut down within {SHUTDOWN_BUDGET:?}"),
    }
}

async fn expect_text_payload(
    client: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    expected_json: &str,
    label: &str,
) {
    let message = timeout(Duration::from_secs(1), client.next())
        .await
        .unwrap_or_else(|_| panic!("{label} timed out waiting for a text frame"))
        .expect("client should receive a text frame before the socket ends")
        .expect("client should not see a protocol error while waiting for a text frame");

    match message {
        Message::Text(json) => assert_eq!(json.as_str(), expected_json, "{label}"),
        other => panic!("{label}: expected text frame, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_receives_current_payload_immediately_on_connect() {
    let address = free_local_address();
    let mut payload = DisplayPayload::new(1);
    payload.channels[0].peak = 0.75;
    payload.channels[0].bins[0] = 0.25;
    payload.channels[0].bins[1] = 0.5;

    let initial_display =
        serde_json::to_string(&payload).expect("failed to serialise initial display payload");
    let (_display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display.clone()));
    let state = Arc::new(AppState::new());

    let handle = Server::new(address, false)
        .spawn(display_rx, state.clone())
        .unwrap();

    let mut client = connect_client(address).await;
    expect_text_payload(&mut client, &initial_display, "initial snapshot").await;

    state.keep_running.store(false, Ordering::Release);
    join_server_bounded(handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_sends_close_frame_on_shutdown() {
    let address = free_local_address();
    let initial_display = serde_json::to_string(&DisplayPayload::new(0))
        .expect("failed to serialise initial display payload");
    let (_display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display.clone()));
    let state = Arc::new(AppState::new());

    let handle = Server::new(address, false)
        .spawn(display_rx, state.clone())
        .unwrap();

    let mut client = connect_client(address).await;
    expect_text_payload(&mut client, &initial_display, "initial snapshot").await;
    sleep(Duration::from_millis(100)).await;

    state.keep_running.store(false, Ordering::Release);
    join_server_bounded(handle).await;

    let message = timeout(Duration::from_secs(1), client.next())
        .await
        .expect("client should observe shutdown within the timeout")
        .expect("client should receive a close frame before the socket ends")
        .expect("client should not see a protocol error during shutdown");

    assert!(
        matches!(message, Message::Close(_)),
        "expected close frame, got {message:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiple_clients_receive_close_frames_on_shutdown() {
    let address = free_local_address();
    let initial_display = serde_json::to_string(&DisplayPayload::new(0))
        .expect("failed to serialise initial display payload");
    let (_display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display.clone()));
    let state = Arc::new(AppState::new());

    let handle = Server::new(address, false)
        .spawn(display_rx, state.clone())
        .unwrap();

    let mut client_a = connect_client(address).await;
    let mut client_b = connect_client(address).await;
    expect_text_payload(&mut client_a, &initial_display, "client a initial snapshot").await;
    expect_text_payload(&mut client_b, &initial_display, "client b initial snapshot").await;
    sleep(Duration::from_millis(100)).await;

    state.keep_running.store(false, Ordering::Release);
    join_server_bounded(handle).await;

    // Sequential iteration is deliberate: a timeout on client_b after
    // client_a passed distinguishes "one client was cancelled" from a
    // full shutdown failure.
    for (label, client) in [("a", &mut client_a), ("b", &mut client_b)] {
        let message = timeout(Duration::from_secs(1), client.next())
            .await
            .unwrap_or_else(|_| panic!("client {label} timed out"))
            .expect("client should receive a close frame before the socket ends")
            .expect("client should not see a protocol error during shutdown");
        assert!(
            matches!(message, Message::Close(_)),
            "client {label}: {message:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_tcp_client_is_closed_after_handshake_timeout() {
    let address = free_local_address();
    let initial_display = serde_json::to_string(&DisplayPayload::new(0))
        .expect("failed to serialise initial display payload");
    let (_display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display));
    let state = Arc::new(AppState::new());

    let handle = Server::new(address, false)
        .spawn(display_rx, state.clone())
        .unwrap();

    let mut client = connect_raw_client(address).await;

    // Longer than the server handshake timeout, with extra slack for scheduler jitter.
    sleep(Duration::from_millis(1_500)).await;

    let mut buffer = [0u8; 1];
    let read_result = timeout(Duration::from_millis(250), client.read(&mut buffer)).await;

    state.keep_running.store(false, Ordering::Release);
    join_server_bounded(handle).await;

    match read_result {
        Ok(Ok(0)) => {}
        Ok(Err(error))
            if matches!(
                error.kind(),
                ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            ) => {}
        Ok(Ok(count)) => {
            panic!("expected handshake timeout to close the socket, read {count} bytes")
        }
        Ok(Err(error)) => {
            panic!("unexpected raw client read error after handshake timeout: {error}")
        }
        Err(error) => {
            panic!("idle raw TCP client was not closed after the handshake timeout: {error}")
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_with_origin_header_is_rejected_when_flag_set() {
    let address = free_local_address();
    let initial_display = serde_json::to_string(&DisplayPayload::new(0))
        .expect("failed to serialise initial display payload");
    let (_display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display));
    let state = Arc::new(AppState::new());

    let handle = Server::new(address, true)
        .spawn(display_rx, state.clone())
        .unwrap();

    let mut request = format!("ws://{address}")
        .into_client_request()
        .expect("failed to build base WebSocket request");
    request
        .headers_mut()
        .insert("Origin", "https://example.invalid".parse().unwrap());

    let connect_result = timeout(
        Duration::from_secs(1),
        tokio_tungstenite::connect_async(request),
    )
    .await;

    state.keep_running.store(false, Ordering::Release);
    join_server_bounded(handle).await;

    match connect_result {
        Ok(Err(tungstenite::Error::Http(response))) => {
            assert_eq!(response.status(), tungstenite::http::StatusCode::FORBIDDEN);
        }
        Ok(Err(error)) => panic!("expected an HTTP origin rejection, got {error}"),
        Ok(Ok((_stream, response))) => {
            panic!(
                "expected origin-bearing handshake to be rejected, got {}",
                response.status()
            )
        }
        Err(error) => {
            panic!("origin-bearing handshake timed out instead of being rejected: {error}")
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extra_client_is_refused_while_existing_clients_keep_working() {
    let address = free_local_address();
    let initial_display = serde_json::to_string(&DisplayPayload::new(1))
        .expect("failed to serialise initial display payload");
    let (display_tx, display_rx) = watch::channel(Utf8Bytes::from(initial_display.clone()));
    let state = Arc::new(AppState::new());

    let handle = Server::new(address, false)
        .spawn(display_rx, state.clone())
        .unwrap();

    let mut clients = Vec::with_capacity(MAX_CLIENTS);
    for _ in 0..MAX_CLIENTS {
        clients.push(connect_client(address).await);
    }

    for (index, client) in clients.iter_mut().enumerate() {
        expect_text_payload(
            client,
            &initial_display,
            &format!("client {index} initial snapshot"),
        )
        .await;
    }

    let extra_connect = timeout(
        Duration::from_secs(1),
        tokio_tungstenite::connect_async(format!("ws://{address}")),
    )
    .await;

    match extra_connect {
        Ok(Err(_)) => {}
        Ok(Ok((_stream, _response))) => {
            panic!(
                "expected the extra client to be refused once {MAX_CLIENTS} clients are connected"
            )
        }
        Err(error) => {
            panic!("extra client connection timed out instead of being refused: {error}")
        }
    }

    let mut updated_payload = DisplayPayload::new(1);
    updated_payload.channels[0].peak = 0.5;
    updated_payload.channels[0].bins[0] = 0.25;
    updated_payload.channels[0].bins[1] = 0.75;
    let updated_display = serde_json::to_string(&updated_payload)
        .expect("failed to serialise updated display payload");

    display_tx
        .send(Utf8Bytes::from(updated_display.clone()))
        .expect("broadcast update should reach connected clients");

    for (index, client) in clients.iter_mut().enumerate() {
        expect_text_payload(
            client,
            &updated_display,
            &format!("client {index} broadcast"),
        )
        .await;
    }

    state.keep_running.store(false, Ordering::Release);
    join_server_bounded(handle).await;
}
