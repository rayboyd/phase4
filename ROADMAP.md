# Roadmap

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
