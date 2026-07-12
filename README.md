# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight tool for real-time audio analysis and MIDI transport, broadcasting both over WebSocket and OSC. Any WebSocket-capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server. OSC output can be sent to any UDP target, including TouchDesigner's OSC In CHOP.

Check the [platform requirements section](docs/tutorials/compile.md#platform-requirements) of this document if you intend to build Phase4 from source.

Phase4 supports 64-bit [macOS](docs/tutorials/compile.md#macos), [Windows](docs/tutorials/compile.md#windows) and [Linux](docs/tutorials/compile.md#linux).

## Contents

- [Quickstart](#quickstart)
  - [Check](#check)
  - [Serve](#serve)
  - [Connect](#connect)
- [Outputs](#outputs)
  - [OSC](#osc)
  - [MIDI transport and clock](#midi-transport-and-clock)
- [Configuration file](#configuration-file)
- [Licence](#licence)

## Quickstart

Pre-built binaries for macOS and Linux are on the [releases page](https://github.com/rayboyd/phase4/releases/latest). Windows users need to [compile from source](docs/tutorials/compile.md). Compiling is also the route if you want a non-default band resolution.

1. [Check](#check) hardware compatibility.
2. Select a device and [serve](#serve) analysis data.
3. [Connect](#connect) a WebSocket client.

See [Outputs](#outputs) to also send OSC data or attach MIDI transport
and clock.

### Check

List available input devices to find your device index and confirm 32-bit Float support.

```sh
./phase4 --audio-list
```

Core Audio, the macOS audio subsystem, works internally with 32-bit Float and typically presents devices (including the built-in microphone) as F32 to applications. So running `./phase4 --audio-list` on a MacBook will almost certainly show the built-in mic as F32-capable.

If a device is not supported, you'll see **No hardware support (32-bit required)** in the terminal output.

### Serve

Launch Phase4 using your device name (e.g., Duet 3) and a WebSocket listen address.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889
```

Press `T` to toggle the engine's active state.
When MIDI input is configured (`--test-midi-clock` or `--midi-device`),
`S` sends Start, `X` sends Stop, and `R` sends Continue.

```
> [INFO] Welcome to phase4.
> [INFO] Audio device resolved (exact match): Duet 3
> [INFO] WebSocket server listening on ws://127.0.0.1:8889
> [INFO] Ready. Press T to toggle engine, S/X/R for MIDI Start/Stop/Continue, Ctrl+C to exit.
```

No audio hardware to hand, calibration mode drives the full pipeline with a synthetic sine wave. See [docs/tutorials/calibration.md](docs/tutorials/calibration.md).

### Connect

Phase4 streams real-time audio analysis data as a JSON broadcast. Any tool capable of opening a standard WebSocket connection (including browsers, Node.js, Python, or creative coding environments like TouchDesigner) can consume this stream. Point your WebSocket client to `ws://127.0.0.1:8889` to start receiving the data.

If Phase4 is broadcasting, check this [CodePen example](https://codepen.io/rayboyd/full/wBzOPPr) to see the server in action.

## Outputs

Beyond the core WebSocket stream, Phase4 can send OSC messages to any
UDP target, and attach MIDI transport and clock data to the streams
you already have running.

### OSC

Phase4 can send real-time analysis data as OSC float messages over UDP. Pass `--osc-addr` with a `host:port` target to enable it, either alongside `--ws-addr` or on its own.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --osc-addr 127.0.0.1:7000
```

Each frequency bin is sent as a separate OSC message with address `/phase4/ch/{channel}/bin/{bin}` and a single `f` argument in the range `0.0` to `1.0`. Map these addresses to parameters using your software's OSC shortcut editor.

See [docs/tutorials/osc.md](docs/tutorials/osc.md) for the full address reference and integration notes.

### MIDI transport and clock

Phase4 can also attach MIDI transport and clock data to the existing WebSocket payload stream, using either a real MIDI input device or a synthetic test clock.

Use one of the following flags.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --midi-device "Loopback"
```

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --test-midi-clock 120.0
```

`--midi-device` and `--test-midi-clock` are mutually exclusive.

When MIDI input is configured, each display frame may include a top-level `midi` key:

```json
{
  "channels": [{ "peak": 0.38, "bins": [0.0, 0.1, 0.2, 0.3] }],
  "midi": {
    "transport": "start",
    "steps": 24
  }
}
```

`transport` is one of `start`, `stop`, or `continue`, and is omitted when no transport event happened since the previous broadcast frame. `steps` is the absolute count of MIDI 1/16 note steps since the most recent Start event. The value does not reset each broadcast frame, clients detect new steps by comparing the current value to the previous frame.

When MIDI input is not configured, the `midi` key is absent, so clients that only read `channels` are unaffected.

When MIDI input is configured, the OSC sender also transmits `/phase4/midi/steps` every frame, one `i` argument, the current absolute step count, and `/phase4/midi/start`, `/phase4/midi/stop`, `/phase4/midi/continue`, each one `i` argument (`1`), sent only on the frame their transport event fired.

Embedding Phase4 in your own application is documented in [docs/tutorials/wrapper.md](docs/tutorials/wrapper.md).

## Configuration file

Instead of passing flags on every invocation you can place a `config.yaml` file in the current working directory, that is, wherever the Phase4 process is launched from, not where the binary itself lives on disk. Phase4 reads it at startup and applies a three-tier priority rule. CLI flags override file values, file values override hardcoded defaults. Any key may be omitted, and absent keys inherit the default.

If you embed Phase4 as a child process, set the child process working directory explicitly so `config.yaml` is found where you expect. Phase4 does not infer the location from the binary path.

Copy the bundled example as a starting point.

```sh
cp example.config.yaml config.yaml
```

Edit only the sections you need. For example, to pin a device and raise the broadcast rate:

```yaml
network:
  ws_addr: "127.0.0.1:8889"
  broadcast_rate: 90.0

audio:
  device_name_match: "Duet 3"
```

Persistent OSC output and vocoder tuning are also supported in the file:

```yaml
network:
  osc_addr: "127.0.0.1:7000"

vocoder:
  attack_ms: 20.0
  freq_high: 16000.0
```

See [example.config.yaml](example.config.yaml) for the full reference with all keys and their defaults.

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
