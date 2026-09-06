# WebSocket API

The [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API) makes it possible to open a two-way interactive communication session between the browser and a server. With this API, we can receive messages without having to poll the server.

Phase4 streams real-time audio analysis data as a one-way JSON broadcast. Any tool capable of opening a standard WebSocket connection (including browsers, Node.js, Python, or creative coding environments like TouchDesigner) can consume this stream.

## Connection Details

- **Default Address:** none, pass `--ws-addr 127.0.0.1:8889` (or set `network.ws_addr` in `config.yaml`) to enable the WebSocket output.
- **Protocol:** Standard WebSocket
- **Format:** JSON (UTF-8)
- **Rate:** A 60 Hz publication target while the engine is active. Delivery is not guaranteed at exactly 60 messages per second.
- **Direction:** Server to client only. Phase4 services Ping, Pong, and Close control frames, but rejects Text and Binary messages by closing the connection. Inbound application data is never passed into Phase4's application pipeline.

The listen address must be a loopback IP address and port. Remote and wildcard listen addresses are rejected. The default client limit is eight, configured with `--max-clients` or `network.max_clients`. Handshakes must complete within one second.

A client receives the current snapshot after its handshake, then the latest available updates. Slow consumers can skip intermediate snapshots. A snapshot can repeat the same analysis values when no newer analysis is available. Pausing stops regular publications but keeps connections and control-frame handling active. A newly connected client can still receive the retained snapshot while paused.

Every write or flush must complete within one second, including the initial snapshot and control-frame replies. Phase4 disconnects clients that exceed this deadline and releases their connection slots. A timed-out connection is dropped without waiting for a close handshake. Healthy clients continue receiving updates independently.

> `--no-browser-origin` rejects any handshake containing an `Origin` header, including native clients that send one. Standard browser WebSocket connections include this header. The flag is not authentication.

The listen address can be set persistently in `config.yaml` to avoid passing it on every invocation. `--no-browser-origin` is CLI-only.

```yaml
network:
  ws_addr: "127.0.0.1:8889"
```

See [example.config.yaml](../example.config.yaml) for the full reference.

## Data Structure

Every message is a JSON object containing a `channels` array.

```json
{
  "channels": [
    {
      "peak": 0.842,
      "bins": [
        0.001, 0.002, 0.003, 0.004, 0.005, 0.006, 0.007, 0.008,
        0.009, 0.010, 0.011, 0.012, 0.013, 0.014, 0.015, 0.016,
        0.017, 0.018, 0.019, 0.020, 0.021, 0.022, 0.023, 0.024,
        0.025, 0.026, 0.027, 0.028, 0.029, 0.030, 0.031, 0.032
      ]
    }
  ]
}
```

| Field      | Type    | Description                                                        |
| :--------- | :------ | :----------------------------------------------------------------- |
| **`peak`** | `float` | Absolute sample peak over the latest analysis chunk, normally `0.0` to `1.0` for hardware input. Not clamped.                    |
| **`bins`** | `array` | Exactly 32 non-negative, unnormalised `f32` frequency envelopes, ordered from low to high. Values can exceed `1.0`. |

Phase4 uses `f32` for audio samples, DSP state, peaks, and bins. JSON does not encode a float width, so JavaScript parses these values as `Number`. Copying them into a `Float32Array` recovers the original `f32` values for direct WebGL use.

After the first mapper publication, messages also carry a top-level `midi` object when MIDI input is configured (`--midi-device` or `--test-midi-clock`). A connection made before that publication can receive the initial zeroed channel snapshot without `midi`. See [MIDI](midi.md#websocket-schema) for the schema. When MIDI input is not configured the key is absent, so clients that only read `channels` are unaffected.

The `channels` array follows the selected hardware channels in ascending order. Its indices are output positions, so selecting hardware channels `1,3` produces array entries `0,1`. Hardware indices and band centre frequencies are not included in the payload. See [configuration](config.md#input-and-output-selection).

Each analysis chunk contains up to approximately 10 ms of audio. Peaks measure that chunk, while bins report the envelope state at its end. The mapper selects the latest chunk rather than accumulating peaks across the whole broadcast interval. A short transient in a skipped chunk may therefore be absent from the broadcast peak.

Frames containing any non-finite peak or bin are rejected before JSON encoding. The server retains the last valid snapshot but does not publish a replacement for each rejected frame. Existing clients receive no update until a valid frame arrives. This output check does not reset the DSP state.

## Noise Floor

A live analogue input can produce non-zero bins with nothing playing. The level and shape depend on the input, preamp gain, connected equipment and the configured filter bank. Phase4 does not apply a noise gate or subtract a measured floor.

Bins are rectified and smoothed filter outputs. Their values include filter gain and bandwidth, so an interface's broadband noise specification does not directly predict each bin's floor. For a positive bin, `20 * Math.log10(bin)` gives decibels relative to an envelope value of one. It is not a calibrated broadband input dBFS measurement.

Digital silence produces zero bins from reset filter and envelope state. After a signal stops, stored filter energy and release smoothing produce a decaying tail. Exact zero is not an immediate-silence guarantee.

### Handling It

Consumers can ignore the floor when it produces no visible movement at their chosen scale.

It matters as soon as anything downstream normalises, auto-gains, or divides by a running maximum. During silence that running maximum collapses to the noise floor, small noise values can become large visual movements.

Apply an absolute floor before any normalisation step. Subtracting a fixed amount and clamping at zero keeps the response continuous, which matters if you then smooth or differentiate the result.

```js
const floored = Math.max(bin - FLOOR, 0.0);
```

Choose `FLOOR` from measurements of your actual setup. A value of `0.0001` is -80 dB relative to one, but is only a trial value and may suppress quiet signals or fail to remove noise. To measure your own, run Phase4 with the input connected exactly as you intend to use it and with nothing playing, then watch the stream for a few seconds and note the largest bin value that appears. Set the floor two to three times that figure. Both the preamp gain and whatever is plugged into the input change the result, so a floor measured at one gain setting does not transfer to another.

## JavaScript Example

Copy this into a `.html` file, or check this [CodePen example](https://codepen.io/rayboyd/full/wBzOPPr) to see the data in action. No dependencies required.

```html
<canvas id="viz" width="800" height="300" style="background:#111;"></canvas>

<script>
  const canvas = document.getElementById("viz");
  const ctx = canvas.getContext("2d");
  const ws = new WebSocket("ws://127.0.0.1:8889");

  ws.onmessage = (event) => {
    const { channels } = JSON.parse(event.data);
    if (!channels?.length) return;

    const bins = channels[0].bins;
    const barWidth = canvas.width / bins.length;

    ctx.clearRect(0, 0, canvas.width, canvas.height);

    bins.forEach((val, i) => {
      // This visual weighting emphasises higher bins. It is not DSP calibration.
      const scale = 1 + i * 0.05;
      const barHeight = val * canvas.height * scale;
      ctx.fillStyle = `hsl(${(i / bins.length) * 360}, 80%, 60%)`;
      ctx.fillRect(
        i * barWidth,
        canvas.height - barHeight,
        barWidth - 1,
        barHeight,
      );
    });
  };
</script>
```
