# Phase4

**Note:** This project is currently in alpha development. It is not considered stable and may contain bugs or undergo significant changes. I've only tested this on ARM64.

Phase4 is a real-time audio analysis engine written in Go. It captures audio input, performs Fast Fourier Transform (FFT) analysis, and can optionally send the results over UDP.

## Getting Started

### Prerequisites

**Go:** Check `go.mod` for the required version.

**PortAudio Development Libraries:** Required for audio capture.

- **macOS (Homebrew):** `brew install portaudio`
- **Debian/Ubuntu:** `sudo apt-get update && sudo apt-get install portaudio19-dev`
- **Windows:** Download from the [PortAudio website](http://www.portaudio.com/download.html) or use a package manager.

### Building

A build script is provided:

```sh
./bin/build.sh
```

This script compiles the application, placing the binary (default name `app`) into the `build/` directory.

Run `bin/build.sh --test` to also run unit tests after building.

### Configuration

The application is configured using `config.yaml`.

Check `internal/config/yaml.go` for details on configuration options and potential environment variable overrides.

## Usage

### Running the Engine

Execute the compiled binary from the project root:

```sh
./build/app
```

## Ideas

1.  **Overall Energy / Loudness:**

    - **What:** Measures the overall amplitude or power of the signal in a buffer. You're already calculating RMS in the `BeatDetector`.
    - **Calculation:** RMS (Root Mean Square) for average loudness, Peak Amplitude for maximum instantaneous level.
    - **Use/Visualization:**
      - Drive a simple VU meter display.
      - Control the overall brightness, size, or intensity of other visual elements (e.g., brighter spectrum on louder sounds).
      - Send as: `{"type": "energy", "rms": 0.5, "peak": 0.9}`.

2.  **Band Energy:**

    - **What:** Calculates the energy/loudness within specific frequency ranges (e.g., Sub-Bass, Bass, Mid, Treble).
    - **Calculation:** Sum the magnitudes from the FFT results within predefined frequency bins corresponding to each band.
    - **Use/Visualization:**
      - Drive separate visual elements for different frequency ranges (e.g., a pulsing effect for bass, shimmering for highs).
      - Control the color or shape of parts of the visualization based on the balance of frequencies.
      - Send as: `{"type": "band_energy", "sub": 0.8, "bass": 0.6, "mid": 0.3, "high": 0.4}`.

3.  **Transient Detection:**

    - **What:** Detects sharp, sudden increases in signal energy, often corresponding to the start ("attack") of sounds like drum hits, plucks, or even sharp vocal consonants. More sophisticated than simple energy thresholding.
    - **Calculation:** Often involves looking at the rate of change in energy or spectral content (Spectral Flux is common). Algorithms like those in Aubio are designed for this.
    - **Use/Visualization:**
      - Trigger brief visual flashes, particle bursts, or sharp movements synchronized with the transients.
      - Could be sent as events similar to the beat detector: `{"type": "event", "name": "transient", "strength": 0.7}`.

4.  **Spectral Centroid:**

    - **What:** The "center of mass" of the spectrum. It indicates where the average frequency concentration is, relating to the perceived "brightness" of the sound.
    - **Calculation:** Weighted average of frequency bins, using magnitudes as weights.
    - **Use/Visualization:**
      - Control the overall color hue of the visualization (e.g., bluer for higher centroid/brighter sounds, redder for lower centroid/darker sounds).
      - Plot its value over time as a line graph.
      - Send as: `{"type": "centroid", "value": 1500.5}` (value in Hz).

5.  **Spectral Flux:**

    - **What:** Measures how quickly the shape of the spectrum is changing from one frame to the next. High flux often indicates an onset or transient.
    - **Calculation:** Calculate the difference (e.g., Euclidean distance) between the magnitude spectrum of the current frame and the previous frame.
    - **Use/Visualization:**
      - Similar to transients, can drive the intensity or frequency of flashes, pulses, or other reactive elements.
      - Send as: `{"type": "flux", "value": 0.85}`.

6.  **Key Detection:**
    - **What:** Estimates the musical key (e.g., C Major, A minor) of the audio.
    - **Calculation:** More complex, typically involves calculating a Chromagram (energy per pitch class) and matching it against predefined key profiles. Libraries like Aubio often have functions for this.
    - **Use/Visualization:**
      - Display the detected key and potentially a confidence score.
      - Could subtly influence the color palette of the visualization to match the key's mood (though this is subjective).
      - Send as: `{"type": "key", "key": "Amin", "confidence": 0.75}`.

Adding processors for Overall/Band Energy and Transients/Flux would likely provide the most immediate visual impact alongside the FFT and BPM data.
