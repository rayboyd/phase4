# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight tool for broadcasting real-time audio analysis and midi data over WebSocket and OSC.

Any WebSocket-capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server. OSC output can be sent to any UDP target, including TouchDesigner's [OSC In DAT](https://derivative.ca/UserGuide/OSC_In_DAT).

## Download

Phase4 supports 64-bit [macOS](docs/compile.md#macos), [Windows](docs/compile.md#windows) and [Linux](docs/compile.md#linux).

Pre-built binaries for macOS and Linux are on the [releases page](https://github.com/rayboyd/phase4/releases/latest). Windows users need to [compile from source](docs/compile.md). Check the [platform requirements section](docs/compile.md#platform-requirements) of the compile guide if you intend to build Phase4 from source.

## Getting Started

List available input devices on your computer to find your device name. You can also confirm `f32` support.

```sh
phase4 --audio-list
```

Launch Phase4 using your device name (e.g., Duet 3) and a WebSocket listen address. If Phase4 is broadcasting, check this [CodePen example](https://codepen.io/rayboyd/full/wBzOPPr) to see the server in action.

```sh
phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889
```

By default every hardware channel is analysed and broadcast. To analyse only specific channels, pass `--audio-analyse-channels` with comma-separated zero-based indices, or set `audio.analyse_channels` in `config.yaml`.

```sh
phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --audio-analyse-channels 0,1
```

Calibration mode drives the full analysis pipeline with a synthetic sine wave. See [docs/calibration.md](docs/calibration.md).

## Config

See [docs/config.md](docs/config.md)

## Outputs

Beyond the core WebSocket stream, Phase4 can send OSC messages to any UDP target, and attach MIDI transport and clock data to the streams you already have running.

### WebSocket

See [docs/websockets.md](docs/websockets.md)

### OSC

See [docs/osc.md](docs/osc.md)

### MIDI

See [docs/midi.md](docs/midi.md)

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
