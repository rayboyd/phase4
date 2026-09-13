//! Serialisation checks for the headless event stream.
//!
//! The emitted lines are a public contract. A host parses them to supervise the
//! engine, so field order, names and the schema version are asserted exactly.

use phase4::config::AppConfigError;
use phase4::headless::{
    headless_stop, is_broken_pipe, watch_stdin, write_event, Event, EventCode, HeadlessStop,
    ReadyAudio, ReadyOutputs, ReadyReport, ShutdownReason, EVENT_SCHEMA_VERSION,
    STDIN_WATCHER_THREAD_NAME,
};
use phase4::managers::audio::DeviceError;
use phase4::managers::midi::MidiDeviceError;
use std::io::{Cursor, ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn line(event: &Event) -> String {
    let mut buffer = Vec::new();
    write_event(&mut buffer, event).expect("writing an event must succeed");
    String::from_utf8(buffer).expect("event lines must be UTF-8")
}

fn ready() -> Event {
    Event::Ready(ReadyReport {
        audio: ReadyAudio {
            device: Some("Duet 3".to_owned()),
            sample_rate: 48_000,
            channels: vec![0, 1],
        },
        midi_device: None,
        outputs: ReadyOutputs {
            websocket: Some("127.0.0.1:53412".parse().expect("valid address")),
            osc: None,
        },
    })
}

#[test]
fn ready_serialises_with_the_documented_shape() {
    assert_eq!(
        line(&ready()),
        concat!(
            r#"{"v":1,"event":"ready","audio":{"device":"Duet 3","sample_rate":48000,"#,
            r#""channels":[0,1]},"midi_device":null,"#,
            r#""outputs":{"websocket":"127.0.0.1:53412","osc":null}}"#,
            "\n"
        )
    );
}

#[test]
fn error_serialises_with_the_documented_shape() {
    let event = Event::Error {
        code: "NoOutputConfigured".to_owned(),
        message: "no output".to_owned(),
    };
    assert_eq!(
        line(&event),
        "{\"v\":1,\"event\":\"error\",\"code\":\"NoOutputConfigured\",\"message\":\"no output\"}\n"
    );
}

#[test]
fn shutdown_serialises_with_the_documented_shape() {
    let event = Event::Shutdown {
        reason: ShutdownReason::Signal,
    };
    assert_eq!(
        line(&event),
        "{\"v\":1,\"event\":\"shutdown\",\"reason\":\"signal\"}\n"
    );
}

#[test]
fn a_stdin_closed_shutdown_serialises_with_the_documented_shape() {
    let event = Event::Shutdown {
        reason: ShutdownReason::StdinClosed,
    };
    assert_eq!(
        line(&event),
        "{\"v\":1,\"event\":\"shutdown\",\"reason\":\"stdin_closed\"}\n"
    );
}

#[test]
fn a_calibration_ready_reports_no_device_and_every_channel() {
    let event = Event::Ready(ReadyReport {
        audio: ReadyAudio {
            device: None,
            sample_rate: 44_100,
            channels: vec![0, 1],
        },
        midi_device: Some("Loopback".to_owned()),
        outputs: ReadyOutputs {
            websocket: None,
            osc: Some("127.0.0.1:7000".parse().expect("valid address")),
        },
    });
    let text = line(&event);
    assert!(text.contains(r#""device":null"#), "got: {text}");
    assert!(text.contains(r#""midi_device":"Loopback""#), "got: {text}");
    assert!(text.contains(r#""osc":"127.0.0.1:7000""#), "got: {text}");
    assert!(text.contains(r#""websocket":null"#), "got: {text}");
}

#[test]
fn every_line_carries_the_schema_version_and_one_trailing_newline() {
    let events = [
        ready(),
        Event::Error {
            code: "EmptyQuery".to_owned(),
            message: "empty".to_owned(),
        },
        Event::Shutdown {
            reason: ShutdownReason::Signal,
        },
        Event::Shutdown {
            reason: ShutdownReason::StdinClosed,
        },
    ];
    for event in &events {
        let text = line(event);
        assert!(
            text.starts_with(&format!(r#"{{"v":{EVENT_SCHEMA_VERSION},"#)),
            "every line must open with the schema version, got: {text}"
        );
        assert_eq!(
            text.matches('\n').count(),
            1,
            "every line must carry exactly one newline, got: {text}"
        );
        assert!(
            text.ends_with('\n'),
            "the newline must terminate the line, got: {text}"
        );
    }
}

#[test]
fn device_error_codes_are_the_variant_names() {
    let cases: [(DeviceError, &str); 4] = [
        (DeviceError::EmptyQuery, "EmptyQuery"),
        (
            DeviceError::NoMatch {
                query: "nothing".to_owned(),
            },
            "NoMatch",
        ),
        (
            DeviceError::UnsupportedFormat {
                format: "i16".to_owned(),
            },
            "UnsupportedFormat",
        ),
        (
            DeviceError::HardwareStreamError {
                message: "device unplugged".to_owned(),
            },
            "HardwareStreamError",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.event_code(), expected);
    }
}

#[test]
fn config_error_codes_are_the_variant_names() {
    let cases: [(AppConfigError, &str); 5] = [
        (AppConfigError::MissingDevice, "MissingDevice"),
        (AppConfigError::NoOutputConfigured, "NoOutputConfigured"),
        (AppConfigError::InvalidMaxClients, "InvalidMaxClients"),
        (
            AppConfigError::InvalidAttackTime { value: -1.0 },
            "InvalidAttackTime",
        ),
        (
            AppConfigError::ChannelIndexOutOfRange {
                idx: 9,
                channels: 2,
            },
            "ChannelIndexOutOfRange",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.event_code(), expected);
    }
}

#[test]
fn an_error_event_carries_the_originating_message() {
    let event = Event::from_error(&AppConfigError::NoOutputConfigured);
    let Event::Error { code, message } = event else {
        panic!("from_error must build an error event");
    };
    assert_eq!(code, "NoOutputConfigured");
    assert_eq!(message, AppConfigError::NoOutputConfigured.to_string());
}

#[test]
fn an_untyped_error_reports_the_unknown_code() {
    let event = Event::from_anyhow(&anyhow::anyhow!("something else went wrong"));
    let Event::Error { code, message } = event else {
        panic!("from_anyhow must build an error event");
    };
    assert_eq!(code, "Unknown");
    assert!(
        message.contains("something else went wrong"),
        "got: {message}"
    );
}

#[test]
fn a_typed_error_is_recovered_from_an_anyhow_chain() {
    let error = anyhow::Error::from(AppConfigError::NoOutputConfigured);
    let Event::Error { code, .. } = Event::from_anyhow(&error) else {
        panic!("from_anyhow must build an error event");
    };
    assert_eq!(code, "NoOutputConfigured");
}

#[test]
fn a_hardware_stream_error_reports_its_code_and_text() {
    let error = anyhow::Error::from(DeviceError::HardwareStreamError {
        message: "device unplugged".to_owned(),
    });
    let Event::Error { code, message } = Event::from_anyhow(&error) else {
        panic!("from_anyhow must build an error event");
    };
    assert_eq!(code, "HardwareStreamError");
    assert!(message.contains("device unplugged"), "got: {message}");
}

#[test]
fn midi_device_error_codes_are_the_variant_names() {
    let cases: [(MidiDeviceError, &str); 3] = [
        (
            MidiDeviceError::MidiUnavailable {
                message: "no backend".to_owned(),
            },
            "MidiUnavailable",
        ),
        (
            MidiDeviceError::MidiNoMatch {
                query: "Loopback".to_owned(),
            },
            "MidiNoMatch",
        ),
        (
            MidiDeviceError::MidiConnectFailed {
                device: "Loopback".to_owned(),
                message: "port busy".to_owned(),
            },
            "MidiConnectFailed",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.event_code(), expected);
    }
}

#[test]
fn a_midi_device_error_is_recovered_from_an_anyhow_chain() {
    let error = anyhow::Error::from(MidiDeviceError::MidiNoMatch {
        query: "Loopback".to_owned(),
    });
    let Event::Error { code, message } = Event::from_anyhow(&error) else {
        panic!("from_anyhow must build an error event");
    };
    assert_eq!(code, "MidiNoMatch");
    assert!(message.contains("Loopback"), "got: {message}");
}

#[test]
fn a_running_engine_does_not_stop() {
    assert_eq!(headless_stop(false, false, true), None);
}

#[test]
fn each_stop_condition_maps_to_its_outcome() {
    assert_eq!(
        headless_stop(true, false, true),
        Some(HeadlessStop::Requested(ShutdownReason::Signal))
    );
    assert_eq!(
        headless_stop(false, true, true),
        Some(HeadlessStop::Requested(ShutdownReason::StdinClosed))
    );
    assert_eq!(
        headless_stop(false, false, false),
        Some(HeadlessStop::Engine)
    );
}

#[test]
fn a_signal_takes_precedence_over_every_other_stop() {
    assert_eq!(
        headless_stop(true, true, false),
        Some(HeadlessStop::Requested(ShutdownReason::Signal))
    );
}

#[test]
fn a_closed_stdin_takes_precedence_over_an_engine_stop() {
    assert_eq!(
        headless_stop(false, true, false),
        Some(HeadlessStop::Requested(ShutdownReason::StdinClosed))
    );
}

/// Runs the watcher over `reader` to completion and reports whether it set the flag.
fn watch_to_end(reader: impl Read + Send + 'static) -> bool {
    let closed = Arc::new(AtomicBool::new(false));
    watch_stdin(reader, Arc::clone(&closed))
        .expect("the watcher thread must spawn")
        .join()
        .expect("the watcher thread must not panic");
    closed.load(Ordering::Acquire)
}

#[test]
fn the_watcher_reports_an_empty_stdin_as_closed() {
    assert!(watch_to_end(std::io::empty()));
}

/// A reader that records, when it reaches end of file, how many bytes the
/// watcher had already consumed and whether the flag was still clear.
struct RecordingReader {
    inner: Cursor<&'static [u8]>,
    closed: Arc<AtomicBool>,
    closed_before_end: Arc<AtomicBool>,
}

impl Read for RecordingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        if count == 0 && self.closed.load(Ordering::Acquire) {
            self.closed_before_end.store(true, Ordering::Release);
        }
        Ok(count)
    }
}

#[test]
fn the_watcher_discards_bytes_and_reports_closed_only_at_end_of_file() {
    let closed = Arc::new(AtomicBool::new(false));
    let closed_before_end = Arc::new(AtomicBool::new(false));
    let reader = RecordingReader {
        inner: Cursor::new(b"stop\nquit\n"),
        closed: Arc::clone(&closed),
        closed_before_end: Arc::clone(&closed_before_end),
    };
    watch_stdin(reader, Arc::clone(&closed))
        .expect("the watcher thread must spawn")
        .join()
        .expect("the watcher thread must not panic");

    assert!(closed.load(Ordering::Acquire));
    assert!(
        !closed_before_end.load(Ordering::Acquire),
        "bytes on stdin must not be treated as a close"
    );
}

/// A reader whose reads fail with the given error kind until `failures` is spent,
/// then report end of file.
struct FailingReader {
    kind: ErrorKind,
    failures: usize,
    reads: Arc<AtomicUsize>,
}

impl Read for FailingReader {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        if self.failures == 0 {
            return Ok(0);
        }
        self.failures -= 1;
        Err(std::io::Error::from(self.kind))
    }
}

#[test]
fn the_watcher_treats_a_read_error_as_closed() {
    let reads = Arc::new(AtomicUsize::new(0));
    let closed = watch_to_end(FailingReader {
        kind: ErrorKind::Other,
        failures: usize::MAX,
        reads: Arc::clone(&reads),
    });
    assert!(closed);
    assert_eq!(
        reads.load(Ordering::Acquire),
        1,
        "a read error must end the watch"
    );
}

#[test]
fn the_watcher_retries_an_interrupted_read() {
    let reads = Arc::new(AtomicUsize::new(0));
    let closed = watch_to_end(FailingReader {
        kind: ErrorKind::Interrupted,
        failures: 1,
        reads: Arc::clone(&reads),
    });
    assert!(closed);
    assert_eq!(reads.load(Ordering::Acquire), 2);
}

/// A reader that records the name of the thread reading it, then reports end of file.
struct NameReader(Arc<Mutex<Option<String>>>);

impl Read for NameReader {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        *self.0.lock().expect("name lock") = std::thread::current().name().map(str::to_owned);
        Ok(0)
    }
}

#[test]
fn the_watcher_thread_is_named() {
    let name = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&name);

    assert!(watch_to_end(NameReader(recorded)));
    assert_eq!(
        name.lock().expect("name lock").as_deref(),
        Some(STDIN_WATCHER_THREAD_NAME)
    );
}

/// A writer whose writes fail with the given error kind.
struct BrokenWriter(ErrorKind);

impl Write for BrokenWriter {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(self.0))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::from(self.0))
    }
}

#[test]
fn a_write_to_a_closed_pipe_is_recognised() {
    let error = write_event(
        &mut BrokenWriter(ErrorKind::BrokenPipe),
        &Event::Shutdown {
            reason: ShutdownReason::StdinClosed,
        },
    )
    .expect_err("the write must fail");
    assert!(is_broken_pipe(&error), "got: {error:#}");
}

#[test]
fn any_other_write_failure_is_not_a_closed_pipe() {
    let error = write_event(
        &mut BrokenWriter(ErrorKind::Other),
        &Event::Shutdown {
            reason: ShutdownReason::Signal,
        },
    )
    .expect_err("the write must fail");
    assert!(!is_broken_pipe(&error), "got: {error:#}");
}
