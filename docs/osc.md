# OSC Output

[Open Sound Control](https://opensoundcontrol.stanford.edu/) (OSC) is a lightweight message protocol carried over UDP. Phase4 can send real-time analysis data as OSC float messages to any UDP target, alongside or instead of the WebSocket broadcast.

## Enabling OSC Output

Pass `--osc-addr` with a `host:port` target when starting Phase4.

```sh
./phase4 --audio-device "Duet 3" --osc-addr 127.0.0.1:7000
```

IPv6 targets use bracketed address notation.

```sh
./phase4 --audio-device "Duet 3" --osc-addr '[::1]:7000'
```

To avoid passing the flag on every invocation, set `osc_addr` in `config.yaml` instead.

```yaml
network:
  osc_addr: "127.0.0.1:7000"
```

Phase4 binds an ephemeral local UDP port using the target's IPv4 or IPv6 address family, then sends the latest analysis snapshot at 60 Hz. OSC and WebSocket share the same fixed output cadence.

## Address Scheme

Each frequency bin is represented by an OSC message. The address pattern is:

```
/phase4/ch/{channel}/bin/{bin}
```

- `{channel}` is zero-based. A stereo device produces channels `0` and `1`.
- `{bin}` is zero-based, ordered from lowest to highest frequency.
- The argument is a single `f` (`f32`) in the range `0.0` to `1.0`.

All `channels * 32` bin messages for a frame are sent together as a single OSC bundle (`#bundle` header, immediate time tag) in one UDP packet, rather than one packet per bin.

All OSC message structures (addresses and argument slots) are built once before the send loop, as the content of a single persistent bundle. On each frame, only the float value is updated in place, then the whole bundle is encoded and sent as one UDP packet. The encoded bytes are written into a reused buffer, so the send loop performs no heap allocation in steady state.

## Message Reference

| Address                  | Type | Range      | Description                                |
| :----------------------- | :--- | :--------- | :----------------------------------------- |
| `/phase4/ch/{n}/bin/{n}` | `f`  | 0.0 to 1.0 | Frequency bin magnitude for channel `{n}`. |

Every channel always carries 32 bins.

MIDI transport and clock data travel their own OSC addresses when MIDI input is configured, see [MIDI](midi.md#osc-addresses).

## Noise Floor

Bin values reach exactly `0.0` only when the input is digitally silent. Any device with a live analogue input stage has a noise floor, so with nothing playing the bins still carry small non-zero values, typically between -90 and -100 dBFS (roughly `0.00001` to `0.00003`).

This is hardware behaviour, not an artefact of the analysis. Patches that map bin values to a fixed range can ignore it. It matters as soon as anything normalises or auto-gains, because during silence the running maximum collapses to the noise floor and every bin normalises to near full scale.

Apply an absolute floor before any normalisation, starting around `0.0001` (-80 dBFS). See [Noise Floor](websockets.md#noise-floor) in the WebSocket API documentation for the full explanation and the reasoning behind that value.

## TouchDesigner Integration

Add an [OSC In DAT](https://derivative.ca/UserGuide/OSC_In_DAT) to your network and set its Network Port to match the port given in `--osc-addr`. OSC In DAT unpacks OSC bundles, so all `/phase4/ch/{channel}/bin/{bin}` messages for a frame arrive together from the single UDP packet Phase4 sends. OSC In CHOP does not unpack bundles, so it receives none of the bin data; only the individually sent `/phase4/midi/*` messages would arrive there. Use OSC In DAT instead.

If you are receiving a large number of messages, check your receiving application for options to buffer or queue bursts of incoming messages rather than dropping them under load.

Phase4 fires and forgets each UDP packet. There is no connection handshake, acknowledgement, or backpressure. If the target is not running or unreachable, packets are silently dropped.

Bin messages for a frame are combined into one OSC bundle per UDP packet. For stereo input, 32 bins per channel produce 64 bin messages and an encoded bundle of roughly 2 to 2.5KB, over standard Ethernet's 1500 byte MTU. That's not an issue on loopback, whose MTU is far larger, but it raises IP fragmentation risk if `--osc-addr` targets a non-loopback destination.

## Notes

- OSC output is disabled by default. Omitting `--osc-addr` adds no overhead to the pipeline.
- The UDP socket is bound eagerly at startup. If the bind fails, Phase4 stops any workers already started during construction before returning the error.
- The OSC sender runs on a dedicated background thread with its own single-threaded Tokio runtime, matching the pattern of the WebSocket server.
