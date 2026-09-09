# Denormal handling

Phase4 retains `no_denormals` around the analyser's processing loop. Suppressing
subnormal arithmetic is an intentional requirement for predictable DSP
processing. The guard is local to the analyser thread and restores the previous
floating-point environment when its scope ends.

## Accepted limitation

The [package documentation](https://docs.rs/no_denormals/0.3.0/no_denormals/)
describes the processor controls and explicitly documents undefined behaviour
under Rust's compiler rules. On x86_64, changing the floating-point environment
around ordinary Rust code conflicts with
[Rust's documented requirements](https://doc.rust-lang.org/core/arch/x86_64/fn._mm_setcsr.html).
Restoring the environment afterwards does not resolve that compiler contract.

Phase4 accepts this limitation and retains the existing guard with behavioural
regression coverage. No resulting compiler failure has been reproduced in the
project's review. Passing tests establish observed behaviour for the tested
build and execution environment. They do not prove the absence of undefined
behaviour or guarantee future compiler behaviour.

## Regression coverage

[`tests/denormals.rs`](../tests/denormals.rs) checks the following behaviour.

- Positive and negative subnormal results are flushed to signed zero inside
  the guard.
- Subnormal operands are treated as zero even when the same multiplication
  outside the guard produces a normal result.
- Ordinary subnormal arithmetic returns after leaving the guard, including
  nested guards and unwinding in the test harness.
- The actual vocoder filter bank produces zero bins for subnormal input from
  a reset state and still responds to an ordinary input sample.
- A tone at a band centre produces its strongest response in that band. A
  subsequent twenty seconds of simulated silence leaves negligible output.
  The exposed bins remain finite, non-negative and normal or zero at every
  checked chunk boundary.

The arithmetic probes use runtime operands through `black_box` and inspect
result bits. `black_box` discourages constant folding. It does not repair the
compiler contract. These tests require ordinary subnormal behaviour outside
the guard and fail if that baseline is unavailable.

The filter tests exercise `VocoderAnalyser` under the same dependency guard.
They do not start an audio device or the production worker thread. They inspect
exposed bins, not every intermediate calculation or private biquad state.
The decay assertion permits negligible normal residues. It does not promise
that flushing alone makes every recursive state reach exact zero.

These are functional tests, not latency benchmarks. They do not establish
callback deadlines, CPU usage or audio-device reliability. Release tests use
Cargo's test harness, which unwinds panics even though the application release
profile uses `panic = "abort"`.

## Running the checks

Run both builds after compiler, dependency, DSP or build-setting changes.

```sh
rustc -Vv
cargo test --locked --test denormals -- --nocapture
cargo test --locked --release --test denormals -- --nocapture
```

The deliberate panic printed by `guard_restores_behaviour_after_unwinding` is
caught by that test. The final test result must still report success.

Run the same commands with Rust 1.88 when verifying the declared minimum
compiler version. The existing full test command includes these tests in
debug builds. CI also runs this test target in release mode.

Record the commit, compiler output, target architecture, operating system,
build flags, test results and whether execution was native, virtualised or
emulated. A successful run applies to that combination.

M4 testing supports the maintainer's own usage. VM or emulation results are
useful additional observations. Run the tests and an audio soak test on a
native x86_64 machine before providing assurances to users on that
architecture. Keep those results separate from M4 results.

An unexpected result requires investigation of the failing build before
release. Any replacement for the guard must preserve denormal suppression,
DSP behaviour and predictable processing time.
