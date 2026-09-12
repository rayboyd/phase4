//! Serialisation checks for the headless event stream.
//!
//! The emitted lines are a public contract. A host parses them to supervise the
//! engine, so field order, names and the schema version are asserted exactly.

use phase4::config::AppConfigError;
use phase4::headless::{
    write_event, Event, EventCode, ReadyAudio, ReadyOutputs, ReadyReport, ShutdownReason,
    EVENT_SCHEMA_VERSION,
};
use phase4::managers::audio::DeviceError;

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
    let cases: [(DeviceError, &str); 3] = [
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
