# Calibration Mode

Calibration mode replaces the hardware input with a synthetic sine wave, making the full pipeline (analysis, WebSocket broadcast, and OSC output) operational with no audio device attached. Use it to verify an installation, exercise a visualisation with a known signal, or pick out how a specific frequency lands in the display bins.

## Fixed tone

Pass `--test-hz` with a frequency in Hz. No `--device` is required.

```sh
./phase4 --test-hz 440
```

The pipeline runs at a synthetic 44.1 kHz stereo configuration and every display bin responds to a steady tone at the given frequency. This is useful for checking which bin a frequency of interest falls into.

## Frequency sweep

Pass `--test-sweep` with an LFO rate in Hz. The signal sweeps logarithmically from 20 Hz up to 0.45 times the sample rate, driven by a sine LFO at the given rate. One full up-and-down cycle takes 1 divided by the rate in seconds, so `0.2` produces a five second cycle and `0.1` a ten second cycle.

```sh
./phase4 --test-sweep 0.2
```

A sweep exercises every display bin in turn, which makes it the quickest way to confirm a visualisation responds across the whole spectrum.

## Notes

- The two flags are mutually exclusive. Passing both is rejected at argument parsing with a non-zero exit code, which matters if you assemble phase4 arguments programmatically from a wrapper process.
- The signal level is fixed at approximately -12 dBFS, leaving headroom so the display output sits at a comfortable level without clipping.
- WebSocket and OSC output behave exactly as they do with a hardware device, so `--osc-addr`, `--broadcast-rate`, and the rest of the network options apply unchanged.
