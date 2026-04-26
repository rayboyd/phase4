# Phase4

[![Build](https://github.com/rayboyd/phase4/actions/workflows/build.yml/badge.svg)](https://github.com/rayboyd/phase4/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/rayboyd/phase4/blob/main/LICENSE)
[![Security Policy](https://img.shields.io/badge/Security-Policy-green.svg)](https://github.com/rayboyd/phase4/blob/main/SECURITY.md)

Phase4 is a fast, lightweight audio analysis tool built for real-time audio visualisation. Any WebSocket-capable tooling, such as [TouchDesigner](https://derivative.ca/) or a browser using the [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API), can connect to the Phase4 server.

Check the [platform requirements section](#platform-requirements) of this document if you intend to build Phase4 from source.

Phase4 supports 64-bit [macOS](#macos), [Windows](#windows) and [Linux](#linux).

## Tutorials

- [WebSocket API](docs/tutorials/websockets.md)
- [TouchDesigner](docs/tutorials/touchdesigner.md)

## Quickstart

Compile from source or grab the latest binary from the [releases page](https://github.com/rayboyd/phase4/releases/latest) to get started.

1. [Check](#check) hardware compatibility.
2. Select a device and [serve](#serve) analysis data.
3. [Connect](#connect) a client to the server.

### Check

List available input devices to find your device index and confirm 32-bit Float support.

```sh
./phase4 --list
```

```sh
> [INFO] [0] Soundcard One (16000Hz, 1ch, I16) * No hardware support (32-bit required)
> [INFO] [1] Soundcard Two (48000Hz, 2ch, F32)
```

_If a device is not supported, you'll see **No hardware support (32-bit required)** in the terminal output._

### Serve

Launch Phase4 using your device index (e.g., index 0).

```sh
./phase4 --device 0
```

```sh
> [INFO] 'A' to analyse, 'B' to broadcast, 'R' to record, Ctrl+C to exit
> [INFO] WebSocket server listening on ws://127.0.0.1:8889
```

_Press `A` to start analysis and `B` to start broadcasting. No harm done if you forget, but you will not get any data._

### Connect

Point your WebSocket client (like TouchDesigner or a browser) to `ws://127.0.0.1:8889`.

Copy this into a `.html` file to see the data in action. No dependencies required.

```html
<canvas id="viz" width="800" height="300" style="background:#111;"></canvas>

<script>
  const canvas = document.getElementById("viz");
  const ctx = canvas.getContext("2d");
  const ws = new WebSocket("ws://127.0.0.1:8889");

  ws.onmessage = (event) => {
    const { channels } = JSON.parse(event.data);
    if (!channels?.length) return;

    const bins = channels[0].bins;
    const barWidth = canvas.width / bins.length;

    ctx.clearRect(0, 0, canvas.width, canvas.height);

    bins.forEach((val, i) => {
      // Apply a gentle perceptual scale to compensate for high-frequency bin energy drop-off.
      const scale = 1 + i * 0.05;
      const barHeight = val * canvas.height * scale;
      ctx.fillStyle = `hsl(${(i / bins.length) * 360}, 80%, 60%)`;
      ctx.fillRect(
        i * barWidth,
        canvas.height - barHeight,
        barWidth - 1,
        barHeight,
      );
    });
  };
</script>
```

## Compiling

Install Rust with `rustup` from [rustup.rs](https://rustup.rs/). This repository pins the stable toolchain and required components in [rust-toolchain.toml](rust-toolchain.toml), once `rustup` is installed, `cargo` will use the right toolchain automatically in this directory.

Clone the repository, and build a release version of Phase4.

```sh
cargo build --release --locked
```

> On Windows the binary will be called `phase4.exe`

### Feature flags

The analyser FFT output resolution is set at compile time via a feature flag. For most use cases, `display-bins-64` (the default) is the right choice.

Higher bin counts increase spectral detail but also CPU cost and data payload, and the visual difference is often imperceptible. Tuning `--vocoder-attack-ms` and `--vocoder-release-ms` to control envelope responsiveness is usually more effective at shaping the output than increasing bin count. For example, a slow release combined with 32 bins produces a tailed, ambient wash in the data that can be exactly what generative visuals need.

| Feature            | Bands | Why.                               |
| :----------------- | :---- | :--------------------------------- |
| `display-bins-32`  | 32    | Least spectral detail. lowest CPU. |
| `display-bins-64`  | 64    |                                    |
| `display-bins-128` | 128   |                                    |
| `display-bins-256` | 256   | Most spectral detail. highest CPU. |

```sh
# Default (64 bands)
cargo build --release --locked

# High resolution (128 bands)
cargo build --release --locked --no-default-features --features display-bins-128
```

## Platform Requirements

Phase4 uses your system’s native audio drivers. To work correctly, your audio interface or microphone must be set to **32-bit Float** input mode. Most modern interfaces support this by default.

If Phase4 doesn't detect your device, check your OS sound settings (e.g., Windows Sound Control Panel or macOS Audio MIDI Setup) to ensure the format is set to "32-bit Float".

### Linux

Phase4 requires the ALSA (Advanced Linux Sound Architecture) development headers. On Ubuntu, Debian, and similar, you should install the necessary build dependencies.

```sh
sudo apt-get update
sudo apt-get install -y libasound2-dev pkg-config
```

If you are on a very recent distribution (e.g., Ubuntu 24.04+) and the above fails, ensure your package manager is pointing to the updated libasound2 development headers.

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
