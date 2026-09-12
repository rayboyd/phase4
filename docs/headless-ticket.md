# Phase4 headless mode

Branch: `feature/headless-mode`

## Summary

Phase4 gains a supervised, non-interactive run mode. `--headless` runs the engine without a
terminal and emits a newline-delimited JSON event stream on stdout so a host process can know when
the engine is ready, what it resolved, and why it stopped.

This is a control plane. The data plane is untouched: WebSocket and OSC behave exactly as they do
now and a host connects to them as any other client.

## Resolved design

`--headless` is a CLI-only presence flag. It is never read from `config.yaml`, for the reason
already documented there for `--no-browser-origin`: a presence flag has no explicitly-off form, so
offering it in the file would break the rule that a CLI flag always overrides a file value.

Headless keeps every existing startup requirement. A headless run with no output configured still
fails with `NoOutputConfigured`. Headless changes how the engine is supervised, not what it
demands.

Three events only: `ready`, `error`, `shutdown`. Device-change, device-loss and buffer-overflow
reporting are out of scope and are not added by this change.

stdout carries the event stream and nothing else, matching the discipline `--audio-list-format
json` already promises. Logs stay on stderr. The `\r` line ending is a raw-mode artefact and is not
emitted in headless.

Every event carries `"v": 1`. Phase4's other JSON output is unversioned, but a host must be able
to detect the contract it is supervising against, and this stream is the only place that matters.

## Step 0. Confirm the current code

Confirm each site before changing it. All were read at `870a5f1` on `main`.

- `src/main.rs`. The whole binary is 46 lines. Line 10 defines
  `TERMINAL_LOG_LINE_ENDING: &str = "\r"`, used in the `env_logger` format closure at lines 15
  through 24. `env_logger` writes to stderr. Lines 27 through 35 handle `--audio-list` and
  `--midi-list` and return early. Lines 38 through 40 are the gate this change replaces:

  ```rust
  if !std::io::stdin().is_terminal() {
      anyhow::bail!("Phase4 requires an interactive terminal. Run it directly from a terminal.");
  }
  ```

  Lines 42 through 44 build `AppConfig::try_from(&args)`, construct `App::new(&config)` and call
  `app.run_until_shutdown()`.
- `src/app.rs`. `App` at line 62 already holds `ws_bound_addr: Option<SocketAddr>` and exposes
  `ws_bound_addr()` at line 119. `App::new` at line 100 calls `bootstrap(config)` and constructs
  `Controller::new(controller_state)`. `run()` at line 128 is `self.controller.run()`.
  `run_until_shutdown()` at line 141 and `shutdown()` at line 151. `Drop` at line 170.
- `src/controller.rs`. `Controller::run` calls `enable_raw_mode()` at line 36, `Drop` calls
  `disable_raw_mode()` at line 66, `handle_panic` at line 75 calls `disable_raw_mode()`, and
  `install_panic_hook` is at line 88.
- `src/bootstrap.rs`. Line 83 `resolve_audio_hardware(config, &mut input_device)` returns
  `resolved`, whose `hw_specs` and `analyse_channels` are used at lines 84 through 95. Confirm the
  `Bootstrapped` struct's exact fields and whether `hw_specs` and the resolved device name survive
  into it. They must, for the `ready` event.
- `src/managers/audio.rs`. `DeviceError` at line 31 with variants `EmptyQuery`, `NoMatch`,
  `UnsupportedFormat`. `Specs { channels: u16, sample_rate: u32 }` follows it.
- `src/config/types.rs`. `AppConfigError` at line 162, variants including `MissingDevice`,
  `NoOutputConfigured`, `NonLoopbackBindAddress` and the vocoder validation set. `AppConfig` at
  line 270 with `outputs`, `input`, `midi_input`, `vocoder_config`.
- `src/lib.rs`. `Args` and its `#[arg]` groups, `NetworkArgs` around line 100.
- `docs/lifecycle.md`. The startup flowchart states the interactive-terminal branch and the
  `NoOutputConfigured` branch. Both need updating.
