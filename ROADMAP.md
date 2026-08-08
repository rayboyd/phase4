# Roadmap

## Medium

- **OSC bundle datagram ceiling** (`src/managers/osc.rs`). Each display frame is
  encoded as one OSC bundle in one UDP datagram. Supported combinations of high
  channel counts and compile-time display-bin counts can exceed the conventional
  65,507-byte IPv4 UDP payload ceiling, causing sends to fail. Revisit when OSC
  is required for those configurations. Measure the encoded bundle during
  startup, then either reject oversized configurations or define a chunked
  frame protocol without adding steady-state allocation.

## Low

- **`WorkerThreads` storage consolidation** (`src/worker.rs`). Three storage
  strategies exist for one concept, a fixed pipeline array indexed by enum
  with a manually-synced `COUNT`, a special-cased `midi_input` field, and a
  `Vec` of output workers. A single ordered `Vec<(WorkerSpec, JoinHandle)>`
  preserves shutdown order.

- **`payload.rs` mutual-exclusion cfg block** (`src/dsp/payload.rs`). The 21
  hand-written `all(...)` pairs are O(n²) in the number of display-bins
  features. Correct today and compile-time checked. Only worth restructuring
  if an 8th resolution is ever added.
