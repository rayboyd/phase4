# Headless Mode

`--headless` runs Phase4 without a terminal and writes a newline-delimited JSON event stream to stdout, so a supervising host process can tell when the engine is ready, what it resolved, and why it stopped.

This is a control plane. The WebSocket and OSC outputs are unaffected, and a host connects to them exactly as any other client does.

```sh
phase4 --headless --audio-device "Duet 3" --ws-addr 127.0.0.1:0
```

`--headless` is CLI-only and is never read from a configuration file. It is a presence flag with no explicitly-off form, so offering it in the file would break the rule that a CLI flag always overrides a file value. This matches `--no-browser-origin`.

Every other startup requirement is unchanged. A headless run still needs at least one output configured, and still needs a device unless it is in calibration mode.

## Streams

stdout carries the event stream and nothing else. Logs go to stderr, as they do in interactive runs, without the carriage return that raw mode requires.

A host that only wants supervision can read stdout line by line and ignore stderr entirely.

stdin is watched for end of file, and every byte arriving on it is discarded. It tells the engine its host is still there, and is never read as a command.

## Events

One JSON object per line. Every line carries `"v"`, the schema version, so a host can tell which contract it is supervising against.

### ready

Written once, after the engine has started and its transports are bound. It carries the facts a host cannot know in advance.

```json
{"v":1,"event":"ready","audio":{"device":"Duet 3","sample_rate":48000,"channels":[0,1]},"midi_device":null,"outputs":{"websocket":"127.0.0.1:53412","osc":null}}
```

| Field | Meaning |
| --- | --- |
| `audio.device` | The device the name match actually resolved to. `null` in calibration mode |
| `audio.sample_rate` | Hardware sample rate in Hz |
| `audio.channels` | Analysed channel indices in output order. The full set when no selection was given |
| `midi_device` | The resolved MIDI device name. `null` when MIDI is disabled or driven by the test clock |
| `outputs.websocket` | The address the listener actually bound. `null` when the WebSocket output is not configured |
| `outputs.osc` | The configured OSC target. `null` when the OSC output is not configured |

Because `outputs.websocket` reports the bound address rather than the requested one, a host can pass `--ws-addr 127.0.0.1:0` and read the real port back. This removes port collision handling from the host.

### error

Written once when startup or the run fails. The process then exits non-zero.

```json
{"v":1,"event":"error","code":"NoOutputConfigured","message":"To start the app, configure at least one output: --ws-addr <ADDR> or --osc-addr <ADDR>"}
```

`code` is a stable identifier taken from the originating error variant, so a host can act on it rather than parsing prose. A host should treat an unrecognised code as a generic failure and show `message`.

Codes come from the configuration and device error sets, for example `MissingDevice`, `NoOutputConfigured`, `NonLoopbackBindAddress`, `InvalidMaxClients`, `ChannelIndexOutOfRange`, `EmptyQuery`, `NoMatch` and `UnsupportedFormat`. A failure carrying no typed error reports `Unknown`. A panic reports `Panic`.

`HardwareStreamError` is written when the input stream fails during a run, for example when the interface is unplugged. The workers drain before the event is written.

Adding an error variant adds a code. Renaming one breaks this contract and is a breaking change.

### shutdown

Written once after the workers have drained, immediately before a clean exit.

```json
{"v":1,"event":"shutdown","reason":"signal"}
```

| Reason | Meaning |
| --- | --- |
| `signal` | SIGINT or SIGTERM was received |
| `stdin_closed` | stdin reached end of file, because the host closed it or exited |

A host should treat an unrecognised reason as a clean stop.

## Stopping and Exit Codes

SIGINT, SIGTERM and stdin reaching end of file all request the same graceful shutdown. Workers drain in their registration order, the `shutdown` event is written, and the process exits 0. When a signal and a closed stdin arrive together, the reason is `signal`.

A headless run started with stdin at `/dev/null` reads end of file at once. It writes `ready`, then shuts down with reason `stdin_closed`.

A startup failure, or a hardware stream error during the run, writes an `error` event and exits non-zero.

If the host has gone by the time the `shutdown` event is written, the write fails on a broken pipe. The workers have already drained, so the process still exits 0.

Interactive runs are unchanged. Without `--headless` Phase4 still requires a terminal, still puts it into raw mode, and still shuts down on Ctrl+C as a key event.

## Supervising Phase4

A host spawns the binary, reads the first line, and has everything it needs.

1. Spawn with `--headless`, stdin piped and held open, and stdout piped.
2. Read one line. On `ready`, connect to `outputs.websocket`. On `error`, report `code` and stop.
3. Read further lines in the background. A `shutdown` line means the engine stopped cleanly. An `error` line means the run failed.
4. To stop the engine, send SIGTERM or close stdin, then wait for the process to exit.

A host that exits for any reason, a crash or SIGKILL included, has its end of the stdin pipe closed by the operating system, and the engine drains and exits.

Settings are changed through the configuration file and the command line, then applied by restarting the process. There is no control channel, by design.
