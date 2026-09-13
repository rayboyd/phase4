# Typed MIDI device errors in headless mode

Branch: `feature/midi-device-errors`

## Summary

A headless run that cannot open its MIDI device writes an `error` event with a stable code, in the same way audio device failures already do. Today `connect_midi_device` builds untyped `anyhow` errors, so `Event::from_anyhow` reports `Unknown` and a host cannot tell a missing MIDI device from any other failure.

MIDI device resolution gains a `MidiDeviceError` enum with three variants. Their names are the new event codes.

| Code | Written when |
| --- | --- |
| `MidiUnavailable` | The MIDI backend cannot be initialised |
| `MidiNoMatch` | No MIDI input port matches the requested name |
| `MidiConnectFailed` | A port matched but could not be opened |

Startup behaviour is unchanged. A MIDI device that cannot be opened still stops the engine before any worker starts. Only the reported code and message change. `--midi-list` is unchanged.

## Step 0. Confirm the current code

Stop and report if any of these differ.

- `src/managers/midi.rs` imports `use anyhow::{anyhow, Context, Result};`.
- `connect_midi_device` in `src/managers/midi.rs` calls `midir::MidiInput::new("phase4").context("Failed to initialise MIDI input")?`, then `find_matching_midi_device(...).with_context(|| format!("No MIDI input device matching '{name_query}' found. Run with --midi-list to see available devices."))?`, then `.map_err(|error| anyhow!("Failed to connect to MIDI device '{port_name}': {error}"))?`. That is the only `anyhow!` use in the file.
- `enumerate_devices` in `src/managers/midi.rs` uses `.context("Failed to initialise MIDI input")?` and stays as it is.
- `src/managers/mod.rs` declares `pub mod midi;`.
- `Event::from_anyhow` in `src/headless.rs` downcasts to `AppConfigError`, then `DeviceError`, then falls back to `"Unknown"`.
- `src/headless.rs` implements `EventCode` for `AppConfigError` and `DeviceError`.
- `DeviceError` in `src/managers/audio.rs` derives `Debug` and `thiserror::Error`.
- `tests/headless.rs` contains `device_error_codes_are_the_variant_names` and `a_hardware_stream_error_reports_its_code_and_text`.
- The unit test `connect_midi_device_fails_for_an_unmatched_name` in `src/managers/midi.rs` only asserts `result.is_err()`.

## Step 1. Tests first

Add to `tests/headless.rs`, importing `use phase4::managers::midi::MidiDeviceError;`.

```rust
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
```

Replace `connect_midi_device_fails_for_an_unmatched_name` in `src/managers/midi.rs` with this test.

```rust
#[test]
fn connect_midi_device_reports_a_typed_error_for_an_unmatched_name() {
    let Err(error) = connect_midi_device(
        "a-name-no-real-device-will-ever-have",
        Arc::new(AppState::new()),
    ) else {
        panic!("an unmatched MIDI device name must fail");
    };
    // A machine without a MIDI backend cannot enumerate ports, which is also a typed failure.
    assert!(
        matches!(
            error.downcast_ref::<MidiDeviceError>(),
            Some(MidiDeviceError::MidiNoMatch { .. } | MidiDeviceError::MidiUnavailable { .. })
        ),
        "got: {error:#}"
    );
}
```

## Step 2. Add the error type

Add to `src/managers/midi.rs`, above `MidiInputSource`.

```rust
/// Typed MIDI device failures. Variant names are headless event codes.
///
/// Message texts are part of the user-facing, not machine-read, surface.
#[derive(Debug, thiserror::Error)]
pub enum MidiDeviceError {
    /// The MIDI backend could not be initialised.
    #[error("MIDI input could not be initialised: {message}")]
    MidiUnavailable { message: String },

    /// No MIDI input port matched the query, exactly or as a substring.
    #[error("No MIDI input device matched \"{query}\". Run with --midi-list to see available devices.")]
    MidiNoMatch { query: String },

    /// A port matched the query but could not be opened.
    #[error("Failed to connect to MIDI device \"{device}\": {message}")]
    MidiConnectFailed { device: String, message: String },
}
```