- `docs/config.md` and `example.config.yaml` state the CLI-only rule for `--no-browser-origin`.

Enumerate every variant of `AppConfigError` and `DeviceError` in step 0 and write the list into
the event-code mapping. Do not invent codes.

## New public surface

Add `src/headless.rs`.

```rust
use std::net::SocketAddr;

/// Schema version for the headless event stream.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// The audio input the engine actually resolved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReadyAudio {
    /// Resolved device name, or `None` in calibration mode.
    pub device: Option<String>,
    /// Hardware sample rate in Hz.
    pub sample_rate: u32,
    /// Analysed channel indices in output order.
    pub channels: Vec<u16>,
}

/// The transports the engine actually bound.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReadyOutputs {
    /// The WebSocket listener's bound address, resolving a `:0` port.
    pub websocket: Option<SocketAddr>,
    /// The configured OSC target address.
    pub osc: Option<SocketAddr>,
}

/// Facts the host cannot know until the engine has started.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReadyReport {
    pub audio: ReadyAudio,
    /// Resolved MIDI device name when MIDI input is enabled.
    pub midi_device: Option<String>,
    pub outputs: ReadyOutputs,
}

/// Why the engine stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShutdownReason {
    /// SIGINT or SIGTERM.
    Signal,
}

/// One line of the headless event stream.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    Ready(ReadyReport),
    Error { code: String, message: String },
    Shutdown { reason: ShutdownReason },
}

/// Writes one event as a single JSON line, then flushes.
///
/// # Errors
///
/// Returns an error if serialisation or the write fails.
pub fn write_event(writer: &mut impl std::io::Write, event: &Event) -> anyhow::Result<()>;

/// The stable code for a typed error, for the `error` event's `code` field.
pub trait EventCode {
    fn event_code(&self) -> &'static str;
}
```

`Event` serialises with `"v"` first, then `"event"`, then the variant's fields flattened. Achieve
that with an explicit `Serialize` implementation or a wrapper struct. The tagged enum above
defines the shape, not the literal derive output. The emitted lines are exactly:

```json
{"v":1,"event":"ready","audio":{"device":"Duet 3","sample_rate":48000,"channels":[0,1]},"midi_device":null,"outputs":{"websocket":"127.0.0.1:53412","osc":null}}
{"v":1,"event":"error","code":"NoOutputConfigured","message":"To start the app, configure at least one output: --ws-addr <ADDR> or --osc-addr <ADDR>"}
{"v":1,"event":"shutdown","reason":"signal"}
```

Implement `EventCode` for `AppConfigError` and `DeviceError`, returning the variant name verbatim
as a stable identifier. Adding a variant adds a code. Renaming one is a breaking change to this
contract and must be treated as such.

### Additions to existing types

`src/lib.rs`, on the top-level `Args`:

```rust
/// Run without a terminal and emit a JSON event stream on stdout.
/// Intended for a supervising host process. Logs stay on stderr.
#[arg(long)]
pub headless: bool,
```

`src/app.rs`, on `App`:

```rust
/// The facts resolved during construction, for the headless `ready` event.
#[must_use]
pub fn ready_report(&self) -> ReadyReport;

/// Runs until a shutdown signal arrives, without a terminal or key handling.
///
/// # Errors
///
/// Returns an error if signal registration fails.
pub fn run_headless_until_shutdown(&mut self) -> anyhow::Result<()>;
```

`App` gains the fields `ready_report` needs. Source them from `bootstrap`, which already resolves
them. Do not re-query the device.

`Controller` is not constructed in headless. Either make `App::controller` an `Option<Controller>`
or split construction, whichever keeps `Drop` and `shutdown()` idempotent. `install_panic_hook`
gains a headless form that does not call `disable_raw_mode` and writes an `error` event with code
`Panic` before exiting non-zero.

## Behaviour

`--headless` skips the interactive-terminal check. Without `--headless` that check stays exactly
as it is, including its message.

