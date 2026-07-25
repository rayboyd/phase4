# Roadmap

## Med

One coherent pass over error classification, agreed after the 0.0.12
stdout-events review.

1. **Type the device-resolution errors.** `map_startup_error` in
   `src/events.rs` classifies device failures by the substring marker
   `"--audio-list"` because `Input::get_device` returns untyped `anyhow!`
   errors. Give those failures a real error type, delete the marker, and make
   the reserved `FatalReason::DeviceUnsupported` slot real (splitting
   not-found vs unsupported-format, which the messages already distinguish
   but the types don't).

2. **`Emitter::emit` EPIPE panic** (`src/events.rs`). If the wrapper dies and
   closes the read end before `ready` fires, the child aborts on EPIPE
   instead of exiting on stdin EOF. Narrow window and practically irrelevant
   (the wrapper is already gone), but a graceful degrade belongs in the same
   pass.

## Low

- **`WorkerThreads` storage consolidation** (`src/worker.rs`). Three storage
  strategies for one concept: a fixed pipeline array indexed by enum with a
  manually-synced `COUNT`, a special-cased `midi_input` field, and a `Vec`
  of output workers. A single ordered `Vec<(WorkerSpec, JoinHandle)>`
  preserves shutdown order. Maintainability only — do it when next touching
  shutdown or adding an output transport, not before.

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
  timing-based startup detection); depends on the typed-errors pass above
  for `device_unsupported`.
- Default to `--ws-addr 127.0.0.1:0` and read the bound port from the
  `ready` event, eliminating the port race.
- Release plan: signed artefact downloads, gh-pages style.
