//! [`OscSender`] receives the mapped [`DisplayPayload`] over a watch channel
//! and emits one OSC float message per bin per channel over UDP.
//!
//! Address scheme: `/phase4/ch/{channel}/bin/{bin}` with a single `f` (float)
//! argument in the range 0.0..=1.0. The receiver maps these to its own
//! parameters using its OSC shortcut editor (e.g. `TouchDesigner` OSC In CHOP).
//!
//! Message structures and addresses are pre-built once before the send loop.
//! Each frame, only the float argument is mutated in place. The UDP socket is
//! bound to an ephemeral local port and kept unconnected, so each send uses
//! `socket.send_to(&bytes, target)`. A reusable Vec<u8> scratch buffer is
//! cleared and reused for each frame's encoding, so the steady-state send loop
//! performs no heap allocation.
//!
//! OSC uses UDP so no connection management, handshake, or backpressure exists.
//! The sender is a plain OS thread with a minimal single-threaded Tokio runtime,
//! required only to await the watch channel in the same pattern as the mapper.

use crate::app::AppState;
use crate::dsp::{DisplayPayload, DISPLAY_BINS};
use anyhow::{Context, Result};
use rosc::{OscMessage, OscPacket, OscType};
use std::net::{SocketAddr, UdpSocket};
use std::sync::{atomic::Ordering, Arc};
use std::thread::JoinHandle;
use tokio::sync::watch;

pub struct OscSender {
    target: SocketAddr,
}

impl OscSender {
    #[must_use]
    pub fn new(target: SocketAddr) -> Self {
        Self { target }
    }

    /// Spawns the OSC sender on a dedicated background thread.
    ///
    /// Binds an ephemeral local UDP socket eagerly so any bind error surfaces
    /// here as a `Result` rather than panicking inside the spawned thread.
    ///
    /// # Errors
    ///
    /// Returns an error if the local UDP socket cannot be bound.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned or if the single-threaded
    /// Tokio runtime cannot be built.
    pub fn spawn(
        self,
        display_rx: watch::Receiver<DisplayPayload>,
        channels: usize,
        state: Arc<AppState>,
    ) -> Result<JoinHandle<()>> {
        let socket =
            UdpSocket::bind("0.0.0.0:0").context("Failed to bind UDP socket for OSC output")?;

        let packets = Self::build_packets(channels);
        let target = self.target;
        let handle = super::spawn_async_worker("osc-sender", async move {
            log::info!("OSC sender transmitting to udp://{target}");
            OscRuntime {
                display_rx,
                socket,
                target,
                packets,
                scratch: Vec::new(),
                state,
                send_failure_logged: false,
                encode_failure_logged: false,
            }
            .run()
            .await;
        });

        Ok(handle)
    }

    /// Pre-builds the packet table for a given channel count.
    ///
    /// Each packet is an `OscMessage` with address and a placeholder float argument.
    /// The table is flattened into a single Vec indexed by ch * `DISPLAY_BINS` + bin.
    fn build_packets(channels: usize) -> Vec<OscPacket> {
        let mut packets = Vec::with_capacity(channels * DISPLAY_BINS);
        for ch in 0..channels {
            for bin in 0..DISPLAY_BINS {
                let addr = format!("/phase4/ch/{ch}/bin/{bin}");
                packets.push(OscPacket::Message(OscMessage {
                    addr,
                    args: vec![OscType::Float(0.0)],
                }));
            }
        }
        packets
    }
}

/// Runtime state for the OSC sender thread.
///
/// Owns the watch receiver, unconnected UDP socket, target address, pre-built
/// packet table, reusable encode scratch buffer, and app state. The async run
/// loop is a method on this struct.
struct OscRuntime {
    display_rx: watch::Receiver<DisplayPayload>,
    socket: UdpSocket,
    target: SocketAddr,
    packets: Vec<OscPacket>,
    scratch: Vec<u8>,
    state: Arc<AppState>,
    send_failure_logged: bool,
    encode_failure_logged: bool,
}

