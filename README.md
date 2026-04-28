# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight audio analysis tool built for real-time audio visualisation. Any WebSocket-capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server.

Check the [platform requirements section](docs/tutorials/compile.md#platform-requirements) of this document if you intend to build Phase4 from source.

Phase4 supports 64-bit [macOS](docs/tutorials/compile.md#macos), [Windows](docs/tutorials/compile.md#windows) and [Linux](docs/tutorials/compile.md#linux).

## Quickstart

Pre-built binaries for macOS and Linux are on the [releases page](https://github.com/rayboyd/phase4/releases/latest). Windows users need to [compile from source](docs/tutorials/compile.md). Compiling is also the route if you want a non-default band resolution.

1. [Check](#check) hardware compatibility.
2. Select a device and [serve](#serve) analysis data.
3. [Connect](#connect) a client to the server.

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
> [INFO] 'A' to analyse, 'B' to broadcast, 'R' to record, Ctrl+C to exit
> [INFO] WebSocket server listening on ws://127.0.0.1:8889
```

### Connect

Phase4 streams real-time audio analysis data as a JSON broadcast. Any tool capable of opening a standard WebSocket connection (including browsers, Node.js, Python, or creative coding environments like TouchDesigner) can consume this stream. Point your WebSocket client to `ws://127.0.0.1:8889` to start receiving the data.

If Phase4 is broadcasting, check this [CodePen example](https://codepen.io/rayboyd/full/wBzOPPr) to see the server in action.

## Tutorials

- [WebSocket API](docs/tutorials/websockets.md)
- _todo_ [Virtual Soundcards with Blackhole](docs/tutorials/blackhole.md)
- _todo_ [TouchDesigner](docs/tutorials/touchdesigner.md)

## Roadmap

**0.0.1**

- [#8](https://github.com/rayboyd/phase4/pull/8) - ~~Runtime channel selection flags for the analyser and recorder~~
- [#13](https://github.com/rayboyd/phase4/pull/13) - ~~Documentation required for launch. Quickstart, WebSocket API, Readme.~~
- Blackhole virtual soundcard tutorial.
- TouchDesigner tutorial.

**0.0.2**

- Terminal displays peak levels per selected channel with `--monitor` flag.
- Local config file support for device presets and persistent flag defaults.
- Analysis low CPU mode. `--low-cpu` selects 32 bands, default remains 64, possible 128 option? Explain spectral detail trade-off vs smoothness in docs and tutorials.
- Double-buffered recording to decouple ring drain latency from disk write latency, improving reliability on high channel count devices.
- Add a dedicated `docs/troubleshooting.md` with OS-specific content, i.e. toggling 32-bit Float in Windows/macOS sound settings.

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
