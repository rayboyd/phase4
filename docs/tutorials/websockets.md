# WebSocket API

The [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API) makes it possible to open a two-way interactive communication session between the browser and a server. With this API, we can receive messages without having to poll the server.

Phase4 streams real-time audio analysis data as a JSON broadcast. Any tool capable of opening a standard WebSocket connection (including browsers, Node.js, Python, or creative coding environments like TouchDesigner) can consume this stream.

## Connection Details

- **Default Address:** none, pass `--ws-addr 127.0.0.1:8889` (or set `network.ws_addr` in `config.yaml`) to enable the WebSocket output.
- **Protocol:** Standard WebSocket
- **Format:** JSON (UTF-8)

> Running Phase4 with the `--no-browser-origin` flag rejects standard browser-based connections.

The listen address can be set persistently in `config.yaml` to avoid passing it on every invocation. `--no-browser-origin` is CLI-only.

```yaml
network:
  ws_addr: "127.0.0.1:8889"
```

See [example.config.yaml](../../example.config.yaml) for the full reference.

## Data Structure

Every message is a JSON object containing a `channels` array.

```json
{
  "channels": [
    {
      "peak": 0.842,
      "bins": [0.0, 0.001, 0.012, 0.034, "..."]
    }
  ]
}
```

| Field      | Type    | Description                                                                    |
| :--------- | :------ | :----------------------------------------------------------------------------- |
| **`peak`** | `float` | The peak sample amplitude (0.0 to 1.0).                                        |
| **`bins`** | `array` | Frequency magnitudes (compile-time, default 32 bands) mapped from low to high. |

When MIDI input is configured (`--midi-device` or `--test-midi-clock`), each message may also carry a top-level `midi` object with transport and step-count data. See the [MIDI section of the README](../../README.md#midi) for the schema. When MIDI input is not configured the key is absent, so clients that only read `channels` are unaffected.

## Noise Floor

Bin values reach exactly `0.0` only when the input is digitally silent. Any device with a live analogue input stage has a noise floor, so a quiet room with nothing playing still produces small non-zero values.

On a typical audio interface that floor sits between -90 and -100 dBFS, roughly `0.00001` to `0.00003` in the linear values Phase4 sends. Convert a bin value to dBFS with `20 * log10(value)`.

The floor is not flat across the bins:

- The lowest bins read highest, from 1/f noise and any DC offset in the input stage.
- Localised bumps can appear at harmonics of the mains frequency (50 Hz or 60 Hz), from transformer or switch-mode supply coupling.
- The highest bins rise gently. Each filter has a fixed Q, so its bandwidth grows with its centre frequency and it integrates proportionally more broadband noise.

This is hardware behaviour, not an artefact of the analysis. Phase4 passes digital silence through as exactly `0.0`.

### Handling It

The floor sits far below anything you would deliberately play, so consumers that map bin values to a fixed visual range can ignore it.

It matters as soon as anything downstream normalises, auto-gains, or divides by a running maximum. During silence that running maximum collapses to the noise floor, every bin normalises to near full scale, and the output reacts violently to an empty stage.

Apply an absolute floor before any normalisation step. Subtracting a fixed amount and clamping at zero keeps the response continuous, which matters if you then smooth or differentiate the result.

```js
const floored = Math.max(bin - FLOOR, 0.0);
```

A floor of `0.0001` (-80 dBFS) is a reasonable starting point, comfortably above a typical noise floor and far below any real signal.
To measure your own, run Phase4 with the input connected exactly as you intend to use it and with nothing playing, then watch the stream for a few seconds and note the largest bin value that appears. Set the floor two to three times that figure. Both the preamp gain and whatever is plugged into the input change the result, so a floor measured at one gain setting does not transfer to another.

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
      // Apply a gentle perceptual scale to compensate for high-frequency bin energy drop-off.
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
