# Roadmap

## Accepted limitations

- **x86_64 denormal guard safety** (`src/managers/analyser.rs`). Retain
  `no_denormals` with [documented risk acceptance and regression coverage](docs/denormals.md).
  The Rust floating-point compiler-contract limitation remains. Run the guard
  and filter-bank tests in debug and release builds after compiler, dependency,
  DSP or build-setting changes. Native x86_64 validation precedes assurances
  to users on that architecture. Revisit the implementation if a regression
  is reproduced.

## Medium

- **OSC bundle datagram ceiling** (`src/managers/osc.rs`). Each display frame is
  encoded as one OSC bundle in one UDP datagram. High input channel counts can
  make the fixed 32-bin payload exceed the conventional 65,507-byte IPv4 UDP
  payload ceiling, causing sends to fail. Revisit when OSC is required for those
  configurations. Measure the encoded bundle during startup, then either reject
  oversized configurations or define a chunked frame protocol without adding
  steady-state allocation.

## Low

- **`WorkerThreads` storage consolidation** (`src/worker.rs`). Three storage
  strategies exist for one concept, a fixed pipeline array indexed by enum
  with a manually-synced `COUNT`, a special-cased `midi_input` field, and a
  `Vec` of output workers. A single ordered `Vec<(WorkerSpec, JoinHandle)>`
  preserves shutdown order.
