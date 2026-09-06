# Compiling

Install Rust with `rustup` from [rustup.rs](https://rustup.rs/). This repository selects the floating `stable` channel and the Clippy and rustfmt components in [rust-toolchain.toml](../rust-toolchain.toml). It does not pin a Rust version. Run `rustup update stable` to update an existing installation.

The manifest declares Rust 1.87, but the current lockfile includes `time` 0.3.55, which requires Rust 1.88.0. The locked dependency set therefore cannot be built with Rust 1.87. Use an up-to-date stable toolchain. The declared minimum version still needs a separate manifest correction.

Clone the repository, and build a release version of Phase4.

```sh
cargo build --release --locked
```

> On Windows the binary will be called `phase4.exe`

## Fixed data contract

Every build processes `f32` audio samples, uses 32 vocoder bands per analysed channel, and schedules output snapshots at 60 Hz. Scheduling and transport delays can reduce the delivered rate, and intermediate snapshots can be skipped. These values are Phase4's data contract. No feature flags or build-time configuration are required.

The vocoder envelope controls remain configurable. Use `--vocoder-attack-ms` and `--vocoder-release-ms` to control how quickly the 32 bands rise and fall.

## Platform Requirements

Phase4 uses CPAL's default audio host and the selected device's default input configuration, including its sample rate and channel count. That configuration must report `F32`. Phase4 rejects other formats, including `I16`, and does not search the device's alternative supported configurations.

`F32` describes the samples delivered to the application. It does not imply a 32-bit floating-point hardware converter or a particular level of audio quality. A host or driver can present integer hardware samples as floats before Phase4 receives them.

List available input devices to confirm `f32` support before running Phase4.

```sh
./target/release/phase4 --audio-list
```

Core Audio uses floating-point audio at the application layer. Check the actual format reported by `--audio-list` on the machine you intend to use. See [Apple's Core Audio overview](https://developer.apple.com/library/archive/documentation/MusicAudio/Conceptual/CoreAudioOverview/WhatisCoreAudio/WhatisCoreAudio.html).

A device whose default configuration is not `F32` shows **No hardware support (32-bit required)** in the terminal output. This wording refers to Phase4's accepted stream format, not a hardware-quality check. Selecting a 32-bit integer format in an OS control panel does not satisfy the `F32` requirement.

If the device is missing, check its connection, driver and audio-input permissions. If it is listed with another format, changing host or driver settings may change the reported default, but support must be checked again with `--audio-list`.

The current dependencies restrict builds to x86_64 and aarch64. CI runs on Linux, release builds cover Linux x86_64 and macOS aarch64, and Windows is not built or tested by the repository workflows.

The analyser currently uses `no_denormals` to alter floating-point processor flags. On x86_64, its use conflicts with [Rust's documented floating-point environment requirements](https://doc.rust-lang.org/core/arch/x86_64/fn._mm_setcsr.html). This is an unresolved portability issue, even when a build and soak test succeed.

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
