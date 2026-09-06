# Configuration

Instead of passing flags on every invocation you can put them in a YAML file. Phase4 reads it at startup and applies a three-tier priority rule. CLI flags override file values, file values override hardcoded defaults. Any key may be omitted, and absent keys inherit the default. Unknown keys are rejected as startup errors so misspelled settings cannot be silently ignored.

## Setting Up a Config File

Without `--config`, Phase4 looks for an optional `config.yaml` in the current working directory, that is, wherever the Phase4 process is launched from, not where the binary itself lives on disk.

Copy the bundled example as a starting point.

```sh
cp example.config.yaml config.yaml
```

Edit only the sections you need. This example selects a WebSocket address and audio device.

```yaml
network:
  ws_addr: "127.0.0.1:8889"

audio:
  device_name_match: "Duet 3"
```

Persistent OSC output and vocoder tuning are also supported in the file.

```yaml
network:
  osc_addr: "127.0.0.1:7000"

vocoder:
  attack_ms: 20.0
  freq_high: 16000.0
```

Vocoder settings must produce finite, stable `f32` filter coefficients at the resolved input sample rate. Phase4 checks all 32 bands before starting the audio pipeline and rejects configurations with poles on or outside the unit circle. Very low band frequencies or very high Q values can fail this check even when the numeric settings are otherwise valid. The error identifies the affected band and sample rate. Increase the lowest band frequency or reduce Q before retrying.

A MIDI input device can be pinned the same way, see [MIDI](midi.md#enabling-midi-input).

## Named Config Files

Pass `--config` with a path to name the file explicitly, which makes it easy to keep one config per setup.

```sh
./phase4 --config digitone.yaml
./phase4 --config focusrite.yaml
```

With `--config`, the file must exist, a missing file is a startup error.

## Input and output selection

At least one output address is required. WebSocket accepts only loopback IP addresses, such as `127.0.0.1` or `::1`. OSC can target a local or remote IP address. Both settings require an IP address and port, not a DNS hostname.

Audio device matching prefers a case-sensitive exact name, then the first case-insensitive substring match. MIDI matching is case-insensitive for both exact and substring matches. Neither falls back to the system default when no device matches.

`audio.analyse_channels` contains zero-based hardware indices. The list is sorted and deduplicated, and an empty list or an unavailable hardware index is rejected. Outputs contain only the selected channels in ascending hardware order, with contiguous output indices. Selecting `3,1,3` therefore sends hardware channel 1 as output channel 0 and hardware channel 3 as output channel 1.

Calibration flags take precedence over an audio device name. A non-empty channel selection is ignored with a warning in calibration mode, which always generates stereo. `--test-midi-clock` overrides a MIDI device set in the file, but conflicts with an explicit `--midi-device` flag.

Vocoder attack and release values are positive, finite time constants in milliseconds. For a constant rectified target, one time constant moves the envelope approximately 63 percent towards that target. It is not a time to reach the target completely. The highest band centre must be at or below 45 percent of the resolved sample rate.

Device listing, calibration and `--no-browser-origin` are CLI-only. Run `phase4 --help` for the complete CLI surface. See [example.config.yaml](../example.config.yaml) for every supported YAML key and its default.
