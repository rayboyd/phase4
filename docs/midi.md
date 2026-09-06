# MIDI

Phase4 can attach MIDI transport and clock data to the WebSocket and OSC streams you already have running, using either a real MIDI input device or a synthetic test clock.

## Enabling MIDI Input

List available MIDI input devices to find your device name.

```sh
./phase4 --midi-list
```

Pass `--midi-device` with a device name, or `--test-midi-clock` with a tempo in BPM for a synthetic clock. The two flags are mutually exclusive.

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --midi-device "Loopback"
```

```sh
./phase4 --audio-device "Duet 3" --ws-addr 127.0.0.1:8889 --test-midi-clock 120.0
```

The synthetic clock tempo must be finite and positive, and its MIDI tick interval must be representable and non-zero. A real MIDI device is opened during startup, if the selected device disappears or cannot be opened, Phase4 exits before starting any workers.

MIDI input supplies transport events and a count derived from clock ticks. Notes, velocity, controller messages and song-position messages are not forwarded. Audio and MIDI are sampled independently, and the output contains no shared sample timestamp.

A MIDI input device can also be pinned in `config.yaml`.

```yaml
midi:
  device_name_match: "Loopback"
```

`--test-midi-clock` stays CLI-only. It overrides a MIDI device configured in YAML and begins with a synthetic Start event. It does not replace audio input, so an audio device or audio calibration signal is still required.

## WebSocket Schema

When MIDI input is configured, mapper publications carry a top-level `midi` object. Its contents are shown below. An initial WebSocket snapshot sent before the first mapper publication can omit it.

```json
{
  "transport": "start",
  "steps": 24
}
```

`transport` contains the last `start`, `stop`, or `continue` event received since the mapper's previous publication. It is omitted when no event is pending. Several events between publications collapse to the last one. The field is an event snapshot, not the current running or stopped state.

`steps` counts one MIDI 1/16 note step for every six received clock ticks. Start resets both the step count and partial tick count. Stop and Continue do not reset or gate clock counting, so steps still advance if the source sends clocks while stopped. Clients detect new steps by comparing successive values. The unsigned 32-bit count wraps after `4,294,967,295`.

MIDI continues being received while the engine is paused, but the mapper does not publish or clear the pending transport event during the pause. Snapshot replacement can lose a transport event before a client observes it, and a newly connected client can receive a retained event. This stream is not a lossless MIDI event log.

When MIDI input is not configured, the `midi` key is absent, so clients that only read `channels` are unaffected.

## OSC Addresses

When MIDI input is configured, the OSC sender transmits four addresses alongside the bin data.

| Address                 | Type | Value | Description                                                                  |
| :----------------------- | :--- | :---- | :----------------------------------------------------------------------------- |
| `/phase4/midi/steps`    | `i`  | count | Absolute MIDI 1/16 note steps since the most recent Start. Sent every frame. |
| `/phase4/midi/start`    | `i`  | `1`   | Sent when the consumed snapshot contains Start.                        |
| `/phase4/midi/stop`     | `i`  | `1`   | Sent when the consumed snapshot contains Stop.                         |
| `/phase4/midi/continue` | `i`  | `1`   | Sent when the consumed snapshot contains Continue.                     |

`/phase4/midi/steps` behaves like the bin addresses, sent every frame, clients detect new steps by comparing the current value to the previous frame. The three transport addresses each carry a conventional bang value (`1`) when the consumed snapshot contains that event. MIDI packets are separate from the audio-bin bundle, so they are not delivered atomically with it. Snapshot replacement and UDP loss can omit events.

OSC encodes `steps` as a signed 32-bit integer using the same bit pattern as the unsigned count. Values above `2,147,483,647` therefore appear negative until the counter wraps. The WebSocket JSON count remains unsigned.

When MIDI input is not configured, none of these four addresses are ever sent.

See [OSC](osc.md) for the bin address scheme and general OSC output behaviour, and [WebSocket API](websockets.md) for the `channels` data structure the `midi` object sits alongside.
