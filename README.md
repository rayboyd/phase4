# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight tool for real-time audio analysis and MIDI transport, broadcasting both over WebSocket and OSC. Any WebSocket-capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server. OSC output can be sent to any UDP target, including TouchDesigner's [OSC In DAT](https://derivative.ca/UserGuide/OSC_In_DAT).

Phase4 has one audio-data contract. It accepts native `f32` input, analyses 32 logarithmically spaced frequency bands per channel, and broadcasts the latest snapshot at 60 Hz.

Check the [platform requirements section](docs/compile.md#platform-requirements) of the compile guide if you intend to build Phase4 from source.

Phase4 supports 64-bit [macOS](docs/compile.md#macos), [Windows](docs/compile.md#windows) and [Linux](docs/compile.md#linux).

## Quickstart

Pre-built binaries for macOS and Linux are on the [releases page](https://github.com/rayboyd/phase4/releases/latest). Windows users need to [compile from source](docs/compile.md).

1. [Check](#check) hardware compatibility.
2. Select a device and [serve](#serve) analysis data.
3. [Connect](#connect) a WebSocket client.

See [Outputs](#outputs) to also send OSC data or attach MIDI transport
and clock.

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

By default every hardware channel is analysed and broadcast. To analyse only specific channels, pass `--audio-analyse-channels` with comma-separated zero-based indices, or set `audio.analyse_channels` in `config.yaml`. Indices are validated against the device's channel count at startup.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --audio-analyse-channels 0,1
```

No audio hardware to hand, calibration mode drives the full pipeline with a synthetic sine wave. See [docs/calibration.md](docs/calibration.md).

### Connect

Phase4 streams real-time audio analysis data as a JSON broadcast. Any tool capable of opening a standard WebSocket connection (including browsers, Node.js, Python, or creative coding environments like TouchDesigner) can consume this stream. Point your WebSocket client to `ws://127.0.0.1:8889` to start receiving the data.

If Phase4 is broadcasting, check this [CodePen example](https://codepen.io/rayboyd/full/wBzOPPr) to see the server in action.

See [docs/websockets.md](docs/websockets.md) for the full data structure and noise floor handling.

## Outputs

Beyond the core WebSocket stream, Phase4 can send OSC messages to any UDP target, and attach MIDI transport and clock data to the streams you already have running.

### OSC

Pass `--osc-addr` with a `host:port` target to enable it, either alongside `--ws-addr` or on its own.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --osc-addr 127.0.0.1:7000
```

See [docs/osc.md](docs/osc.md) for the address scheme and TouchDesigner integration notes.

### MIDI

List available MIDI input devices to find your device name.

```sh
./phase4 --midi-list
```

Use one of the following flags. They are mutually exclusive.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --midi-device "Loopback"
```

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --test-midi-clock 120.0
```

See [docs/midi.md](docs/midi.md) for the WebSocket and OSC schema.

## Config

Instead of passing flags on every invocation you can put them in a YAML file.

```sh
cp example.config.yaml config.yaml
```

```yaml
network:
  ws_addr: "127.0.0.1:8889"

audio:
  device_name_match: "Duet 3"
```

See [docs/config.md](docs/config.md) for the full priority rules and reference.

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
