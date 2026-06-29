# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight audio analysis tool built for real-time audio visualisation. WebSocket and OSC are both first-class output protocols. Any WebSocket-capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server. OSC output can be sent to any UDP target, including [Resolume](https://resolume.com/) and similar media servers.

Check the [platform requirements section](docs/tutorials/compile.md#platform-requirements) of this document if you intend to build Phase4 from source.

Phase4 supports 64-bit [macOS](docs/tutorials/compile.md#macos), [Windows](docs/tutorials/compile.md#windows) and [Linux](docs/tutorials/compile.md#linux).

## Quickstart

Pre-built binaries for macOS and Linux are on the [releases page](https://github.com/rayboyd/phase4/releases/latest). Windows users need to [compile from source](docs/tutorials/compile.md). Compiling is also the route if you want a non-default band resolution.

1. [Check](#check) hardware compatibility.
2. Select a device and [serve](#serve) analysis data.
3. [Connect](#connect) a WebSocket client.
4. Optionally [send OSC output](#osc).

### Check

List available input devices to find your device index and confirm 32-bit Float support.

```sh
./phase4 --list
```

Core Audio, the macOS audio subsystem, works internally with 32-bit Float and typically presents devices (including the built-in microphone) as F32 to applications. So running `./phase4 --list` on a MacBook will almost certainly show the built-in mic as F32-capable.

The output from my **MacBook Pro M4** is as follows.

```
> [INFO] [0] Test Soundcard One (16000Hz, 1ch, I16) * No hardware support (32-bit required)
> [INFO] [1] Duet 3 (48000Hz, 2ch, F32)
> [INFO] [2] MacBook Pro Microphone (48000Hz, 1ch, F32)
> [INFO] [3] Microsoft Teams Audio (48000Hz, 2ch, F32)
```

_If a device is not supported, you'll see **No hardware support (32-bit required)** in the terminal output._

### Serve

Launch Phase4 using your device index (e.g., index 0).

```sh
./phase4 --device 0
```

Press `A` to start analysis and `B` to start broadcasting. No harm done if you forget, but you will not get any data.

```
> [INFO] 'A' to analyse, 'B' to broadcast, Ctrl+C to exit
> [INFO] WebSocket server listening on ws://127.0.0.1:8889
```

### Connect

Phase4 streams real-time audio analysis data as a JSON broadcast. Any tool capable of opening a standard WebSocket connection (including browsers, Node.js, Python, or creative coding environments like TouchDesigner) can consume this stream. Point your WebSocket client to `ws://127.0.0.1:8889` to start receiving the data.

If Phase4 is broadcasting, check this [CodePen example](https://codepen.io/rayboyd/full/wBzOPPr) to see the server in action.

### OSC

Phase4 can send real-time analysis data as OSC float messages over UDP. Pass `--osc-addr` with a `host:port` target to enable it alongside the WebSocket broadcast.

```sh
./phase4 --device 0 --osc-addr 127.0.0.1:7000
```

Each frequency bin is sent as a separate OSC message with address `/phase4/ch/{channel}/bin/{bin}` and a single `f` argument in the range `0.0` to `1.0`. Map these addresses to parameters using your software's OSC shortcut editor.

See [docs/tutorials/osc.md](docs/tutorials/osc.md) for the full address reference and integration notes.

## Roadmap

**0.0.2**

- Local config file support for device presets and persistent flag defaults.

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
