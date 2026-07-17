# OSC Output

[Open Sound Control](https://opensoundcontrol.stanford.edu/) (OSC) is a lightweight message protocol carried over UDP. Phase4 can send real-time analysis data as OSC float messages to any UDP target, alongside or instead of the WebSocket broadcast.

## Enabling OSC Output

Pass `--osc-addr` with a `host:port` target when starting Phase4.

```sh
./phase4 --audio-device "Duet 3" --osc-addr 127.0.0.1:7000
```

To avoid passing the flag on every invocation, set `osc_addr` in `config.yaml` instead.

```yaml
network:
  osc_addr: "127.0.0.1:7000"
```

Phase4 binds an ephemeral local UDP port and sends to the specified target. OSC output shares the same rate-limit gate as the WebSocket broadcast, so `--broadcast-rate` applies to both.

## Address Scheme

Each frequency bin is represented by an OSC message. The address pattern is:

```
/phase4/ch/{channel}/bin/{bin}
```

- `{channel}` is zero-based. A stereo device produces channels `0` and `1`.
- `{bin}` is zero-based, ordered from lowest to highest frequency.
- The argument is a single `f` (32-bit float) in the range `0.0` to `1.0`.

All `channels * DISPLAY_BINS` bin messages for a frame are sent together as a single OSC bundle (`#bundle` header, immediate time tag) in one UDP packet, rather than one packet per bin.

All OSC message structures (addresses and argument slots) are built once before the send loop, as the content of a single persistent bundle. On each frame, only the float value is updated in place, then the whole bundle is encoded and sent as one UDP packet. The encoded bytes are written into a reused buffer, so the send loop performs no heap allocation in steady state.

## Message Reference

| Address                  | Type | Range      | Description                                |
| :----------------------- | :--- | :--------- | :----------------------------------------- |
| `/phase4/ch/{n}/bin/{n}` | `f`  | 0.0 to 1.0 | Frequency bin magnitude for channel `{n}`. |

The bin count is set at compile time. The default is 32 bins. See [compile.md](compile.md) for how to change it.

## MIDI Address Scheme

When MIDI input is configured (`--midi-device` or `--test-midi-clock`), the OSC sender also transmits four additional addresses alongside the bin data:

| Address                 | Type | Value | Description                                                                |
| :----------------------- | :--- | :---- | :--------------------------------------------------------------------------- |
| `/phase4/midi/steps`     | `i`  | count | Absolute MIDI 1/16 note steps since the most recent Start. Sent every frame. |
| `/phase4/midi/start`     | `i`  | `1`   | Sent only on the frame a Start transport event fired.                        |
| `/phase4/midi/stop`      | `i`  | `1`   | Sent only on the frame a Stop transport event fired.                         |
| `/phase4/midi/continue`  | `i`  | `1`   | Sent only on the frame a Continue transport event fired.                     |

`/phase4/midi/steps` behaves like the bin addresses: it is sent every frame, and clients detect new steps by comparing the current value to the previous frame. The three transport addresses instead follow an event model: each carries a conventional bang value (`1`) and is only sent on the frame its event actually happened, so an OSC In CHOP channel bound to `/phase4/midi/start` only receives a message when playback starts.

When MIDI input is not configured, none of these four addresses are ever sent.

## TouchDesigner Integration

Add an OSC In DAT to your network and set its Network Port to match the port given in `--osc-addr`. OSC In DAT unpacks OSC bundles, so all `/phase4/ch/{channel}/bin/{bin}` messages for a frame arrive together from the single UDP packet Phase4 sends. OSC In CHOP does not unpack bundles, so it receives none of the bin data; only the individually sent `/phase4/midi/*` messages would arrive there. Use OSC In DAT instead.

If you are receiving a large number of messages, check your receiving application for options to buffer or queue bursts of incoming messages rather than dropping them under load.

Phase4 fires and forgets each UDP packet. There is no connection handshake, acknowledgement, or backpressure. If the target is not running or unreachable, packets are silently dropped.

Bin messages for a frame are combined into one OSC bundle per UDP packet. At the default build (stereo, 32 bins, 64 bin messages), the encoded bundle runs roughly 2 to 2.5KB, over standard Ethernet's 1500 byte MTU. That's not an issue on loopback, whose MTU is far larger, but it raises IP fragmentation risk if `--osc-addr` targets a non-loopback destination.

## Notes

- OSC output is disabled by default. Omitting `--osc-addr` adds no overhead to the pipeline.
- The UDP socket is bound eagerly at startup. If the bind fails, Phase4 exits with an error before spawning any threads.
- The OSC sender runs on a dedicated background thread with its own single-threaded Tokio runtime, matching the pattern of the WebSocket server.
