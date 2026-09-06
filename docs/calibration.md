# Calibration Mode

Calibration mode replaces the hardware input with a synthetic sine wave, making the full pipeline (analysis, WebSocket broadcast, and OSC output) operational with no audio device attached. Use it to verify an installation, exercise a visualisation with a known signal, or pick out how a specific frequency lands in the display bins.

## Fixed tone

Pass `--test-hz` with a frequency above 0 Hz and no higher than 19,845 Hz. This ceiling is 0.45 times the synthetic 44.1 kHz sample rate and retains anti-aliasing headroom. No `--audio-device` is required, but at least one output transport (`--ws-addr` or `--osc-addr`) must still be given.

```sh
./phase4 --test-hz 440 --ws-addr 127.0.0.1:8889
```

The pipeline runs at a synthetic 44.1 kHz stereo configuration with the same signal in both channels. The bands respond according to their filter frequencies, bandwidths and envelope settings. This is useful for checking which bin a frequency of interest falls into.

## Frequency sweep

Pass `--test-sweep` with an LFO rate above 0 Hz and no higher than 19,845 Hz. The signal sweeps logarithmically from 20 Hz up to 0.45 times the sample rate, driven by a sine LFO at the given rate. One full up-and-down cycle takes 1 divided by the rate in seconds, so `0.2` produces a five second cycle and `0.1` a ten second cycle.

```sh
./phase4 --test-sweep 0.2 --ws-addr 127.0.0.1:8889
```

A slow sweep traverses the configured filter bank and lets you inspect its response. Fast sweeps interact with filter settling and envelope timing, so they do not give each band a steady-state measurement. The 19,845 Hz limit validates the LFO parameter, but does not guarantee an alias-free frequency-modulated signal at high sweep rates.

## Notes

- The two flags are mutually exclusive. Passing both is rejected at argument parsing with a non-zero exit code.
- Both values must be finite and within the documented range. Zero, negative values, `NaN`, positive infinity, negative infinity, and values above 19,845 Hz are rejected during configuration.
- The input sine amplitude is fixed at `0.25`, approximately -12 dBFS. Filter gain can still produce bins above `1.0`. The output is not normalised or clamped.
- Calibration takes precedence over a configured audio device. A non-empty channel selection is ignored with a warning, and both generated channels are analysed.
- WebSocket and OSC output behave exactly as they do with a hardware device, including the fixed 32-band, 60 Hz data contract. At least one of `--ws-addr` or `--osc-addr` must be given, both are opt-in.
