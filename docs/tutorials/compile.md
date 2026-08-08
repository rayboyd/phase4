# Compiling

Install Rust with `rustup` from [rustup.rs](https://rustup.rs/). This repository pins the stable toolchain and required components in [rust-toolchain.toml](../../rust-toolchain.toml), once `rustup` is installed, `cargo` will use the right toolchain automatically in this directory.

Clone the repository, and build a release version of Phase4.

```sh
cargo build --release --locked
```

> On Windows the binary will be called `phase4.exe`

## Fixed data contract

Every build uses native `f32` audio samples, 32 vocoder bands per channel, and a 60 Hz output cadence. These values are Phase4's data contract.

The vocoder envelope controls remain configurable. Use `--vocoder-attack-ms` and `--vocoder-release-ms` to control how quickly the 32 bands rise and fall.

## Platform Requirements

Phase4 uses your system’s native audio drivers. To work correctly, your audio interface or microphone must expose an `f32` input configuration. Most modern interfaces support this by default.

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
