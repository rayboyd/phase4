# OSC Output

[Open Sound Control](https://opensoundcontrol.stanford.edu/) (OSC) is a lightweight message protocol carried over UDP. Phase4 can send real-time analysis data as OSC float messages to any UDP target, alongside or instead of the WebSocket broadcast.

## Enabling OSC Output

Pass `--osc-addr` with an `IP:port` target when starting Phase4.

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

Phase4 binds an ephemeral local UDP port using the target's IPv4 or IPv6 address family, then sends the latest available display snapshot when the mapper publishes. OSC and WebSocket share a 60 Hz publication target. The sender can skip intermediate snapshots if it falls behind.

## Address Scheme

Each frequency bin is represented by an OSC message. The address pattern is shown below.

```
/ch/{channel}/bin/{bin}
```

- `{channel}` is the zero-based output position. With all channels selected, stereo input produces channels `0` and `1`. Selecting hardware channels `1,3` also produces output channels `0,1`, in that order.
- `{bin}` is zero-based, ordered from lowest to highest frequency.
- The argument is a single `f` (`f32`) carrying a non-negative, unnormalised envelope value. Values can exceed `1.0`.

Filter Q affects both bandwidth and gain. With the default settings, a 440 Hz sine wave at `0.25` amplitude produces a strongest bin of approximately `1.14`. Consumers that need a `0.0` to `1.0` control range must apply their own scaling and clamping.

All `channels * 32` bin messages for a frame are sent together as a single OSC bundle (`#bundle` header, immediate time tag) in one UDP packet, rather than one packet per bin.

The bin bundle is built and encoded during startup, before the OSC socket or sender thread is created. Phase4 checks its actual encoded size against a supported UDP payload limit of 65,507 bytes for both IPv4 and IPv6. A bundle above that limit returns a startup error containing the audio channel count, band count, encoded byte size and limit. Select fewer audio channels or use WebSocket output for that configuration. There is no separate audio channel cap.

With the current address format and 32 bands per channel, 64 audio channels encode to 56,960 bytes. The largest fitting channel count is 73 (65,024 bytes). A 74-channel bundle encodes to 65,920 bytes and is rejected. These counts follow from the encoded size and are not hardcoded limits. The check uses the selected output channel count, not the device's total available channels. WebSocket-only output does not run this check.

The bundle and its initial encoding buffer are retained by the sender. On each frame, only the float values are updated in place, then the whole bundle is encoded and sent as one UDP packet. Changing these fixed-width float values does not change the encoded size. Successful encoding and sending of the fixed payload avoid further buffer growth in steady state. Error reporting can allocate.

## Message Reference

| Address                  | Type | Range      | Description                                |
| :----------------------- | :--- | :--------- | :----------------------------------------- |
| `/ch/{n}/bin/{n}` | `f`  | Non-negative, unnormalised | Frequency bin envelope for channel `{n}`, which can exceed `1.0`. |

Every analysed channel carries 32 bins. Peaks are not sent over OSC. The sender forwards bin floats without a non-finite-value check, unlike the WebSocket serialiser.

MIDI transport and clock data travel in separate UDP messages at their own OSC addresses when MIDI input is configured, see [MIDI](midi.md#osc-addresses).

## Noise Floor

Live analogue input can produce non-zero bins with nothing playing. Filter gain, bandwidth, envelope history and the connected hardware all affect the result. Digital silence produces zero from reset state, but an earlier signal leaves a decaying filter and envelope tail.

Normalisation can amplify the floor into visible movement. Measure the quiet-input bins at the gain settings you intend to use, then choose an absolute floor before normalisation. See [Noise Floor](websockets.md#noise-floor) for the measurement procedure and the meaning of these envelope values.

## TouchDesigner Integration

Add an [OSC In DAT](https://derivative.ca/UserGuide/OSC_In_DAT) to your network and set its Network Port to match the port given in `--osc-addr`. Configure the receiver to unpack OSC bundles and inspect the `/ch/{channel}/bin/{bin}` addresses. Each received bin bundle contains one complete frame. MIDI messages arrive separately.

If you are receiving a large number of messages, check your receiving application for options to buffer or queue bursts of incoming messages rather than dropping them under load.

UDP provides no delivery acknowledgement or receiver backpressure. Packets can be lost or reordered. Phase4 logs local encoding and send errors, but a successful send does not establish that the target received anything. The socket uses blocking sends on the OSC worker thread, so local socket pressure can delay that worker.

Bin messages for a frame are combined into one OSC bundle per UDP packet. For stereo input, 32 bins per channel produce 64 bin messages and an encoded bundle of 1,728 bytes, over standard Ethernet's 1500 byte MTU. Loopback commonly has a larger MTU. Remote paths can require fragmentation. Passing the startup payload-size check does not guarantee delivery or that a receiver can accept the bundle. Phase4 does not split frames.

## Notes

- OSC output is disabled by default. Leaving the OSC address unset in both CLI and config avoids creating an OSC socket, packet buffers or worker.
- Bundle preparation and UDP socket binding happen during startup. If either fails, Phase4 stops any workers already started during construction before returning the error.
- The OSC sender runs on a dedicated background thread with its own single-threaded Tokio runtime, matching the pattern of the WebSocket server.