impl OscRuntime {
    /// Main async loop for the OSC sender.
    ///
    /// Waits for the display channel to signal a change, reads the payload
    /// with minimal lock duration, updates each packet's float argument,
    /// encodes and sends all packets, then loops.
    async fn run(mut self) {
        while self.state.keep_running.load(Ordering::Acquire) {
            if self.display_rx.changed().await.is_err() {
                log::info!("- Display channel closed, OSC sender exiting");
                break;
            }

            // Minimise the watch read-lock duration: update packet values and release
            // the guard before any encoding or I/O work begins.
            {
                let guard = self.display_rx.borrow_and_update();
                for (ch_packets, channel) in self
                    .packets
                    .chunks_exact_mut(DISPLAY_BINS)
                    .zip(guard.channels.iter())
                {
                    for (packet, &bin_value) in ch_packets.iter_mut().zip(channel.bins.iter()) {
                        if let OscPacket::Message(msg) = packet {
                            if let Some(OscType::Float(ref mut f)) = msg.args.first_mut() {
                                *f = bin_value;
                            }
                        }
                    }
                }
            }

            // Encode and send each packet. No allocations occur in this loop.
            for packet in &self.packets {
                self.scratch.clear();
                match rosc::encoder::encode_into(packet, &mut self.scratch) {
                    Ok(_) => {
                        self.encode_failure_logged = false;

                        match self.socket.send_to(&self.scratch, self.target) {
                            Ok(_) => self.send_failure_logged = false,
                            Err(e) => {
                                if !self.send_failure_logged {
                                    if let OscPacket::Message(msg) = packet {
                                        log::warn!(
                                            "OSC send failed for {}: {e}. Further send failures until \
                                             the next successful send will not be logged individually.",
                                            msg.addr
                                        );
                                    }
                                    self.send_failure_logged = true;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if !self.encode_failure_logged {
                            if let OscPacket::Message(msg) = packet {
                                log::warn!(
                                    "OSC encode failed for {}: {e}. Further encode failures until \
                                     the next successful encode will not be logged individually.",
                                    msg.addr
                                );
                            }
                            self.encode_failure_logged = true;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pre-built packet table must have the right shape and structure.
    #[test]
    fn pre_built_address_table_has_correct_shape() {
        let channels = 2;
        let packets = OscSender::build_packets(channels);

        assert_eq!(
            packets.len(),
            channels * DISPLAY_BINS,
            "packet table length must be channels * DISPLAY_BINS"
        );

        // Check a specific index mapping.
        let ch_0_bin_0_idx = 0;
        let ch_1_bin_5_idx = DISPLAY_BINS + 5;

        if let OscPacket::Message(msg) = &packets[ch_0_bin_0_idx] {
            assert_eq!(msg.addr, "/phase4/ch/0/bin/0");
            assert_eq!(msg.args.len(), 1);
            assert!(matches!(msg.args[0], OscType::Float(_)));
        } else {
            panic!("packet at [{ch_0_bin_0_idx}] must be an OscMessage");
        }

        if let OscPacket::Message(msg) = &packets[ch_1_bin_5_idx] {
            assert_eq!(msg.addr, "/phase4/ch/1/bin/5");
            assert_eq!(msg.args.len(), 1);
            assert!(matches!(msg.args[0], OscType::Float(_)));
        } else {
            panic!("packet at [{ch_1_bin_5_idx}] must be an OscMessage");
        }
    }

    // Address strings must follow the /phase4/ch/{n}/bin/{n} scheme exactly.
    #[test]
    fn osc_address_format_is_correct() {
        assert_eq!(
            format!("/phase4/ch/{ch}/bin/{bin}", ch = 0, bin = 0),
            "/phase4/ch/0/bin/0"
        );
        assert_eq!(
            format!("/phase4/ch/{ch}/bin/{bin}", ch = 1, bin = 63),
            "/phase4/ch/1/bin/63"
        );
    }

    // Encoding a float OSC message with encode_into must succeed and produce
    // non-empty bytes. The scratch buffer must be cleared before each encode.
    #[test]
    fn osc_float_encodes_with_encode_into() {
        let packet = OscPacket::Message(OscMessage {
            addr: "/phase4/ch/0/bin/0".to_string(),
            args: vec![OscType::Float(0.5_f32)],
        });

        let mut scratch = Vec::new();
        scratch.clear();
        let result = rosc::encoder::encode_into(&packet, &mut scratch);
        assert!(result.is_ok(), "encode_into must succeed");
        assert!(!scratch.is_empty(), "encoded packet must not be empty");
        assert_eq!(scratch[0], b'/', "OSC address must begin with '/'");

        // Second encode into the same cleared buffer.
        scratch.clear();
        let packet2 = OscPacket::Message(OscMessage {
            addr: "/phase4/ch/1/bin/10".to_string(),
            args: vec![OscType::Float(0.75_f32)],
        });
        let result2 = rosc::encoder::encode_into(&packet2, &mut scratch);
        assert!(result2.is_ok(), "second encode_into must succeed");
        assert!(
            !scratch.is_empty(),
            "second encoded packet must not be empty"
        );
    }

    // The float value 0.0 and 1.0 (the range bounds) must encode cleanly.
    #[test]
    fn osc_float_encodes_range_bounds() {
        for value in [0.0_f32, 1.0_f32] {
            let packet = OscPacket::Message(OscMessage {
                addr: "/phase4/ch/0/bin/0".to_string(),
                args: vec![OscType::Float(value)],
            });
            let mut scratch = Vec::new();
            assert!(
                rosc::encoder::encode_into(&packet, &mut scratch).is_ok(),
                "encode_into must succeed for value {value}"
            );
        }
    }

    // Binding an ephemeral local UDP socket must succeed on any platform.
    #[test]
    fn udp_socket_bind_succeeds() {
        let socket = UdpSocket::bind("0.0.0.0:0");
        assert!(socket.is_ok(), "ephemeral UDP bind must succeed");
    }

    // send_to on an unconnected socket must not error locally, even when
    // the destination has nothing listening. This is what makes packets
    // silently dropped rather than surfaced as ECONNREFUSED, the documented
    // fire-and-forget contract in docs/tutorials/osc.md. A connected UDP
    // socket instead receives a delayed ICMP error on a later send call,
    // which cannot be asserted deterministically in one synchronous test,
    // so treat this as a smoke test for the intended code path rather than
    // a full proof of the underlying OS behaviour.
    #[test]
    fn send_to_unconnected_socket_does_not_error_locally() {
        let socket = UdpSocket::bind("0.0.0.0:0").expect("ephemeral UDP bind must succeed");
        let closed_target: SocketAddr = "127.0.0.1:1".parse().expect("valid address");
        let result = socket.send_to(b"test", closed_target);
        assert!(
            result.is_ok(),
            "send_to must not error locally for an unconnected socket"
        );
    }
}
