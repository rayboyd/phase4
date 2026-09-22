//! Serves one client over XPC and owns the engine run.
//!
//! The peer connection's handler answers each request. A monitor thread polls
//! the running engine and reports a failure the engine records itself, such as
//! an unplugged interface. `stop` and the monitor both end a run by taking it
//! out of the mutex before shutting it down, so exactly one of them ends each
//! run and neither waits while holding the lock.

use super::ffi::{
    self, xpc_connection_t, xpc_object_t, ArrayBuilder, ConnectionError, DictionaryBuilder,
    DictionaryRef, Kind, Owned,
};
use super::protocol::{
    parse_request, to_app_config, LinkError, Request, StartRequest, EVENT_ERROR, EVENT_STOPPED,
    KEY_AUDIO_DEVICE, KEY_BAND_COUNT, KEY_CHANNELS, KEY_CODE, KEY_CONTRACT_VERSION, KEY_DEVICES,
    KEY_ENGINE_VERSION, KEY_ERROR, KEY_FRAMES, KEY_FRAME_BYTES, KEY_MESSAGE, KEY_MIDI_DEVICE,
    KEY_NAME, KEY_OK, KEY_REASON, KEY_SAMPLE_RATE, KEY_SUPPORTED, KEY_TYPE, KEY_VERSION,
    REASON_FAILED, REASON_REQUESTED,
};
use super::CONTRACT_VERSION;
use crate::app::App;
use crate::dsp::BAND_COUNT;
use crate::frames::{FrameRegion, FrameRegionSlot};
use crate::headless::{error_code, EventCode, UNKNOWN_EVENT_CODE};
use crate::managers::{Input, MidiListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How often the monitor checks whether the engine stopped itself.
const ENGINE_POLL_INTERVAL_MS: u64 = 50;

/// Name of the thread that watches a running engine.
const MONITOR_THREAD_NAME: &str = "xpc-monitor";

/// Message for a start whose frame region was never created.
const REGION_MISSING_MESSAGE: &str = "The frame region was not created";

/// Message for a frame region libxpc could not box for sending.
const REGION_UNSHARED_MESSAGE: &str = "The frame region could not be shared";

/// The service state shared by the peer connection's handler and the monitor.
pub(crate) struct Service {
    peer: Mutex<Option<Owned>>,
    run: Mutex<Option<Running>>,
}

/// One engine run.
struct Running {
    app: App,
    /// Keeps the frame region mapped for the length of the run.
    _region: Arc<FrameRegion>,
    monitor_stop: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
}

/// Work that follows a reply, sent only once the reply is on its way.
enum AfterReply {
    Nothing,
    /// A run was stopped at the client's request.
    Stopped,
}

/// Locks `mutex`, recovering the data when a panicking thread poisoned it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Marks `reply` failed with a code and message.
fn fail(reply: &mut DictionaryBuilder, code: &str, message: &str) {
    let mut error = DictionaryBuilder::new();
    error.set_str(KEY_CODE, code).set_str(KEY_MESSAGE, message);
    reply
        .set_bool(KEY_OK, false)
        .set_value(KEY_ERROR, &error.into_owned());
}

/// A contract integer for a count or index the engine reports.
fn contract_int(value: impl TryInto<i64>) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

impl Service {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            peer: Mutex::new(None),
            run: Mutex::new(None),
        })
    }

    /// The process-wide service `xpc_main` hands peers to.
    pub(crate) fn shared() -> &'static Arc<Self> {
        static SHARED: OnceLock<Arc<Service>> = OnceLock::new();
        SHARED.get_or_init(Self::new)
    }

    /// Accepts a peer connection, or cancels it when a peer is already active.
    pub(crate) fn accept_peer(self: &Arc<Self>, connection: xpc_connection_t) {
        // SAFETY: libxpc hands the handler a live connection, which is
        // retained here so it outlives the handler call.
        let Some(connection) = (unsafe { Owned::retain(connection) }) else {
            return;
        };

        let mut peer = lock(&self.peer);
        if peer.is_some() {
            log::warn!("Refused a second client while one is connected");
            ffi::cancel(&connection);
            return;
        }
        *peer = Some(connection.clone());
        drop(peer);

        let service = Arc::clone(self);
        ffi::activate(&connection, move |event| service.handle_event(event));
        log::info!("Client connected");
    }

    /// Whether a run is active.
    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        lock(&self.run).is_some()
    }

    fn handle_event(self: &Arc<Self>, event: xpc_object_t) {
        match ffi::kind(event) {
            Kind::Dictionary => self.handle_message(event),
            Kind::Error => match ffi::connection_error(event) {
                Some(ConnectionError::Invalid | ConnectionError::TerminationImminent) => {
                    log::info!("Client disconnected");
                    self.end_run_silently();
                    *lock(&self.peer) = None;
                }
                other => log::debug!("Ignored a connection error: {other:?}"),
            },
            other => log::debug!("Ignored an XPC event of kind {other:?}"),
        }
    }

    fn handle_message(self: &Arc<Self>, event: xpc_object_t) {
        // SAFETY: the message is live for the length of this handler call.
        let Some(message) = (unsafe { DictionaryRef::from_ptr(event) }) else {
            return;
        };
        let Some(mut reply) = DictionaryBuilder::reply_to(message) else {
            log::debug!("Ignored a message that expects no reply");
            return;
        };
        reply.set_i64(KEY_VERSION, CONTRACT_VERSION);

        let after = match parse_request(message) {
            Ok(request) => self.handle_request(request, &mut reply),
            Err(error) => {
                fail(&mut reply, error.event_code(), &error.to_string());
                AfterReply::Nothing
            }
        };

        self.send_to_peer(&reply.into_owned());

        if let AfterReply::Stopped = after {
            self.send_stopped(REASON_REQUESTED);
            ffi::transaction_end();
        }
    }

    fn handle_request(
        self: &Arc<Self>,
        request: Request,
        reply: &mut DictionaryBuilder,
    ) -> AfterReply {
        match request {
            Request::Hello => {
                reply
                    .set_bool(KEY_OK, true)
                    .set_str(KEY_ENGINE_VERSION, env!("CARGO_PKG_VERSION"))
                    .set_i64(KEY_CONTRACT_VERSION, CONTRACT_VERSION)
                    .set_i64(KEY_BAND_COUNT, contract_int(BAND_COUNT));
                AfterReply::Nothing
            }
            Request::ListAudioDevices => {
                Self::list_audio_devices(reply);
                AfterReply::Nothing
            }
            Request::ListMidiDevices => {
                Self::list_midi_devices(reply);
                AfterReply::Nothing
            }
            Request::Start(start) => {
                self.start(&start, reply);
                AfterReply::Nothing
            }
            Request::Stop => self.stop(reply),
        }
    }

    fn list_audio_devices(reply: &mut DictionaryBuilder) {
        match Input::enumerate_devices() {
            Ok(entries) => {
                let mut devices = ArrayBuilder::new();
                for entry in entries {
                    let mut device = DictionaryBuilder::new();
                    device
                        .set_str(KEY_NAME, &entry.name)
                        .set_bool(KEY_SUPPORTED, entry.supported);
                    if let Some(sample_rate) = entry.sample_rate {
                        device.set_i64(KEY_SAMPLE_RATE, i64::from(sample_rate));
                    }
                    if let Some(channels) = entry.channels {
                        device.set_i64(KEY_CHANNELS, i64::from(channels));
                    }
                    devices.push(&device.into_owned());
                }
                reply
                    .set_bool(KEY_OK, true)
                    .set_value(KEY_DEVICES, &devices.into_owned());
            }
            Err(error) => fail(reply, error_code(&error), &format!("{error:#}")),
        }
    }

    fn list_midi_devices(reply: &mut DictionaryBuilder) {
        match MidiListener::enumerate_devices() {
            Ok(entries) => {
                let mut devices = ArrayBuilder::new();
                for entry in entries {
                    let mut device = DictionaryBuilder::new();
                    device.set_str(KEY_NAME, &entry.name);
                    devices.push(&device.into_owned());
                }
                reply
                    .set_bool(KEY_OK, true)
                    .set_value(KEY_DEVICES, &devices.into_owned());
            }
            Err(error) => fail(reply, error_code(&error), &format!("{error:#}")),
        }
    }

    fn start(self: &Arc<Self>, start: &StartRequest, reply: &mut DictionaryBuilder) {
        let mut run = lock(&self.run);
        if run.is_some() {
            let error = LinkError::AlreadyRunning;
            fail(reply, error.event_code(), &error.to_string());
            return;
        }

        let slot = FrameRegionSlot::new();
        let config = match to_app_config(start, &slot) {
            Ok(config) => config,
            Err(error) => {
                fail(reply, error.event_code(), &error.to_string());
                return;
            }
        };
        let mut app = match App::new_headless(&config) {
            Ok(app) => app,
            Err(error) => {
                fail(reply, error_code(&error), &format!("{error:#}"));
                return;
            }
        };
        let Some(region) = slot.region() else {
            app.shutdown();
            fail(reply, UNKNOWN_EVENT_CODE, REGION_MISSING_MESSAGE);
            return;
        };
        // SAFETY: the region is a MAP_SHARED mapping of `mapped_len` bytes
        // that the run keeps alive.
        let Some(frames) = (unsafe { ffi::shmem_create(region.as_mut_ptr(), region.mapped_len()) })
        else {
            app.shutdown();
            fail(reply, UNKNOWN_EVENT_CODE, REGION_UNSHARED_MESSAGE);
            return;
        };

        let ready = app.ready_report();
        let mut channels = ArrayBuilder::new();
        for channel in &ready.audio.channels {
            channels.push_i64(i64::from(*channel));
        }
        reply
            .set_bool(KEY_OK, true)
            .set_i64(KEY_SAMPLE_RATE, i64::from(ready.audio.sample_rate))
            .set_value(KEY_CHANNELS, &channels.into_owned())
            .set_value(KEY_FRAMES, &frames)
            .set_i64(KEY_FRAME_BYTES, contract_int(region.frame_bytes()));
        if let Some(device) = &ready.audio.device {
            reply.set_str(KEY_AUDIO_DEVICE, device);
        }
        if let Some(device) = &ready.midi_device {
            reply.set_str(KEY_MIDI_DEVICE, device);
        }

        ffi::transaction_begin();
        let monitor_stop = Arc::new(AtomicBool::new(false));
        let monitor = self.spawn_monitor(Arc::clone(&monitor_stop));
        *run = Some(Running {
            app,
            _region: region,
            monitor_stop,
            monitor: Some(monitor),
        });
        log::info!("Engine started");
    }

    fn stop(&self, reply: &mut DictionaryBuilder) -> AfterReply {
        reply.set_bool(KEY_OK, true);
        if self.end_run() {
            log::info!("Engine stopped at the client's request");
            AfterReply::Stopped
        } else {
            AfterReply::Nothing
        }
    }

    /// Takes the run out of the mutex, stops its monitor and shuts the engine
    /// down. Returns whether a run was active.
    fn end_run(&self) -> bool {
        let taken = lock(&self.run).take();
        let Some(mut running) = taken else {
            return false;
        };
        running.monitor_stop.store(true, Ordering::Release);
        if let Some(monitor) = running.monitor.take() {
            if monitor.join().is_err() {
                log::warn!("The engine monitor panicked");
            }
        }
        running.app.shutdown();
        true
    }

    /// Ends any run without sending events, for a client that has gone.
    fn end_run_silently(&self) {
        if self.end_run() {
            ffi::transaction_end();
        }
    }

    fn spawn_monitor(self: &Arc<Self>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
        let service = Arc::clone(self);
        thread::Builder::new()
            .name(MONITOR_THREAD_NAME.into())
            .spawn(move || service.monitor(&stop))
            .unwrap_or_else(|error| panic!("failed to spawn the engine monitor: {error}"))
    }

    fn monitor(&self, stop: &AtomicBool) {
        loop {
            if stop.load(Ordering::Acquire) {
                return;
            }
            thread::sleep(Duration::from_millis(ENGINE_POLL_INTERVAL_MS));
            if stop.load(Ordering::Acquire) {
                return;
            }

            let failed = {
                let mut run = lock(&self.run);
                let Some(active) = run.as_ref() else {
                    return;
                };
                active
                    .app
                    .engine_failure()
                    .map(|error| (run.take().expect("the run is present"), error))
            };
            let Some((mut running, error)) = failed else {
                continue;
            };

            log::error!("The engine stopped itself: {error:#}");
            self.send_event(EVENT_ERROR, |event| {
                event
                    .set_str(KEY_CODE, error_code(&error))
                    .set_str(KEY_MESSAGE, &format!("{error:#}"));
            });
            running.monitor = None;
            running.app.shutdown();
            self.send_stopped(REASON_FAILED);
            ffi::transaction_end();
            return;
        }
    }

    fn send_stopped(&self, reason: &str) {
        self.send_event(EVENT_STOPPED, |event| {
            event.set_str(KEY_REASON, reason);
        });
    }

    /// Sends an event with `v`, `type` and the fields `fill` sets.
    fn send_event(&self, event_type: &str, fill: impl FnOnce(&mut DictionaryBuilder)) {
        let mut event = DictionaryBuilder::new();
        event
            .set_i64(KEY_VERSION, CONTRACT_VERSION)
            .set_str(KEY_TYPE, event_type);
        fill(&mut event);
        self.send_to_peer(&event.into_owned());
    }

    fn send_to_peer(&self, message: &Owned) {
        let peer = lock(&self.peer).clone();
        if let Some(peer) = peer {
            ffi::send(&peer, message);
        }
    }
}

/// The handler `xpc::run` passes to `xpc_main`.
pub(crate) extern "C" fn accept_peer_connection(connection: xpc_connection_t) {
    Service::shared().accept_peer(connection);
}
