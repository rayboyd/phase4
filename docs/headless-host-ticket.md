# Phase4 headless host lifetime

Branch: `feature/headless-host-lifetime`

## Summary

A headless run stops when its host closes stdin, so the engine cannot outlive the process supervising it. The operating system closes the pipe however the host ends, SIGKILL and crashes included.

A hardware stream error during a headless run is reported as an `error` event with the code `HardwareStreamError` and a non-zero exit. It is currently reported as a `shutdown` with reason `signal` and exit 0.

The data plane is untouched. The event schema stays at `"v": 1`, because both changes are additive.

## Resolved design

Every headless run watches stdin. There is no flag. A host keeps stdin piped and open for as long as it wants the engine running. A headless run started with stdin at `/dev/null` reads end of file at once, writes `ready`, then shuts down with reason `stdin_closed`.

Bytes arriving on stdin are read and discarded. stdin is a lifetime signal, not a control channel, and no input is ever interpreted.

The watcher is a detached thread blocked on a read. A blocking read on stdin cannot be interrupted portably, so the thread is not registered with `WorkerThreads` and ends with the process.

`ShutdownReason` gains `StdinClosed`, serialised as `"stdin_closed"`. `"signal"` is unchanged.

The wait loop checks, in order, a signal, then a closed stdin, then `keep_running` cleared by the engine itself. The first match decides the outcome, so a signal arriving in the same poll as a closed stdin reports `signal`.

`keep_running` cleared by the engine means the run failed. The only runtime site that does this outside test modules is the hardware stream error callback in `src/managers/audio.rs`. That callback records the first error's text on `AppState` before clearing `keep_running`, so the loop always finds the message. The error callback is not the realtime data callback, so allocating the message there is permitted. If the loop finds `keep_running` cleared with no recorded error, it returns an untyped error, reported with the code `Unknown`.

After a clean drain, a broken pipe while writing the `shutdown` event is not a failure. A host that has exited no longer reads stdout, and the process exits 0.

## Step 0. Confirm the current code

Confirm each site before changing it. All were read at `7287b58` on `main`.

- `src/main.rs`. `run_headless` at line 82 writes `ready`, calls `app.run_headless_until_shutdown()?` at line 90, then writes `Event::Shutdown { reason: ShutdownReason::Signal }` at lines 92 through 97 whatever stopped the run. `main` at line 14 writes an `error` event from `Event::from_anyhow` and returns `ExitCode::FAILURE` on any `Err`.
- `src/app.rs`. `HEADLESS_POLL_RATE_MS` at line 31. `AppState` at line 34 holds three atomics and is built only through `Default` at line 52 and `new` at line 62. Every construction in `src/controller.rs`, `src/bootstrap.rs` and `src/app.rs` calls `AppState::new()`. `run_headless` at line 194 registers SIGINT and SIGTERM and loops `while keep_running`, returning `Ok(())` however the loop ends. `run_headless_until_shutdown` at line 220 returns `Result<()>`. The test module starts at line 272.
- `src/managers/audio.rs`. `DeviceError` at line 31 with variants `EmptyQuery`, `NoMatch` and `UnsupportedFormat`. `start_stream` clones `error_state` at line 442. The error callback at lines 457 through 460 logs `Hardware Stream Error: {err}` and clears `keep_running`.
- `src/headless.rs`. `ShutdownReason` at line 67 uses `#[serde(rename_all = "lowercase")]`, which would serialise `StdinClosed` as `stdinclosed`. `impl EventCode for DeviceError` at line 194 is the only exhaustive match over `DeviceError`.
- `tests/headless_cli.rs`. `spawn` at line 17 sets `.stdin(Stdio::null())`. Under this change every test using `spawn` would shut down immediately, so `spawn` must pipe stdin. `a_headless_startup_failure_reports_a_typed_code` at line 131 keeps `Stdio::null()`, since it fails before the watcher starts.
- `tests/headless.rs`. `device_error_codes_are_the_variant_names` at line 122.
- `docs/headless.md`, `docs/lifecycle.md` lines 17 and 158, and the `## Headless` section of `README.md`.

