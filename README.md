# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight audio analysis tool built for real-time audio visualisation. Any WebSocket capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server.

It supports 64-bit [macOS](#macos), [Windows](#windows) and [Linux](#linux). Check the [platform requirements section](#platform-requirements) of this document if you intend to build Phase4 from source.

## Quickstart

**Check** - Run Phase4 to see available devices and ensure your interface is set to 32-bit Float.

```sh
./phase4 --list
```

> **Note** If a device is not supported, you'll see **No hardware support (32-bit required)** in the terminal output.

```sh
[INFO] [0] Soundcard One (16000Hz, 1ch, I16) * No hardware support (32-bit required)
[INFO] [1] Soundcard Two (48000Hz, 2ch, F32)
```

**Serve** - Launch Phase4 using your device index (e.g., index 0).

```sh
./phase4 --device 0
```

**Connect** - Point your WebSocket client (like TouchDesigner or a browser) to `ws://127.0.0.1:8889`.

### Tutorials

- [WebSocket API](docs/tutorials/websockets.md)
- [TouchDesigner](docs/tutorials/touchdesigner.md)

## Compiling

Install Rust with `rustup` from [rustup.rs](https://rustup.rs/). This repository pins the stable toolchain and required components in [rust-toolchain.toml](rust-toolchain.toml), once `rustup` is installed, `cargo` will use the right toolchain automatically in this directory.

Clone the repository, and build a release version of Phase4.

```sh
cargo build --release --locked
```

> **Note** On Windows the binary will be called `phase4.exe`

### Feature flags

The analyser FFT output resolution is set at compile time via a feature flag.

| Feature            | Bands | Why.                               |
| :----------------- | :---- | :--------------------------------- |
| `display-bins-32`  | 32    | Least spectral detail. lowest cpu. |
| `display-bins-64`  | 64    |                                    |
| `display-bins-128` | 128   |                                    |
| `display-bins-256` | 256   | Most spectral detail. highest cpu. |

> **Note** For most use cases, `display-bins-64` (the default) is the right choice. Higher bin counts increase spectral detail but also CPU cost, and the visual difference is often imperceptible. Start at 64 and only go higher if you have a specific reason to.

```sh
# Default (64 bands)
cargo build --release --locked

# High resolution (128 bands)
cargo build --release --locked --no-default-features --features display-bins-128
```

## Platform Requirements

Phase4 uses your system’s native audio drivers. To work correctly, your audio interface or microphone must be set to **32-bit Float** input mode. Most modern interfaces support this by default.

> **Note** If Phase4 doesn't detect your device, check your OS sound settings (e.g., Windows Sound Control Panel or macOS Audio MIDI Setup) to ensure the format is set to "32-bit Float".

### Linux

Phase4 requires the ALSA (Advanced Linux Sound Architecture) development headers. On Ubuntu, Debian, and similar, you should install the necessary build dependencies.

```sh
sudo apt-get update
sudo apt-get install -y libasound2-dev pkg-config
```

> **Note** If you are on a very recent distribution (e.g., Ubuntu 24.04+) and the above fails, ensure your package manager is pointing to the updated libasound2 development headers.

### macOS

On macOS you may need to install the Xcode Command Line Tools. You don't need the full Xcode app from the App Store. A popup will appear asking if you want to install the tools. Click Install.

```sh
xcode-select --install
```

### Windows

To build on Windows, you must install the Microsoft Visual C++ (MSVC) toolchain.

Download the [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/?q=build+tools). In the installer, check the box for Desktop development with C++. Ensure MSVC and Windows 10/11 SDK are selected in the installation details panel on the right.

Once installation finishes, restart your PowerShell or Command Prompt to refresh your environment variables.

## Roadmap

**0.0.1**

- [#8](https://github.com/rayboyd/phase4/pull/8) - ~~Runtime channel selection flags for the analyser and recorder~~
- Documentation required for launch. Quickstart, WebSocket API, TouchDesigner.

**0.0.2**

- Terminal displays peak levels per selected channel with `--monitor` flag.
- Local config file support for device presets and persistent flag defaults.
- Analysis low CPU mode. `--low-cpu` selects 32 bands, default remains 64, possible 128 option? Explain spectral detail tradeoff vs smoothness in docs and tutorials.
- Double-buffered recording to decouple ring drain latency from disk write latency, improving reliability on high channel count devices.
- Add a dedicated `docs/troubleshooting.md` with OS-specific content, i.e. toggling 32-bit Float in Windows/macOS sound settings.

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
