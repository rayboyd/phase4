//! [`OscSender`] receives the mapped [`DisplayPayload`] over a watch channel
//! and emits one OSC float message per bin per channel over UDP.
//!
//! Address scheme: `/phase4/ch/{channel}/bin/{bin}` with a single `f` (float)
//! argument in the range 0.0..=1.0. The receiver maps these to its own
//! parameters using its OSC shortcut editor (e.g. Resolume Avenue/Arena).
//!
//! All address strings are pre-built before the send loop to avoid per-frame
//! heap allocation. The UDP socket is bound to an ephemeral local port and
//! connected to the target address so each send is a plain `socket.send(&bytes)`.
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
use std::thread::{self, JoinHandle};
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
    /// Returns an error if the local UDP socket cannot be bound or connected.
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
        socket
            .connect(self.target)
            .with_context(|| format!("Failed to connect OSC UDP socket to {}", self.target))?;

        // Pre-build all address strings before the thread starts to avoid
        // per-frame heap allocation in the send loop.
        let addrs: Vec<Vec<String>> = (0..channels)
            .map(|ch| {
                (0..DISPLAY_BINS)
                    .map(|bin| format!("/phase4/ch/{ch}/bin/{bin}"))
                    .collect()
            })
            .collect();

        let target = self.target;
        let handle = thread::Builder::new()
            .name("osc-sender".into())
            .spawn(move || {
                log::info!("OSC sender transmitting to udp://{target}");
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime for osc-sender")
                    .block_on(Self::run(display_rx, socket, addrs, channels, state));
            })
            .expect("failed to spawn osc-sender thread");

        Ok(handle)
    }

    async fn run(
        mut display_rx: watch::Receiver<DisplayPayload>,
        socket: UdpSocket,
        addrs: Vec<Vec<String>>,
        channels: usize,
        state: Arc<AppState>,
    ) {
        // Pre-allocated buffer reused every frame to avoid per-frame heap allocation.
        let mut local = DisplayPayload::new(channels);

        while state.keep_running.load(Ordering::Acquire) {
            if display_rx.changed().await.is_err() {
                log::info!("- Display channel closed, OSC sender exiting");
                break;
            }

            // Minimise the watch read-lock duration: blit values into the local
            // buffer and release the guard before any I/O work begins.
            {
                let guard = display_rx.borrow_and_update();
                for (local_ch, remote_ch) in local.channels.iter_mut().zip(guard.channels.iter()) {
                    local_ch.peak = remote_ch.peak;
                    local_ch.bins.copy_from_slice(&remote_ch.bins);
                }
            }

            for (ch_idx, channel) in local.channels.iter().enumerate() {
                let ch_addrs = &addrs[ch_idx];
                for (bin_idx, &value) in channel.bins.iter().enumerate() {
                    let packet = OscPacket::Message(OscMessage {
                        addr: ch_addrs[bin_idx].clone(),
                        args: vec![OscType::Float(value)],
                    });
                    match rosc::encoder::encode(&packet) {
                        Ok(bytes) => {
                            if let Err(e) = socket.send(&bytes) {
                                log::warn!("OSC send failed for {}: {e}", ch_addrs[bin_idx]);
                            }
                        }
                        Err(e) => {
                            log::warn!("OSC encode failed for {}: {e}", ch_addrs[bin_idx]);
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
    use rosc::encoder;

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

    // Pre-built address table must have the right shape and values.
    #[test]
    fn pre_built_address_table_has_correct_shape() {
        let channels = 2;
        let addrs: Vec<Vec<String>> = (0..channels)
            .map(|ch| {
                (0..DISPLAY_BINS)
                    .map(|bin| format!("/phase4/ch/{ch}/bin/{bin}"))
                    .collect()
            })
            .collect();

        assert_eq!(addrs.len(), channels);
        assert_eq!(addrs[0].len(), DISPLAY_BINS);
        assert_eq!(addrs[0][0], "/phase4/ch/0/bin/0");
        assert_eq!(
            addrs[1][DISPLAY_BINS - 1],
            format!("/phase4/ch/1/bin/{}", DISPLAY_BINS - 1)
        );
    }

    // Encoding a float OSC message must succeed and produce non-empty bytes.
    #[test]
    fn osc_float_encodes_without_error() {
        let packet = OscPacket::Message(OscMessage {
            addr: "/phase4/ch/0/bin/0".to_string(),
            args: vec![OscType::Float(0.5_f32)],
        });
        let bytes = encoder::encode(&packet).expect("encode must succeed");
        assert!(!bytes.is_empty(), "encoded packet must not be empty");
        assert_eq!(bytes[0], b'/', "OSC address must begin with '/'");
    }

    // The float value 0.0 and 1.0 (the range bounds) must encode cleanly.
    #[test]
    fn osc_float_encodes_range_bounds() {
        for value in [0.0_f32, 1.0_f32] {
            let packet = OscPacket::Message(OscMessage {
                addr: "/phase4/ch/0/bin/0".to_string(),
                args: vec![OscType::Float(value)],
            });
            assert!(
                encoder::encode(&packet).is_ok(),
                "encode must succeed for value {value}"
            );
        }
    }

    // Binding an ephemeral local UDP socket must succeed on any platform.
    #[test]
    fn udp_socket_bind_succeeds() {
        let socket = UdpSocket::bind("0.0.0.0:0");
        assert!(socket.is_ok(), "ephemeral UDP bind must succeed");
    }
}
