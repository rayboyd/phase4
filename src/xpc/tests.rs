//! End to end checks of the service over an anonymous XPC listener in this
//! process. Each test builds its own service, listener and client, so the
//! link contract is exercised exactly as the app will use it, without
//! launchd or a bundle.

use super::ffi::{self, ArrayBuilder, DictionaryBuilder, DictionaryRef, Kind, Owned};
use super::protocol::{
    EVENT_STOPPED, KEY_AUDIO_DEVICE, KEY_BAND_COUNT, KEY_BPM, KEY_CHANNELS, KEY_CODE,
    KEY_CONFIG_YAML, KEY_CONTRACT_VERSION, KEY_DEVICES, KEY_DEVICE_NAME, KEY_ENGINE_VERSION,
    KEY_ERROR, KEY_FRAMES, KEY_FRAME_BYTES, KEY_HZ, KEY_KIND, KEY_MIDI, KEY_NAME, KEY_OK,
    KEY_REASON, KEY_SAMPLE_RATE, KEY_SOURCE, KEY_SUPPORTED, KEY_TYPE, KEY_VERSION, MIDI_TEST_CLOCK,
    REASON_REQUESTED, SOURCE_DEVICE, SOURCE_TEST_TONE, TYPE_HELLO, TYPE_LIST_AUDIO_DEVICES,
    TYPE_LIST_MIDI_DEVICES, TYPE_START, TYPE_STOP,
};
use super::service::Service;
use super::CONTRACT_VERSION;
use crate::frames::{
    frame_bytes, read_frame, read_header, uptime_raw_ns, Frame, FrameHeader, FRAME_LAYOUT_VERSION,
    MIDI_FLAG_CONFIGURED,
};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const TONE_HZ: f64 = 440.0;
const CLOCK_BPM: f64 = 300.0;
const CALIBRATION_SAMPLE_RATE: i64 = 44_100;
const CALIBRATION_CHANNELS: [i64; 2] = [0, 1];
const MISSING_DEVICE: &str = "Phase4 Test No Such Device";
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(2);
const EVENT_TIMEOUT: Duration = Duration::from_secs(2);
const MIDI_TIMEOUT: Duration = Duration::from_secs(1);
const QUIET_PERIOD: Duration = Duration::from_millis(200);
const FRAME_ADVANCE_WAIT: Duration = Duration::from_millis(100);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MIDI_TRANSPORT_START: u32 = 1;

/// A service with one connected client, and the events that client received.
struct Harness {
    service: Arc<Service>,
    listener: Owned,
    endpoint: Owned,
    client: Owned,
    events: Receiver<Owned>,
}

impl Harness {
    fn new() -> Self {
        let service = Service::new();
        let listener = ffi::anonymous_listener();
        let accepting = Arc::clone(&service);
        ffi::activate(&listener, move |event| {
            if ffi::kind(event) == Kind::Connection {
                accepting.accept_peer(event);
            }
        });
        let endpoint = ffi::endpoint(&listener);
        let (client, events) = Self::connect(&endpoint);
        Self {
            service,
            listener,
            endpoint,
            client,
            events,
        }
    }

    fn connect(endpoint: &Owned) -> (Owned, Receiver<Owned>) {
        let client = ffi::connect(endpoint);
        let (sender, events) = mpsc::channel();
        ffi::activate(&client, move |event| {
            // SAFETY: the event is live for the handler call and retained here.
            if let Some(event) = unsafe { Owned::retain(event) } {
                let _ = sender.send(event);
            }
        });
        (client, events)
    }

    fn request(&self, message: DictionaryBuilder) -> Owned {
        ffi::send_with_reply_sync(&self.client, &message.into_owned()).expect("a reply arrives")
    }

    /// The next event with `type`, skipping connection errors.
    fn event(&self, event_type: &str, timeout: Duration) -> Option<Owned> {
        let deadline = Instant::now() + timeout;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let event = self.events.recv_timeout(remaining).ok()?;
            let matches = event
                .as_dictionary()
                .and_then(|dictionary| dictionary.get_string(KEY_TYPE))
                .is_some_and(|found| found == event_type);
            if matches {
                return Some(event);
            }
        }
        None
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        ffi::cancel(&self.client);
        ffi::cancel(&self.listener);
    }
}

