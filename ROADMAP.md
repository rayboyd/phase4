# Roadmap

## Med

(Empty. The typed-errors pass — `DeviceError` replacing the `"--audio-list"`
message marker, a real `device_unsupported` fatal reason, and the
`Emitter::emit` EPIPE graceful degrade — landed on
`feature/typed-device-errors`.)

## Low

- **`WorkerThreads` storage consolidation** (`src/worker.rs`). Three storage
  strategies for one concept: a fixed pipeline array indexed by enum with a
  manually-synced `COUNT`, a special-cased `midi_input` field, and a `Vec`
  of output workers. A single ordered `Vec<(WorkerSpec, JoinHandle)>`
  preserves shutdown order.

- **`payload.rs` mutual-exclusion cfg block** (`src/dsp/payload.rs`). The 21
  hand-written `all(...)` pairs are O(n²) in the number of display-bins
  features. Correct today and compile-time checked. Only worth restructuring
  if an 8th resolution is ever added.

- **SIGTERM/SIGINT handling in headless mode.** Currently documented in
  `docs/tutorials/wrapper.md` as unsupported (stdin close is the shutdown
  path). Revisit only if a real deployment needs signal-driven shutdown,
  e.g. running under a process supervisor that can't hold a pipe.

## Related, separate repo (phase4-macos wrapper)

Tracked in the wrapper repo, listed here because they consume phase4's wire
contract:

- Event-based readiness probe using `ready` / `fatal.reason` (replaces
  timing-based startup detection); `device_unsupported` is now emitted for
  real, so this is unblocked.
- Default to `--ws-addr 127.0.0.1:0` and read the bound port from the
  `ready` event, eliminating the port race.
- Release plan: signed artefact downloads, gh-pages style.
