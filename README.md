# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight audio analysis tool built for real-time audio visualization. Any WebSocket capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server.

It supports 64-bit 64-bit [macOS](#macos), [Windows](#windows) and [Linux](#linux).

## Compiling

Install Rust with `rustup` from [rustup.rs](https://rustup.rs/). This repository pins the stable toolchain and required components in [rust-toolchain.toml](rust-toolchain.toml), once `rustup` is installed, `cargo` will use the right toolchain automatically in this directory.

Clone the repository, and build a release version of Phase4.

```sh
cargo build --release --locked
```

## Usage

Phase4 runs as an interactive terminal app. Once started, single key presses toggle features on and off. Status changes, warnings and recoverable errors are logged directly to the terminal.

| Key      | Effect                     |
| -------- | -------------------------- |
| `A`      | Toggle audio analysis      |
| `B`      | Toggle WebSocket broadcast |
| `R`      | Toggle audio recording     |
| `Ctrl+C` | Quit                       |

```sh
# List available input devices.
phase4 --list

# Start with device at index.
phase4 -d <index>

# Start with a specific WebSocket address and 16-bit recording.
phase4 -d 0 -a 127.0.0.1:9000 -b 16
```

_On Windows the binary will be called `phase4.exe`_

## Requirements

Phase4 uses the platform's native audio driver, and requires a device whose default input configuration is `f32`. Most audio interfaces provide this natively. On Linux, install the ALSA development libraries before building.

### macOS

On macOS you may need to install the Xcode Command Line Tools. You don't need the full Xcode app from the App Store. A popup will appear asking if you want to install the tools. Click Install.

```sh
xcode-select --install
```

### Windows

To build on Windows, you must install the Microsoft Visual C++ (MSVC) toolchain.

Download the [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/?q=build+tools). In the installer, check the box for Desktop development with C++. Ensure MSVC and Windows 10/11 SDK are selected in the installation details panel on the right.

Once installation finishes, restart your PowerShell or Command Prompt to refresh your environment variables.

### Linux

On Ubuntu or Debian, install the native audio build dependencies. These are required for the `cpal` crate used by Phase4.

```sh
sudo apt-get update
sudo apt-get install -y libasound2-dev pkg-config
```

## Roadmap

**0.0.1**

- [#8](https://github.com/rayboyd/phase4/pull/8) - ~~Runtime channel selection flags for the analyser and recorder~~
- Local config file support (`~/.config/phase4/config.toml` or `.phase4.toml`) for device presets and persistent flag defaults.

**0.0.2**

- Double-buffered recording to decouple ring drain latency from disk write latency, improving reliability on high channel count devices.
- `--monitor` mode: terminal peak level display per selected channel, for device inspection without a WebSocket client.

## Licence

Apache License, Version 2.0. See [LICENSE](https://github.com/rayboyd/phase4/blob/main/LICENSE).