/// A frame region mapped into this process, unmapped on drop.
struct Mapped {
    base: NonNull<c_void>,
    len: usize,
}

impl Mapped {
    fn from_reply(reply: DictionaryRef<'_>) -> Self {
        let frames = reply
            .get_value(KEY_FRAMES)
            .expect("the reply shares frames");
        let (base, len) = ffi::shmem_map(frames).expect("the frame region maps");
        Self { base, len }
    }

    fn header(&self) -> Option<FrameHeader> {
        // SAFETY: the mapping is live and page aligned.
        unsafe { read_header(self.base.as_ptr().cast(), self.len) }
    }

    fn frame(&self) -> Option<Frame> {
        // SAFETY: the mapping is live and page aligned.
        unsafe { read_frame(self.base.as_ptr().cast(), self.len) }
    }

    /// The first frame matching `accept` within `timeout`.
    fn wait_for(&self, timeout: Duration, accept: impl Fn(&Frame) -> bool) -> Option<Frame> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(frame) = self.frame().filter(|frame| accept(frame)) {
                return Some(frame);
            }
            thread::sleep(POLL_INTERVAL);
        }
        None
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: the mapping came from xpc_shmem_map with this length.
        unsafe { libc::munmap(self.base.as_ptr(), self.len) };
    }
}

fn message(request_type: &str) -> DictionaryBuilder {
    let mut builder = DictionaryBuilder::new();
    builder
        .set_i64(KEY_VERSION, CONTRACT_VERSION)
        .set_str(KEY_TYPE, request_type);
    builder
}

fn tone_start() -> DictionaryBuilder {
    let mut source = DictionaryBuilder::new();
    source
        .set_str(KEY_KIND, SOURCE_TEST_TONE)
        .set_f64(KEY_HZ, TONE_HZ);
    let mut builder = message(TYPE_START);
    builder.set_value(KEY_SOURCE, &source.into_owned());
    builder
}

fn tone_start_with_config(config: &str) -> DictionaryBuilder {
    let mut builder = tone_start();
    builder.set_str(KEY_CONFIG_YAML, config);
    builder
}

fn device_start(name: &str, channels: &[i64]) -> DictionaryBuilder {
    let mut list = ArrayBuilder::new();
    for channel in channels {
        list.push_i64(*channel);
    }
    let mut source = DictionaryBuilder::new();
    source
        .set_str(KEY_KIND, SOURCE_DEVICE)
        .set_str(KEY_DEVICE_NAME, name)
        .set_value(KEY_CHANNELS, &list.into_owned());
    let mut builder = message(TYPE_START);
    builder.set_value(KEY_SOURCE, &source.into_owned());
    builder
}

fn dictionary(reply: &Owned) -> DictionaryRef<'_> {
    reply.as_dictionary().expect("the reply is a dictionary")
}

fn assert_ok(reply: &Owned) {
    let reply = dictionary(reply);
    assert_eq!(reply.get_i64(KEY_VERSION), Some(CONTRACT_VERSION));
    assert_eq!(reply.get_bool(KEY_OK), Some(true), "the reply must succeed");
}

fn error_code_of(reply: &Owned) -> String {
    let reply = dictionary(reply);
    assert_eq!(reply.get_i64(KEY_VERSION), Some(CONTRACT_VERSION));
    assert_eq!(reply.get_bool(KEY_OK), Some(false), "the reply must fail");
    let error = reply
        .get_dictionary(KEY_ERROR)
        .expect("a failure carries an error");
    assert!(error.get_string(super::protocol::KEY_MESSAGE).is_some());
    error.get_string(KEY_CODE).expect("an error carries a code")
}

fn int_array(reply: DictionaryRef<'_>, key: &std::ffi::CStr) -> Vec<i64> {
    let array = reply.get_array(key).expect("an array");
    (0..array.len())
        .map(|index| array.get_i64(index).expect("an int64"))
        .collect()
}

