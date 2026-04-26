# WebSocket API

The [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API) makes it possible to open a two-way interactive communication session between the browser and a server. With this API, we can receive messages without having to poll the server.

Phase4 streams real-time audio analysis data as a JSON broadcast. Any tool capable of opening a standard WebSocket connection (including browsers, Node.js, Python, or creative coding environments like TouchDesigner) can consume this stream.

## Connection Details

- **Default Address:** `ws://127.0.0.1:8889`
- **Protocol:** Standard WebSocket
- **Format:** JSON (UTF-8)

> **Note:** If you run Phase4 with the `--no-browser-origin` flag, standard browser-based connections will be rejected.

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
| **`bins`** | `array` | Frequency magnitudes (compile-time, default 64 bands) mapped from low to high. |

## JavaScript Example

Copy this into a `.html` file to see the data in action. No dependencies required.

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
