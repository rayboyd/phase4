# OSC Output

[Open Sound Control](https://opensoundcontrol.stanford.edu/) (OSC) is a lightweight message protocol carried over UDP. Phase4 can send real-time analysis data as OSC float messages to any UDP target, alongside or instead of the WebSocket broadcast.

## Enabling OSC Output

Pass `--osc-addr` with a `host:port` target when starting Phase4.

```sh
./phase4 --device 0 --osc-addr 127.0.0.1:7000
```

Phase4 binds an ephemeral local UDP port and sends to the specified target. OSC output shares the same rate-limit gate as the WebSocket broadcast, so `--broadcast-rate` applies to both.

## Address Scheme

Each frequency bin is sent as a separate OSC message. The address pattern is:

```
/phase4/ch/{channel}/bin/{bin}
```

- `{channel}` is zero-based. A stereo device produces channels `0` and `1`.
- `{bin}` is zero-based, ordered from lowest to highest frequency.
- The argument is a single `f` (32-bit float) in the range `0.0` to `1.0`.

All address strings are pre-built before the send loop, so there is no per-frame heap allocation.

## Message Reference

| Address                  | Type | Range      | Description                                |
| :----------------------- | :--- | :--------- | :----------------------------------------- |
| `/phase4/ch/{n}/bin/{n}` | `f`  | 0.0 to 1.0 | Frequency bin magnitude for channel `{n}`. |

The bin count is set at compile time. The default is 32 bins. See [compile.md](compile.md) for how to change it.

## Resolume Integration

In Resolume Avenue or Arena, open the OSC input configuration and add a new OSC shortcut. Set the input address to match the Phase4 address pattern, for example `/phase4/ch/0/bin/0`, and map it to the target parameter. Resolume's OSC shortcut editor accepts literal address strings, so copy the address exactly as shown.

Phase4 fires and forgets each UDP packet. There is no connection handshake, acknowledgement, or backpressure. If the target is not running or unreachable, packets are silently dropped.

## Notes

- OSC output is disabled by default. Omitting `--osc` adds no overhead to the pipeline.
- The UDP socket is bound eagerly at startup. If the bind fails, Phase4 exits with an error before spawning any threads.
- The OSC sender runs on a dedicated background thread with its own single-threaded Tokio runtime, matching the pattern of the WebSocket server.