The other `keep_running.store(false, ...)` sites in `src/managers/server.rs`, `src/managers/midi.rs` and `src/worker.rs` are inside test modules. `src/bootstrap.rs` line 173 runs during construction, before `ready`. Confirm this before relying on the stream error callback being the only runtime site.

## New public surface

`src/headless.rs`:

```rust
/// Name of the thread that watches stdin for end of file.
pub const STDIN_WATCHER_THREAD_NAME: &str = "headless-stdin";

/// Size of the buffer the stdin watcher reads into. Every byte read is discarded.
const STDIN_WATCH_BUFFER_BYTES: usize = 1024;

/// Why the engine stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    /// SIGINT or SIGTERM was received.
    Signal,

    /// stdin reached end of file, because the host closed it or exited.
    StdinClosed,
}

/// What ended a headless run, decided once per poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessStop {
    /// A shutdown was requested and the run drains cleanly.
    Requested(ShutdownReason),

    /// The engine cleared `keep_running` itself, so the run failed.
    Engine,
}

/// Decides whether a headless run stops. A signal takes precedence over a
/// closed stdin, and both take precedence over a stop raised by the engine.
#[must_use]
pub fn headless_stop(
    signal_received: bool,
    stdin_closed: bool,
    keep_running: bool,
) -> Option<HeadlessStop> {
    if signal_received {
        Some(HeadlessStop::Requested(ShutdownReason::Signal))
    } else if stdin_closed {
        Some(HeadlessStop::Requested(ShutdownReason::StdinClosed))
    } else if keep_running {
        None
    } else {
        Some(HeadlessStop::Engine)
    }
}

/// Spawns a detached thread that reads `reader` until end of file or a read
/// error, discarding every byte, then sets `closed`. An interrupted read is
/// retried.
///
/// # Errors
///
/// Returns an error if the thread cannot be spawned.
pub fn watch_stdin(
    mut reader: impl Read + Send + 'static,
    closed: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(STDIN_WATCHER_THREAD_NAME.to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; STDIN_WATCH_BUFFER_BYTES];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => {
                        log::warn!("Treating stdin as closed after a read error: {error}");
                        break;
                    }
                }
            }
            closed.store(true, Ordering::Release);
        })
}

/// Reports whether an event write failed because the reader of stdout has gone.
#[must_use]
pub fn is_broken_pipe(error: &anyhow::Error) -> bool {
    let kind = error
        .downcast_ref::<std::io::Error>()
        .map(std::io::Error::kind)
        .or_else(|| {
            error
                .downcast_ref::<serde_json::Error>()
                .and_then(serde_json::Error::io_error_kind)
        });
    kind == Some(ErrorKind::BrokenPipe)
}
```

`EventCode for DeviceError` gains `Self::HardwareStreamError { .. } => "HardwareStreamError"`.

`src/managers/audio.rs`, on `DeviceError`:

```rust
/// The input stream failed while the engine was running.
#[error("The audio input stream failed: {message}. Check the device is still connected.")]
HardwareStreamError { message: String },
```

`src/app.rs`:

```rust
/// Message for a run the engine stopped without recording a reason.
const ENGINE_STOPPED_MESSAGE: &str = "The engine stopped without a shutdown request.";
```

`AppState` gains a field, initialised to `Mutex::new(None)` in `Default`:

```rust
/// The first hardware stream error of the run. Written by the stream error
/// callback before it clears `keep_running`, and read by the headless loop.
pub hardware_stream_error: Mutex<Option<String>>,
```

and two methods:

```rust
/// Records the first hardware stream error, then requests shutdown. A later
/// error does not replace the first.
pub fn fail_hardware_stream(&self, message: String) {
    self.hardware_stream_error
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert(message);
    self.keep_running.store(false, Ordering::Release);
}

/// Takes the recorded hardware stream error, if one was recorded.
#[must_use]
pub fn take_hardware_stream_error(&self) -> Option<String> {
    self.hardware_stream_error
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}
```