## Step 3. Raise it from `connect_midi_device`

The signature of `connect_midi_device` is unchanged. Its three failure points become typed.

```rust
let midi_in = midir::MidiInput::new("phase4").map_err(|error| MidiDeviceError::MidiUnavailable {
    message: error.to_string(),
})?;

let ports = midi_in.ports();
let port = find_matching_midi_device(ports, name_query, |port| midi_in.port_name(port).ok())
    .ok_or_else(|| MidiDeviceError::MidiNoMatch {
        query: name_query.to_owned(),
    })?;
```

```rust
    .map_err(|error| MidiDeviceError::MidiConnectFailed {
        device: port_name.clone(),
        message: error.to_string(),
    })?;
```

`anyhow!` is no longer used in the file, so the import becomes `use anyhow::{Context, Result};`.

## Step 4. Map the codes

In `src/headless.rs`, import `use crate::managers::midi::MidiDeviceError;` and add the implementation below the `DeviceError` one.

```rust
impl EventCode for MidiDeviceError {
    fn event_code(&self) -> &'static str {
        match self {
            Self::MidiUnavailable { .. } => "MidiUnavailable",
            Self::MidiNoMatch { .. } => "MidiNoMatch",
            Self::MidiConnectFailed { .. } => "MidiConnectFailed",
        }
    }
}
```

Extend the chain in `Event::from_anyhow`, after the `DeviceError` downcast.

```rust
.or_else(|| {
    error
        .downcast_ref::<MidiDeviceError>()
        .map(EventCode::event_code)
})
```

## Step 5. Documentation

- `docs/headless.md`, in the `error` section, after the sentence listing example codes, add: "MIDI device failures report `MidiUnavailable`, `MidiNoMatch` or `MidiConnectFailed`."
- `docs/midi.md`, after "A real MIDI device is opened during startup, if the selected device disappears or cannot be opened, Phase4 exits before starting any workers.", add: "Under `--headless` the error event carries `MidiUnavailable`, `MidiNoMatch` or `MidiConnectFailed`, see [Headless](headless.md#error)."

## Public contract changes

- Three new headless error codes. No existing code is renamed or removed.
- `phase4::managers::midi::MidiDeviceError` is a new public type.
- Error messages for MIDI device failures change wording. They are user-facing text, not a machine contract.

## Done criteria

- [ ] `MidiDeviceError::event_code` returns `MidiUnavailable`, `MidiNoMatch` and `MidiConnectFailed` for its three variants.
- [ ] `Event::from_anyhow` on an `anyhow::Error` built from `MidiNoMatch` reports code `MidiNoMatch` and a message containing the query.
- [ ] `connect_midi_device` with an unmatched name returns an error that downcasts to `MidiNoMatch`, or to `MidiUnavailable` on a machine without a MIDI backend.
- [ ] Running `phase4 --headless --test-sweep 0.1 --ws-addr 127.0.0.1:0 --midi-device "no such device"` writes exactly one stdout line, an `error` event with code `MidiNoMatch`, and exits non-zero.
- [ ] The same run without `--midi-device` still writes a `ready` event.
- [ ] `phase4 --midi-list --midi-list-format json` output is unchanged.
- [ ] `docs/headless.md` and `docs/midi.md` name the three codes.
- [ ] `cargo clippy --all-targets` reports no new warnings, including no unused import in `src/managers/midi.rs`.

## Commit message

```text
feat(headless): report typed codes for MIDI device failures

A headless run that cannot open its MIDI device now writes an error
event with MidiUnavailable, MidiNoMatch or MidiConnectFailed instead of
Unknown, so a host can tell the user which device to fix.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```
