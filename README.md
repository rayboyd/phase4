# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight tool for broadcasting real-time audio data over WebSocket and OSC.

Any WebSocket-capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server. OSC output can be sent to any UDP target, including TouchDesigner's [OSC In DAT](https://derivative.ca/UserGuide/OSC_In_DAT).

Phase4 supports 64-bit [macOS](docs/compile.md#macos), [Windows](docs/compile.md#windows) and [Linux](docs/compile.md#linux).

## Quickstart

Pre-built binaries for macOS and Linux are on the [releases page](https://github.com/rayboyd/phase4/releases/latest). Windows users need to [compile from source](docs/compile.md).

1. [Check](#check) hardware compatibility.
2. Select a device and [serve](#serve) analysis data.
3. [Connect](#connect) a WebSocket client.

_Check the [platform requirements section](docs/compile.md#platform-requirements) of the compile guide if you intend to build Phase4 from source._

### Check

List available input devices to find your device index and confirm `f32` support.

```sh
./phase4 --audio-list
```

See [Platform Requirements](docs/compile.md#platform-requirements) if a device doesn't show up as supported.

### Serve

Launch Phase4 using your device name (e.g., Duet 3) and a WebSocket listen address.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889
```

Press `T` to toggle the engine's active state.

```
[INFO] Audio device resolved (fuzzy match): Loopback Audio
[INFO] WebSocket server listening on ws://127.0.0.1:8889
[INFO] Ready. Press T to toggle engine, Ctrl+C to exit.
```

By default every hardware channel is analysed and broadcast. To analyse only specific channels, pass `--audio-analyse-channels` with comma-separated zero-based indices, or set `audio.analyse_channels` in `config.yaml`.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --audio-analyse-channels 0,1
```

Calibration mode drives the full pipeline with a synthetic sine wave. See [docs/calibration.md](docs/calibration.md).

### Connect

Phase4 streams real-time audio analysis data as a JSON broadcast. Any tool capable of opening a standard WebSocket connection (including browsers, Node.js, Python, or creative coding environments like TouchDesigner) can consume this stream. Point your WebSocket client to `ws://127.0.0.1:8889` to start receiving the data.

If Phase4 is broadcasting, check this [CodePen example](https://codepen.io/rayboyd/full/wBzOPPr) to see the server in action.

See [docs/websockets.md](docs/websockets.md) for the full data structure and noise floor handling.

## Outputs

Beyond the core WebSocket stream, Phase4 can send OSC messages to any UDP target, and attach MIDI transport and clock data to the streams you already have running.

### OSC

See [docs/osc.md](docs/osc.md)

### MIDI

See [docs/midi.md](docs/midi.md)

## Config

See [docs/config.md](docs/config.md)

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
