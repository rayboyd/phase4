# OSC Output

[Open Sound Control](https://opensoundcontrol.stanford.edu/) (OSC) is a lightweight message protocol carried over UDP. Phase4 can send real-time analysis data as OSC float messages to any UDP target, alongside or instead of the WebSocket broadcast.

## Enabling OSC Output

Pass `--osc-addr` with a `host:port` target when starting Phase4.

```sh
./phase4 --device 0 --osc-addr 127.0.0.1:7000
```

To avoid passing the flag on every invocation, set `osc_addr` in `config.yaml` instead.

```yaml
network:
  osc_addr: "127.0.0.1:7000"
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

All OSC message structures (addresses and argument slots) are built once before the send loop. On each frame, only the float value is updated in place. The encoded bytes are written into a reused buffer, so the send loop performs no heap allocation in steady state.

## Message Reference

| Address                  | Type | Range      | Description                                |
| :----------------------- | :--- | :--------- | :----------------------------------------- |
| `/phase4/ch/{n}/bin/{n}` | `f`  | 0.0 to 1.0 | Frequency bin magnitude for channel `{n}`. |

The bin count is set at compile time. The default is 32 bins. See [compile.md](compile.md) for how to change it.

## TouchDesigner Integration

Add an OSC In CHOP to your network and set its Network Port to match the port given in `--osc-addr`. Each `/phase4/ch/{channel}/bin/{bin}` message arrives as its own channel.

OSC In CHOP does not unpack OSC bundles, individual messages are required, confirmed against a real report of OSC In CHOP receiving nothing when sent a bundle. Phase4 always sends one message per bin per channel rather than bundling them, so this needs no special handling on the TouchDesigner side.

If you are receiving a large number of channels, OSC In CHOP has a Queued option with configurable target buffer sizes, useful for smoothing out bursts of incoming messages rather than dropping them under load.

Phase4 fires and forgets each UDP packet. There is no connection handshake, acknowledgement, or backpressure. If the target is not running or unreachable, packets are silently dropped.

## Notes

- OSC output is disabled by default. Omitting `--osc-addr` adds no overhead to the pipeline.
- The UDP socket is bound eagerly at startup. If the bind fails, Phase4 exits with an error before spawning any threads.
- The OSC sender runs on a dedicated background thread with its own single-threaded Tokio runtime, matching the pattern of the WebSocket server.