#[test]
fn hello_replies_with_the_versions_and_band_count() {
    let harness = Harness::new();
    let reply = harness.request(message(TYPE_HELLO));

    assert_ok(&reply);
    let reply = dictionary(&reply);
    assert_eq!(
        reply.get_string(KEY_ENGINE_VERSION).as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(reply.get_i64(KEY_CONTRACT_VERSION), Some(1));
    assert_eq!(reply.get_i64(KEY_BAND_COUNT), Some(32));
}

#[test]
fn another_version_and_an_unknown_type_are_refused() {
    let harness = Harness::new();

    let mut other_version = message(TYPE_HELLO);
    other_version.set_i64(KEY_VERSION, CONTRACT_VERSION + 1);
    assert_eq!(
        error_code_of(&harness.request(other_version)),
        "ContractMismatch"
    );

    assert_eq!(
        error_code_of(&harness.request(message("restart"))),
        "InvalidRequest"
    );
}

#[test]
fn device_listings_reply_with_the_documented_keys() {
    let harness = Harness::new();

    let audio = harness.request(message(TYPE_LIST_AUDIO_DEVICES));
    assert_ok(&audio);
    let devices = dictionary(&audio)
        .get_array(KEY_DEVICES)
        .expect("a devices array");
    for index in 0..devices.len() {
        let device = devices.get_dictionary(index).expect("a device dictionary");
        assert!(device.get_string(KEY_NAME).is_some());
        assert!(device.get_bool(KEY_SUPPORTED).is_some());
        assert!(!device.contains(KEY_SAMPLE_RATE) || device.get_i64(KEY_SAMPLE_RATE).is_some());
        assert!(!device.contains(KEY_CHANNELS) || device.get_i64(KEY_CHANNELS).is_some());
    }

    let midi = harness.request(message(TYPE_LIST_MIDI_DEVICES));
    assert_ok(&midi);
    let devices = dictionary(&midi)
        .get_array(KEY_DEVICES)
        .expect("a devices array");
    for index in 0..devices.len() {
        let device = devices.get_dictionary(index).expect("a device dictionary");
        assert!(device.get_string(KEY_NAME).is_some());
    }
}

#[test]
fn a_tone_start_shares_a_live_frame_region_until_stop() {
    let harness = Harness::new();
    let reply = harness.request(tone_start());

    assert_ok(&reply);
    let fields = dictionary(&reply);
    assert_eq!(
        fields.get_i64(KEY_SAMPLE_RATE),
        Some(CALIBRATION_SAMPLE_RATE)
    );
    assert_eq!(int_array(fields, KEY_CHANNELS), CALIBRATION_CHANNELS);
    assert!(!fields.contains(KEY_AUDIO_DEVICE));
    assert_eq!(
        fields.get_i64(KEY_FRAME_BYTES),
        Some(i64::try_from(frame_bytes(CALIBRATION_CHANNELS.len())).unwrap())
    );

    let mapped = Mapped::from_reply(fields);
    assert_eq!(
        mapped.header(),
        Some(FrameHeader {
            layout_version: FRAME_LAYOUT_VERSION,
            channel_count: 2,
            band_count: 32,
            sample_rate: 44_100,
            midi_flags: 0,
        })
    );

    let first = mapped
        .wait_for(FIRST_FRAME_TIMEOUT, |_| true)
        .expect("a frame arrives within 2 seconds");
    assert!(first.sequence >= 2 && first.sequence.is_multiple_of(2));
    assert!(first.published_ns <= uptime_raw_ns());
    assert_eq!(first.channels.len(), 2);
    assert!(first
        .channels
        .iter()
        .all(|channel| channel.peak.is_finite() && channel.bins.iter().all(|bin| bin.is_finite())));

    thread::sleep(FRAME_ADVANCE_WAIT);
    let later = mapped.frame().expect("frames keep arriving");
    assert!(later.sequence > first.sequence);

    assert_eq!(
        error_code_of(&harness.request(tone_start())),
        "AlreadyRunning"
    );

    assert_ok(&harness.request(message(TYPE_STOP)));
    let stopped = harness
        .event(EVENT_STOPPED, EVENT_TIMEOUT)
        .expect("a stopped event arrives");
    assert_eq!(
        dictionary(&stopped).get_string(KEY_REASON).as_deref(),
        Some(REASON_REQUESTED)
    );
    assert!(!harness.service.is_running());
}

#[test]
fn stop_while_idle_replies_ok_and_sends_no_event() {
    let harness = Harness::new();
    assert_ok(&harness.request(message(TYPE_STOP)));
    assert!(harness.event(EVENT_STOPPED, QUIET_PERIOD).is_none());
}

#[test]
fn a_test_clock_reaches_the_frame() {
    let harness = Harness::new();
    let mut clock = DictionaryBuilder::new();
    clock
        .set_str(KEY_KIND, MIDI_TEST_CLOCK)
        .set_f64(KEY_BPM, CLOCK_BPM);
    let mut start = tone_start();
    start.set_value(KEY_MIDI, &clock.into_owned());

    let reply = harness.request(start);
    assert_ok(&reply);
    let mapped = Mapped::from_reply(dictionary(&reply));
    assert_eq!(
        mapped.header().map(|header| header.midi_flags),
        Some(MIDI_FLAG_CONFIGURED)
    );

    let frame = mapped
        .wait_for(MIDI_TIMEOUT, |frame| frame.midi.steps > 0)
        .expect("MIDI steps reach the frame within 1 second");
    assert!(frame.midi.transport_count >= 1);
    assert_eq!(frame.midi.transport_last, MIDI_TRANSPORT_START);
}

#[test]
fn config_text_is_validated_and_its_network_section_ignored() {
    let harness = Harness::new();

    assert_eq!(
        error_code_of(&harness.request(tone_start_with_config("vocoder: ["))),
        "ConfigFileParseError"
    );
    assert_eq!(
        error_code_of(&harness.request(tone_start_with_config("vocoder:\n  attack_ms: 0\n"))),
        "InvalidAttackTime"
    );

    let reply = harness.request(tone_start_with_config(
        "network:\n  ws_addr: \"127.0.0.1:8889\"\n",
    ));
    assert_ok(&reply);
    assert_ok(&harness.request(message(TYPE_STOP)));
}

#[test]
fn device_failures_reply_with_the_engine_codes() {
    let harness = Harness::new();
    assert_eq!(
        error_code_of(&harness.request(device_start(MISSING_DEVICE, &[]))),
        "EmptyChannelSelection"
    );
    assert_eq!(
        error_code_of(&harness.request(device_start(MISSING_DEVICE, &[0]))),
        "NoMatch"
    );
    assert!(!harness.service.is_running());
}

#[test]
fn a_second_client_is_refused_and_the_first_run_continues() {
    let harness = Harness::new();
    let reply = harness.request(tone_start());
    assert_ok(&reply);
    let mapped = Mapped::from_reply(dictionary(&reply));

    let (second, _events) = Harness::connect(&harness.endpoint);
    let refused = ffi::send_with_reply_sync(&second, &message(TYPE_HELLO).into_owned())
        .expect("the second client receives something");
    assert_eq!(refused.kind(), Kind::Error);
    ffi::cancel(&second);

    let before = mapped
        .wait_for(FIRST_FRAME_TIMEOUT, |_| true)
        .expect("the first run is live");
    thread::sleep(FRAME_ADVANCE_WAIT);
    assert!(mapped.frame().expect("frames keep arriving").sequence > before.sequence);
    assert!(harness.service.is_running());
    assert_ok(&harness.request(message(TYPE_STOP)));
}

#[test]
fn cancelling_the_client_stops_the_run() {
    let harness = Harness::new();
    assert_ok(&harness.request(tone_start()));
    assert!(harness.service.is_running());

    ffi::cancel(&harness.client);

    let deadline = Instant::now() + EVENT_TIMEOUT;
    while harness.service.is_running() && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
    }
    assert!(!harness.service.is_running());
}
