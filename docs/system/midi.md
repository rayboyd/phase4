# MIDI Transport Design Decision

Status: accepted, PSE4-01, not yet implemented

## Context

Phase4 already bridges one external signal, audio, into a broadcast consumers can subscribe to with no protocol knowledge of their own. MIDI transport (Start, Stop, Continue, and clock-derived step position) is the same shape of problem, a local hardware or virtual source phase4 listens to and reports outward, exactly like the audio device it already reads.

Motivating use case: a clock-driven visual layer (a row of rectangles stepping on 1/16 subdivisions, structural and orderly) composited in TouchDesigner against phase4's existing audio-reactive layer (expressive and chaotic), both fed from one phase4 process, one entry point in the TD file. This also covers a hand-built TD step sequencer UI, which becomes just another subscriber to the same broadcast, no separate feature needed for that case.

Explicitly out of scope: full MIDI clock fidelity (that is TD's MIDI In CHOP, if ever needed) and sequencing (patterns, swing, probability, authoring). Phase4 listens and reports. It does not become a sequencer.

This is treated as a first-class feature, not a bolt-on. It gets the same design rigour, testing discipline, and documentation treatment as any other transport phase4 ships.

## Module layout

A new sibling to src/dsp/, not an addition to it:

```text
src/dsp/      audio DSP: vocoder, envelope, RawPayload, DisplayPayload
src/midi/     MIDI logic: byte parsing, tick accumulator, MidiEvent
src/managers/ IO and threading for both: mapper, generator, osc,
              server, and the new MIDI manager
```

src/dsp/ means audio DSP output specifically. MIDI parsing is not DSP, and folding it in under a shared payload name would erode the one thing that module boundary currently guarantees, that its name tells you what is inside without checking. The managers layer already separates producers (generator.rs, and the new MIDI manager) from transports (server.rs, osc.rs), and both existing and new types slot into that same split.

## The event type

MidiEvent, not MidiPayload. DisplayPayload is a state snapshot, safe to resend every frame. A MIDI transport message is a discrete occurrence, Start happened, step 4 fired, and naming it a state would misdescribe it.

Variants: Start, Stop, Continue, StepFired(u32).

StepFired carries an absolute count, N ticks-worth of steps since the last Start, not a count wrapped to 16. Phase4 has no notion of a meaningful position of its own, there is nothing to track beyond a running count, position is entirely a downstream, implementation defined idea. Absolute is the finer signal and the recoverable one, a consumer wanting a 16-step wheel takes N & 15 for free, and coarser divisions fall out of the same counter, N >> k for a divide-by-2^k, the standard binary clock divider shape, one absolute counter feeding every division a consumer needs simultaneously. A consumer already wrapped to 16 can never recover bar-level position from it. Coarsening downstream is free, narrowing upstream is not, so phase4 emits the finest true count and leaves wrapping to whoever needs it.

## Channel choice: broadcast, not watch

RawPayload and DisplayPayload ride watch channels because they are continuous state, a subscriber that misses three frames and only sees the fourth has lost nothing that mattered, the fourth frame is a complete picture on its own. MidiEvent does not have that property. watch coalesces to the latest value, so a fast Start-Stop-Start sequence can silently drop the Stop before a slower subscriber catches up, making transport appear never to have stopped. MidiEvent uses tokio::sync::broadcast instead, same multi-producer, multi-subscriber fan-out shape, but no value is silently replaced.

## Input resolution

Mirrors the pattern ConfigInput established for audio (Calibration versus Device), applied independently to MIDI rather than folded into the same enum. Audio input and MIDI input are not mutually exclusive the way calibration and a real device are, running real audio hardware against a synthetic MIDI clock while building a TD patch with no MIDI interface plugged in is a real combination, so they need separate resolution, not a shared type.

Working shape: a MIDI-specific enum resolving to either a synthetic test clock (a configured BPM) or a real device by name, resolved once in resolve_config, same as audio's ConfigInput.

## The synthetic clock generator

Plays the same role TestSignal plays for audio calibration, not a mock standing in for the real logic, a generator of genuine synthetic input that runs through the same parser a real MIDI source would. It emits real bytes, 0xF8 (Clock) on a schedule derived from its configured BPM, 0xFA, 0xFB, 0xFC (Start, Continue, Stop) on demand. Deliberately the same generator serving two jobs, the no-hardware dogfood path (build and demo the whole feature before any MIDI interface is involved), and the integration test driver.

## midir integration

Verified against the published API. MidiInput::connect takes a callback FnMut(u64, &[u8], &mut T) + Send, invoked by the backend's own thread (CoreMIDI on macOS, confirmed cross-platform support beyond that). This is the same shape phase4 already integrates for CPAL, a native callback on a thread phase4 does not own, handed off through a channel to a phase4-owned async worker. Not a new integration style, a second application of the one already trusted for audio.

This piece is deliberately not built first. The type shape the callback hands off is defined now so the broadcast wiring downstream does not need to change when a real midir connection replaces the synthetic generator later. Whether midir batches more than one MIDI message into a single callback invocation is unverified and needs checking against midir's own examples before this piece is implemented. Real-Time messages can legally arrive interleaved inside another message's bytes, so this would affect the parser if callbacks are not strictly one message each, though the parser's slice-based API (process_bytes(&[u8])) handles either case correctly since it scans every byte independently regardless of how many arrive per callback.

## Step computation

MIDI clock is 24 ticks per quarter note by spec. Resolution is fixed at a 1/16 note, six ticks per step (24 / 4), not configurable. This matches the standard step-sequencer grid, including Ray's own hand-built TD UI, and any coarser subdivision TD needs, 1/8, 1/4, half notes, is a trivial bit shift and mask on the receiving end (N >> k, & 15), no reason to make phase4 configurable for something a two-line counter handles downstream. Triplets and swing were considered and explicitly excluded, swing in particular is a timing offset applied to alternate steps, not a grid resolution, and the stated use case (visual sync) does not need it.

Phase4 computes the step boundary itself, with privileged, unthrottled access to the raw incoming clock, before any broadcast throttling touches it, and emits a discrete StepFired(n) event rather than forwarding raw ticks. Forwarding raw Clock was rejected, jitter from broadcast throttling would smear the interval between ticks, which is the entire informational content of Clock. Discrete step events do not have this problem, they are one-shot transitions, not a continuous signal, and are fine on the existing broadcast rate at realistic tempos, confirmed by direct calculation: at 120 bpm, 1/16 notes are 125 ms apart, and even a 60 Hz broadcast's worst-case 16.7 ms jitter is under 15 percent of that, well under anything perceptible on a visual layer.

## Transport semantics: Stop does not imply reset

MIDI defines three distinct transport messages specifically because Stop is not reset and wait. Start begins playback from position zero. Stop halts the clock but the transmitting device retains its position. Continue resumes from that retained position, not from zero. If Stop meant reset, Continue would be redundant, it would just be Start again. The existence of Continue as its own message is the proof that Stop preserves position.

This also means phase4's own tick counter needs no special pause logic. Clock ticks only arrive on the wire while the transport is running, a transmitting device stops sending 0xF8 the moment it stops and resumes only after Continue, so the counter simply stops advancing because nothing drives it, and resumes for the same reason. Silence on the wire is the pause, nothing needs to be explicitly remembered or restored on phase4's side.

Some modular and hardware sequencer conventions treat a Stop as an implicit reset to zero regardless, this is a real and common choice, but it is a convention layered on top of MIDI, not part of the spec. Phase4 reports the true event, Stop, and never synthesises a position reset on its own behalf. A consumer that wants Stop means zero gets that by resetting its own local counter on receiving Stop, the same listen-and-report dividing line as everything else here. Baking the reset into phase4 would make a real pause-and-resume mid-sequence impossible to represent, since Continue would have nothing left to resume from.

## Test strategy

Two-tier, mirroring the existing split between direct unit tests on dsp/payload.rs and the integration coverage in tests/app_calibration.rs.

Direct tests feed the parser and tick accumulator exact byte sequences and assert the exact MidiEvent sequence produced, for example six ticks at the fixed 1/16 resolution firing exactly one StepFired. Fast, deterministic, no generator involved, so a scheduling bug in the generator can never make these tests lie about whether the arithmetic itself is correct.

Generator-driven tests prove the wiring, synthetic generator through parser through broadcast channel through to a subscriber, the same role tests/app_calibration.rs plays for audio today.

## Wire format

A new message shape, not merged into DisplayPayload's JSON. OSC follows the existing /phase4/ch/{channel}/bin/{bin} convention, likely something under /phase4/midi/. Exact addresses and the WebSocket JSON shape are open questions below.

## CLI and config surface

--midi-device <name>, mirroring --device. No subdivision flag, resolution is fixed at 1/16, see Step computation above. Opt-in like every other transport this cycle, absent entirely, phase4 behaves exactly as it does today.

## Alternatives considered

Reusing the existing DisplayPayload watch channel for MIDI events: rejected, watch channels are correct specifically because audio state is safe to coalesce, and MIDI transport events are not.

Naming the shared type MidiPayload and placing it in dsp/payload.rs: rejected, dsp means audio DSP output, and MIDI parsing is not DSP.

Extending ConfigInput or TestSignal directly to cover MIDI: rejected, audio input and MIDI input can coexist, unlike calibration and a real device, which are mutually exclusive by design.

A trait abstraction over MIDI sources, built before a real midir integration exists to justify its shape: rejected as premature, Input is not behind a trait for audio either. Build the synthetic path for real now, add the midir-backed path when hardware testing is possible.

A separate standalone tool rather than a phase4 feature: considered at length, rejected. Phase4's scaffolding (spawn_async_worker, the bounded shutdown pattern, watch and broadcast fan-out, opt-in transports) is already proven and directly reusable, a second tool would either duplicate it or require premature abstraction into a shared crate with only one real consumer to validate the abstraction against. src/midi/ alongside src/dsp/, and a MIDI manager alongside osc.rs and server.rs, reuses the proven shape directly with no new architecture.

A configurable subdivision flag: rejected, the only real downstream need is coarsening a fixed fine grid, which is free on the consuming side via bit shifting, not a reason to add a flag and a default to argue about.

A composite reset to zero event (Stop, Song Position Pointer 0, Continue) as a single convenience: rejected, phase4 reports the real Stop event and lets the consumer decide what it means, per the transport semantics section above.

## Costs accepted

Two channel primitives in use across the codebase, watch for continuous state, broadcast for discrete events, rather than one uniform mechanism everywhere. Deliberate, because the underlying semantics genuinely differ.

No real MIDI device support yet, only the synthetic generator. This is an explicit, tracked gap, not a silently skipped one.

## Open questions

Exact OSC address scheme and WebSocket JSON shape. Whether midir batches multiple messages per callback needs checking against midir's own examples, not just its published API, before the midir-integration piece is built.

## Consequences

New module src/midi/, new src/managers/ file for MIDI IO, a new broadcast channel wired into both server.rs and osc.rs alongside the existing watch channel, new CLI and config surface, opt-in like every other transport. Nothing about the existing audio path changes, ConfigInput, DisplayPayload, and the audio watch channel are untouched.

## Implementation order

Four pieces, deliberately separable:

1. Pure parser and tick accumulator in src/midi/, with direct unit tests. No generator, no midir, no async. Buildable and fully testable in isolation.
2. The synthetic clock generator, emitting real bytes through the same parser, serving as both the no-hardware dogfood path and the integration test driver.
3. Wire integration, the broadcast channel and its WS and OSC output. This is the first piece with anything user-observable, a --midi-device flag, new WS message shape, new OSC addresses. This is also the piece where docs/system/lifecycle.md gains a new conditional spawn box (mirroring the existing OSC conditional spawn), the README gains the new flag, and a proper docs/tutorials/midi.md tutorial earns its place, the same role calibration.md plays. Per standing project rule, these doc updates are steps within this piece's own instruction, not a separate follow-up to be requested.
4. midir integration, replacing the synthetic generator with a real device connection. The only piece that needs an actual MIDI source to test against, and the only piece with an open verification question against the crate itself.

## Done criteria

- docs/system/midi.md exists, states its status as accepted but unimplemented, and covers every section above.
- No code changed.
