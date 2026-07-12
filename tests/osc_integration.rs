//! Integration tests for the OSC sender's full send path.
//!
//! Unlike the unit tests in `src/managers/osc.rs`, which exercise encoding
//! and socket binding in isolation, these tests bind a real UDP socket to
//! receive what `OscSender::spawn` transmits, decode each packet with
//! `rosc`, and check the address and float value against the
//! `DisplayPayload` that produced it.

use phase4::app::AppState;
use phase4::dsp::{DisplayPayload, MidiSnapshot, DISPLAY_BINS};
use phase4::managers::OscSender;
use rosc::{OscPacket, OscType};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

/// Binds an ephemeral local UDP socket for the test to receive on.
async fn bind_receiver() -> (UdpSocket, SocketAddr) {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("failed to bind test UDP receiver");
    let address = socket.local_addr().expect("failed to read local address");
    (socket, address)
}

/// Receives exactly `count` OSC packets and returns each decoded message's
/// bin index, parsed from its address, mapped to its float argument.
///
/// Receiving by count rather than a fixed duration avoids a race against
/// the sender's sequential `send_to` calls. Parsing into a map rather than
/// assuming arrival order avoids depending on UDP delivery order, which is
/// not guaranteed even on loopback.
async fn receive_bin_values(socket: &UdpSocket, count: usize) -> HashMap<usize, f32> {
    let mut values = HashMap::with_capacity(count);
    let mut buffer = [0u8; 1024];

    for _ in 0..count {
        let (length, _from) = timeout(Duration::from_secs(1), socket.recv_from(&mut buffer))
            .await
            .expect("timed out waiting for an OSC packet")
            .expect("failed to receive an OSC packet");

        let (_remainder, packet) =
            rosc::decoder::decode_udp(&buffer[..length]).expect("failed to decode OSC packet");

        let OscPacket::Message(message) = packet else {
            panic!("expected an OSC message, got a bundle");
        };

        let bin = message
            .addr
            .rsplit('/')
            .next()
            .and_then(|segment| segment.parse::<usize>().ok())
            .unwrap_or_else(|| panic!("failed to parse bin index from address {}", message.addr));

        let Some(OscType::Float(value)) = message.args.first() else {
            panic!("expected a float argument, got {:?}", message.args);
        };

        values.insert(bin, *value);
    }

    values
}

/// Receives exactly `count` OSC packets and returns each decoded message's
/// address, ignoring argument values. Used to confirm which addresses fired
/// on a given frame rather than their payload.
async fn receive_addresses(socket: &UdpSocket, count: usize) -> Vec<String> {
    let mut addresses = Vec::with_capacity(count);
    let mut buffer = [0u8; 1024];
    for _ in 0..count {
        let (length, _from) = timeout(Duration::from_secs(1), socket.recv_from(&mut buffer))
            .await
            .expect("timed out waiting for an OSC packet")
            .expect("failed to receive an OSC packet");
        let (_remainder, packet) =
            rosc::decoder::decode_udp(&buffer[..length]).expect("failed to decode OSC packet");
        if let OscPacket::Message(message) = packet {
            addresses.push(message.addr);
        }
    }
    addresses
}

