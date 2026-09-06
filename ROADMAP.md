# Roadmap

## High

- **x86_64 denormal guard safety** (`src/managers/analyser.rs`). The
  `no_denormals` guard changes MXCSR modes around ordinary Rust code, which
  conflicts with Rust's documented floating-point environment requirements.
  Passing tests and stable resource use do not resolve that compiler contract.
  Review the denormal strategy before claiming x86_64 safety.

## Medium

- **Selected-channel alignment after pause** (`src/managers/analyser.rs`).
  Selected-channel capture publishes samples individually. A paused drain can
  end within a frame, discard the alignment bookkeeping, and resume with a
  remaining sample assigned to the wrong channel. A controlled interleaving
  reproduces this with finite samples within full scale. It affects selections
  of two or more channels, not the all-channel path or single-channel selection.
  Preserve alignment across discard and resume before treating that path as
  correct under every producer/consumer interleaving.

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