The `AppState` doc comment no longer calls it atomic flags only. It states the one mutex and that no realtime path takes it.

## Signature changes other code depends on

- `App::run_headless_until_shutdown` returns `Result<ShutdownReason>` instead of `Result<()>`. The only caller is `src/main.rs`.
- `ShutdownReason` serialises with `snake_case` instead of `lowercase`. The existing `"signal"` value is unchanged.
- `DeviceError` gains `HardwareStreamError`. The only exhaustive match is `EventCode for DeviceError` in `src/headless.rs`.
- `AppState` gains a non-atomic field. Every construction goes through `AppState::new()`, so no literal needs updating.

## Behaviour

`run_headless` registers SIGINT and SIGTERM as now, starts `watch_stdin(std::io::stdin(), ...)` with context `Failed to start the stdin watcher`, logs `Ready. Send SIGINT or SIGTERM, or close stdin, to exit.`, then polls `headless_stop` every `HEADLESS_POLL_RATE_MS`.

- `Requested(reason)` clears `keep_running` and returns `Ok(reason)`.
- `Engine` returns `Err(DeviceError::HardwareStreamError { message })` when `take_hardware_stream_error` yields a message, otherwise `Err(anyhow!(ENGINE_STOPPED_MESSAGE))`.

`run_headless_until_shutdown` still calls `shutdown()` on every path before returning.

The stream error callback in `start_stream` keeps its `log::error!` line and replaces the `keep_running` store with `error_state.fail_hardware_stream(err.to_string())`.

`src/main.rs` `run_headless` writes `Event::Shutdown { reason }` with the returned reason. A write error for which `is_broken_pipe` is true returns `Ok(())`. Any other write error is returned as now. An `Err` from the run reaches the existing error path in `main`, which writes the `error` event and exits non-zero.

Interactive runs are unchanged. The controller never reads `hardware_stream_error`, and the stream error still stops an interactive run as it does now.

## Steps

1. Extend `tests/headless.rs`.
   - `StdinClosed` serialises as exactly `{"v":1,"event":"shutdown","reason":"stdin_closed"}\n`, and add it to `every_line_carries_the_schema_version_and_one_trailing_newline`.
   - Add `HardwareStreamError` to `device_error_codes_are_the_variant_names`.
   - `headless_stop` returns `None` while running, `Requested(Signal)` for a signal, `Requested(StdinClosed)` for a closed stdin, `Engine` for cleared `keep_running`, `Requested(Signal)` when all three are set, and `Requested(StdinClosed)` when stdin is closed and `keep_running` is cleared.
   - `watch_stdin` over `std::io::empty()` sets `closed` once joined. Over `Cursor::new(b"stop\n")` it sets `closed` only after end of file. Over a reader that returns `ErrorKind::Other` it sets `closed`. Over a reader that returns `ErrorKind::Interrupted` once, then `Ok(0)`, it sets `closed` after exactly two reads. The spawned thread is named `STDIN_WATCHER_THREAD_NAME`.
   - `is_broken_pipe` is true for `write_event` into a writer whose writes fail with `ErrorKind::BrokenPipe`, and false for one failing with `ErrorKind::Other`.
   - `Event::from_anyhow` over `DeviceError::HardwareStreamError` reports the code `HardwareStreamError` and a message containing the recorded text.
