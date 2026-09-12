# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight tool for broadcasting real-time audio analysis and MIDI transport and clock data over WebSocket and OSC.

Any WebSocket-capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server. OSC output can be sent to any UDP target, including TouchDesigner's [OSC In DAT](https://derivative.ca/UserGuide/OSC_In_DAT).

## Download

Phase4 targets 64-bit [macOS](docs/compile.md#macos), [Windows](docs/compile.md#windows) and [Linux](docs/compile.md#linux). CI runs the checks and tests on Linux. The release workflow builds Linux x86_64 and macOS Apple Silicon binaries, but does not build or test Windows.

Pre-built binaries for macOS and Linux are on the [releases page](https://github.com/rayboyd/phase4/releases/latest). Windows users need to [compile from source](docs/compile.md). Check the [platform requirements section](docs/compile.md#platform-requirements) of the compile guide if you intend to build Phase4 from source.

## Getting Started

List available input devices to find your device name and check whether the default input configuration reports `F32`. Phase4 requires that application-facing format. It does not convert integer input formats or search for an alternative configuration. This requirement does not describe the hardware converter bit depth or audio quality.

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

Run Phase4 in an interactive terminal. Analysis and broadcasting run continuously. Press `Ctrl+C` to shut down. To run it under another process instead, see [headless mode](docs/headless.md).

Calibration mode drives the full analysis pipeline with a synthetic sine wave. See [docs/calibration.md](docs/calibration.md).

## Config

See [docs/config.md](docs/config.md)

## Outputs

Phase4 can drive more than one output at a time. Each is opt-in and stays off until you name an address or a device.

### WebSocket

Phase4 broadcasts analysis as one-way JSON over a loopback WebSocket, publishing at a 60 Hz target while the engine runs. Anything that can open a WebSocket can consume it, and eight clients share the stream by default. Phase4 accepts no application data back.

See [docs/websockets.md](docs/websockets.md)

### OSC

Phase4 sends analysis to any UDP target as OSC float messages, bundling every bin for every analysed channel into one datagram per frame under `/ch/{n}/bin/{n}`. It sends only and never listens, and it can run alongside the WebSocket output or instead of it.

See [docs/osc.md](docs/osc.md)

### MIDI

Phase4 can read a real MIDI input device, or a synthetic test clock, and attach transport and clock data to the WebSocket and OSC streams already running. It stays off until a device or test clock is named.

See [docs/midi.md](docs/midi.md)

## Headless

Phase4 runs without a terminal under `--headless`, writing a newline-delimited JSON event stream to stdout so a host process can supervise it. The stream reports when the engine is ready, the device, sample rate and channels it resolved, the addresses it actually bound, and why it stopped. Logs stay on stderr.

See [docs/headless.md](docs/headless.md)

## Architecture

See [docs/lifecycle.md](docs/lifecycle.md) for buffering, snapshot delivery, worker ownership and shutdown behaviour.

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