Startup order in headless: parse args, initialise logging without the `\r` suffix, resolve config,
construct `App`. On success write `ready` and block in `run_headless_until_shutdown`. On failure
write one `error` event carrying the typed code and the error's existing `Display` text, then exit
non-zero. `--audio-list` and `--midi-list` continue to return before any of this.

SIGINT and SIGTERM both request shutdown, drain workers in the existing registration order, write
`shutdown` with reason `signal`, and exit 0.

Exit codes: 0 for a signalled shutdown, non-zero for any startup or runtime failure. A failure
always writes an `error` event before exiting.

## Steps

1. Write `tests/headless.rs`. Cover: `Event` serialisation for all three variants against the
   exact expected lines above, `"v"` present on every line, `EventCode` returning the variant name
   for every `AppConfigError` and `DeviceError` variant, and `write_event` emitting exactly one
   newline-terminated line per call with no trailing output.
2. Write `src/headless.rs` and its types.
3. Add `headless` to `Args`. Confirm it does not collide with existing flags.
4. Add `ready_report()` and `run_headless_until_shutdown()` to `App`, plumbing the resolved device
   name, sample rate and channel indices out of `bootstrap`.
5. Make `Controller` construction conditional and add the headless panic hook.
6. Rewrite `src/main.rs` to branch on `args.headless` before the terminal check, choose the log
   format, and route startup failures through an `error` event.
7. Add an integration test that spawns the built binary with `--headless --ws-addr 127.0.0.1:0`
   and a calibration signal so it needs no hardware, reads the `ready` line from stdout, asserts
   the reported WebSocket port is non-zero and connectable, sends SIGTERM, asserts the `shutdown`
   line and exit code 0, and asserts stdout contained nothing but those two lines.
8. Add a failing-startup integration test: `--headless` with no output configured writes one
   `error` event with code `NoOutputConfigured` and exits non-zero.
9. Add `docs/headless.md` describing the flag, the stream, each event, the code list, stdout and
   stderr discipline, signals and exit codes. Update `docs/lifecycle.md` so the startup flowchart
   shows the headless branch. Update `README.md`. State in `docs/config.md` and
   `example.config.yaml` that `--headless` is CLI-only for the same reason as
   `--no-browser-origin`.
10. Run the full check suite, including the debug and release denormal and filter-bank runs the
    roadmap's accepted limitation requires.

## Done criteria

- `--headless` starts the engine with stdin redirected from `/dev/null` and no controlling
  terminal.
- Without `--headless`, a non-interactive stdin still fails with the existing message.
- The first stdout line in headless is one `ready` event carrying the resolved device name, sample
  rate, analysed channel indices, MIDI device when enabled, and bound transport addresses.
- `--ws-addr 127.0.0.1:0` reports a non-zero bound port in `ready` and that port accepts a client.
- stdout in headless contains only event lines. Every log line goes to stderr, and no log line
  carries a `\r`.
- SIGTERM and SIGINT both drain workers in registration order, write one `shutdown` event and exit
  0.
- Every startup failure writes one `error` event with the typed code and exits non-zero.
- A panic in headless writes an `error` event with code `Panic` and does not attempt to restore a
  terminal.
- Interactive behaviour is unchanged: raw mode, key handling, the `\r` log ending and the
  `Ctrl+C` path all behave as before when `--headless` is absent.
- The WebSocket and OSC payloads, addresses and options are unchanged.

## Commit message

```text
feat(headless): add a headless mode with a JSON event stream

Run without a terminal under --headless and emit newline-delimited JSON
on stdout so a supervising host can know when the engine is ready, what
it resolved and why it stopped. The ready event reports the resolved
audio device, sample rate and analysed channels, the MIDI device when
enabled, and the actually bound transport addresses, so a host can pass
a zero port and be told the real one.

Errors carry stable codes taken from the existing typed error variants.
SIGINT and SIGTERM both drain workers in registration order. Logs stay
on stderr without the raw-mode line ending, and stdout carries the event
stream alone.

Interactive behaviour, both output transports and every payload are
unchanged.
```