/// Joins the sender thread with a hard deadline, mirroring
/// `join_server_bounded` in `tests/client_server.rs`.
async fn join_sender_bounded(handle: std::thread::JoinHandle<()>) {
    const SHUTDOWN_BUDGET: Duration = Duration::from_secs(2);

    let join_task = tokio::task::spawn_blocking(move || handle.join());

    #[allow(clippy::match_wild_err_arm)]
    match tokio::time::timeout(SHUTDOWN_BUDGET, join_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(panic))) => std::panic::resume_unwind(panic),
        Ok(Err(join_err)) => panic!("spawn_blocking task failed: {join_err}"),
        Err(_) => panic!("OSC sender thread did not shut down within {SHUTDOWN_BUDGET:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_transmits_bin_values_matching_display_payload() {
    let (receiver, receiver_address) = bind_receiver().await;
    let channels = 1usize;
    let (display_tx, display_rx) = watch::channel(DisplayPayload::new(channels));
    let state = Arc::new(AppState::new());

    let handle = OscSender::new(receiver_address)
        .spawn(display_rx, channels, state.clone(), false)
        .expect("OSC sender should bind its local socket");

    let mut first_payload = DisplayPayload::new(channels);
    first_payload.channels[0].bins[0] = 0.25;
    first_payload.channels[0].bins[1] = 0.75;
    display_tx
        .send(first_payload)
        .expect("initial update should reach the OSC sender");

    let first_values = receive_bin_values(&receiver, DISPLAY_BINS).await;
    assert_eq!(
        first_values.len(),
        DISPLAY_BINS,
        "expected one packet per display bin"
    );
    assert_eq!(first_values.get(&0).copied(), Some(0.25));
    assert_eq!(first_values.get(&1).copied(), Some(0.75));

    let mut second_payload = DisplayPayload::new(channels);
    second_payload.channels[0].bins[0] = 0.1;
    second_payload.channels[0].bins[1] = 0.9;
    display_tx
        .send(second_payload)
        .expect("second update should reach the OSC sender");

    let second_values = receive_bin_values(&receiver, DISPLAY_BINS).await;
    assert_eq!(second_values.get(&0).copied(), Some(0.1));
    assert_eq!(second_values.get(&1).copied(), Some(0.9));

    state.keep_running.store(false, Ordering::Release);
    drop(display_tx);
    join_sender_bounded(handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_exits_promptly_once_the_display_channel_closes() {
    let (_receiver, receiver_address) = bind_receiver().await;
    let channels = 1usize;
    let (display_tx, display_rx) = watch::channel(DisplayPayload::new(channels));
    let state = Arc::new(AppState::new());

    let handle = OscSender::new(receiver_address)
        .spawn(display_rx, channels, state.clone(), false)
        .expect("OSC sender should bind its local socket");

    // Let the sender thread finish starting and block on display_rx.changed().
    sleep(Duration::from_millis(50)).await;

    let start = std::time::Instant::now();
    state.keep_running.store(false, Ordering::Release);
    // The sender only observes shutdown once its watch channel closes,
    // matching production ordering where the mapper (upstream) is joined
    // before the OSC sender, dropping the mapper's display_tx and closing
    // this channel from the sender's side.
    drop(display_tx);
    join_sender_bounded(handle).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(300),
        "shutdown took {elapsed:?}, expected a prompt exit once the channel closes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_forwards_midi_steps_and_transport_when_enabled() {
    let (receiver, receiver_address) = bind_receiver().await;
    let channels = 1usize;
    let (display_tx, display_rx) = watch::channel(DisplayPayload::new(channels));
    let state = Arc::new(AppState::new());

    let handle = OscSender::new(receiver_address)
        .spawn(display_rx, channels, state.clone(), true)
        .expect("OSC sender should bind its local socket");

    let mut payload = DisplayPayload::new(channels);
    payload.midi = Some(MidiSnapshot {
        transport: Some("start"),
        steps: 5,
    });
    display_tx
        .send(payload)
        .expect("update should reach the OSC sender");

    // DISPLAY_BINS bin packets, plus /phase4/midi/steps and
    // /phase4/midi/start, not /stop or /continue, on this frame.
    let addresses = receive_addresses(&receiver, DISPLAY_BINS + 2).await;
    assert!(addresses.contains(&"/phase4/midi/steps".to_string()));
    assert!(addresses.contains(&"/phase4/midi/start".to_string()));
    assert!(!addresses.contains(&"/phase4/midi/stop".to_string()));
    assert!(!addresses.contains(&"/phase4/midi/continue".to_string()));

    state.keep_running.store(false, Ordering::Release);
    drop(display_tx);
    join_sender_bounded(handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_never_forwards_midi_when_not_enabled() {
    let (receiver, receiver_address) = bind_receiver().await;
    let channels = 1usize;
    let (display_tx, display_rx) = watch::channel(DisplayPayload::new(channels));
    let state = Arc::new(AppState::new());

    let handle = OscSender::new(receiver_address)
        .spawn(display_rx, channels, state.clone(), false)
        .expect("OSC sender should bind its local socket");

    let mut payload = DisplayPayload::new(channels);
    payload.midi = Some(MidiSnapshot {
        transport: Some("start"),
        steps: 5,
    });
    display_tx
        .send(payload)
        .expect("update should reach the OSC sender");

    let addresses = receive_addresses(&receiver, DISPLAY_BINS).await;
    assert!(addresses.iter().all(|a| !a.starts_with("/phase4/midi/")));

    state.keep_running.store(false, Ordering::Release);
    drop(display_tx);
    join_sender_bounded(handle).await;
}
