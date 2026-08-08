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

A MIDI input device can also be pinned in `config.yaml`.

```yaml
midi:
  device_name_match: "Loopback"
```

`--test-midi-clock` stays CLI-only, it's a calibration flag and is never read from the file.

## WebSocket Schema

When MIDI input is configured, every WebSocket message also carries a top-level `midi` object.

```json
{
  "transport": "start",
  "steps": 24
}
```

`transport` is one of `start`, `stop`, or `continue`, and is omitted when no transport event happened since the previous broadcast frame. `steps` is the absolute count of MIDI 1/16 note steps since the most recent Start event. The value does not reset each broadcast frame, clients detect new steps by comparing the current value to the previous frame.

When MIDI input is not configured, the `midi` key is absent, so clients that only read `channels` are unaffected.

## OSC Addresses

When MIDI input is configured, the OSC sender transmits four addresses alongside the bin data.

| Address                 | Type | Value | Description                                                                  |
| :----------------------- | :--- | :---- | :----------------------------------------------------------------------------- |
| `/phase4/midi/steps`    | `i`  | count | Absolute MIDI 1/16 note steps since the most recent Start. Sent every frame. |
| `/phase4/midi/start`    | `i`  | `1`   | Sent only on the frame a Start transport event fired.                        |
| `/phase4/midi/stop`     | `i`  | `1`   | Sent only on the frame a Stop transport event fired.                         |
| `/phase4/midi/continue` | `i`  | `1`   | Sent only on the frame a Continue transport event fired.                     |

`/phase4/midi/steps` behaves like the bin addresses, sent every frame, clients detect new steps by comparing the current value to the previous frame. The three transport addresses instead follow an event model, each carrying a conventional bang value (`1`) that is only sent on the frame its event actually happened.

When MIDI input is not configured, none of these four addresses are ever sent.

See [OSC](osc.md) for the bin address scheme and general OSC output behaviour, and [WebSocket API](websockets.md) for the `channels` data structure the `midi` object sits alongside.
