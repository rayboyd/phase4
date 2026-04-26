# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight audio analysis tool built for real-time audio visualization. Any WebSocket capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server.

It supports 64-bit [macOS](#macos), [Windows](#windows) and [Linux](#linux). Check the [platform requirements section](#platform-requirements) of this document if you intend to build Phase4 from source.

## Quickstart

**Check** - Run Phase4 to see available devices and ensure your interface is set to 32-bit Float.

```sh
./phase4 --list
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

_On Windows the binary will be called `phase4.exe`_

## Platform Requirements

Phase4 uses your system’s native audio drivers. To work correctly, your audio interface or microphone must be set to **32-bit Float** input mode. Most modern interfaces support this by default.

_If Phase4 doesn't detect your device, check your OS sound settings (e.g., Windows Sound Control Panel or macOS Audio MIDI Setup) to ensure the format is set to "32-bit Float"._

### Linux

Phase4 requires the ALSA (Advanced Linux Sound Architecture) development headers. On Ubuntu, Debian, and similar, you can install the necessary build dependencies with.

```sh
sudo apt-get update
sudo apt-get install -y libasound2-dev pkg-config
```

_If you are on a very recent distribution (e.g., Ubuntu 24.04+) and the above fails, ensure your package manager is pointing to the updated libasound2 development headers._

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

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
