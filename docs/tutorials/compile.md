# Compiling

Install Rust with `rustup` from [rustup.rs](https://rustup.rs/). This repository pins the stable toolchain and required components in [rust-toolchain.toml](rust-toolchain.toml), once `rustup` is installed, `cargo` will use the right toolchain automatically in this directory.

Clone the repository, and build a release version of Phase4.

```sh
cargo build --release --locked
```

> On Windows the binary will be called `phase4.exe`

## Feature flags

The number of display bands sent to clients is set at compile time via a feature flag. For most use cases, `display-bins-64` (the default) is the right choice.

Higher bin counts increase spectral detail but also CPU cost and data payload, and the visual difference is often imperceptible. Tuning `--vocoder-attack-ms` and `--vocoder-release-ms` to control envelope responsiveness is usually more effective at shaping the output than increasing bin count. For example, a slow release combined with 32 bins produces a tailed, ambient wash in the data that can be exactly what generative visuals need.

| Feature            | Bands | Notes                              |
| :----------------- | :---- | :--------------------------------- |
| `display-bins-32`  | 32    | Least spectral detail. Lowest CPU. |
| `display-bins-64`  | 64    | Default.                           |
| `display-bins-128` | 128   |                                    |
| `display-bins-256` | 256   | Most spectral detail. Highest CPU. |

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