2. Add to the `src/app.rs` test module. `fail_hardware_stream` clears `keep_running` and `take_hardware_stream_error` returns its message. A second call keeps the first message. A second take returns `None`.
3. Extend `tests/headless_cli.rs`. Change `spawn` to `Stdio::piped()` for stdin. The returned `Child` owns the pipe, so the existing tests keep stdin open and still report `signal`. Add these tests.
   - `closing_stdin_shuts_a_headless_run_down`. Read `ready`, drop `child.stdin.take()`, read `shutdown` with reason `stdin_closed`, exit 0, nothing further on stdout.
   - `bytes_on_stdin_are_ignored`. Read `ready`, write and flush `b"stop\nquit\n"`, sleep `STDIN_SETTLE` (a named constant of 250 ms), send SIGTERM, read `shutdown` with reason `signal`.
   - `a_host_that_goes_away_leaves_no_engine_running`. Read `ready`, drop the stdout reader, drop stdin, and assert the process exits 0 within `EXIT_TIMEOUT`.
   - `a_headless_run_with_null_stdin_stops_at_once`. Spawn with `Stdio::null()` stdin, read `ready`, read `shutdown` with reason `stdin_closed`, exit 0.
4. Update `ShutdownReason`, add `HeadlessStop`, `headless_stop`, `watch_stdin`, `is_broken_pipe` and the new `EventCode` arm in `src/headless.rs`.
5. Add `DeviceError::HardwareStreamError` in `src/managers/audio.rs` and route the stream error callback through `fail_hardware_stream`.
6. Add the `AppState` field and methods, rewrite `run_headless` and change `run_headless_until_shutdown` in `src/app.rs`.
7. Update `src/main.rs` to write the returned reason and ignore a broken pipe on that write.
8. Update `docs/headless.md`.
   - Streams: stdin is watched for end of file and every byte on it is discarded.
   - `shutdown`: a table of reasons, `signal` and `stdin_closed`, and that a host treats an unrecognised reason as a clean stop.
   - `error`: add `HardwareStreamError` to the listed codes, written when the input stream fails during a run, for example when the interface is unplugged.
   - Rename `## Signals and Exit Codes` to `## Stopping and Exit Codes`. Cover SIGINT, SIGTERM and stdin end of file, that null stdin stops the run at once, that a stream error exits non-zero, and that a broken pipe on the final event still exits 0.
   - Supervising Phase4: step 1 spawns with stdin piped and held open, step 4 stops the engine with SIGTERM or by closing stdin, and a new line states that a host which exits for any reason closes the pipe and the engine drains.
9. Update `docs/lifecycle.md` line 17 and the line 158 transition so closing stdin is listed beside SIGINT and SIGTERM for headless. Add one sentence to the `## Headless` section of `README.md` stating that the engine stops when its host closes stdin.
10. Run the full check suite, including the debug and release denormal and filter-bank runs the roadmap's accepted limitation requires.

## Done criteria

- A headless run whose host holds stdin open runs until signalled, and reports `signal`.
- Closing stdin drains the workers, writes one `shutdown` event with reason `stdin_closed` and exits 0.
- A host killed with SIGKILL leaves no phase4 process behind, and the engine exits 0 even though nothing reads its final event.
- Bytes written to stdin have no effect on the run.
- Headless with stdin at `/dev/null` writes `ready`, then `shutdown` with reason `stdin_closed`, and exits 0.
- A hardware stream error during a headless run drains the workers, writes one `error` event with code `HardwareStreamError` and the stream's error text, and exits non-zero.
- A signal and a closed stdin in the same poll report `signal`.
- `"signal"` serialises exactly as before and every line still carries `"v":1`.
- Interactive runs are unchanged: raw mode, `Ctrl+C`, and a stream error stopping the run.
- The WebSocket and OSC payloads, addresses and options are unchanged.

## Commit message

```text
feat(headless): stop with the host and report stream errors

Watch stdin in every headless run and drain when it reaches end of file,
so the engine stops however its host ends, a crash or SIGKILL included.
Bytes on stdin are discarded. The shutdown event reports stdin_closed,
and a broken pipe on that final write still exits zero.

A hardware stream error during a headless run now writes an error event
with the code HardwareStreamError and exits non-zero, instead of a clean
shutdown attributed to a signal.

The event schema stays at version 1. Interactive runs and both output
transports are unchanged.
```
