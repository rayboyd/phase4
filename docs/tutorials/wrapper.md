# Embedding Phase4

Phase4 is designed to run as a child process of a wrapper application (a menu
bar app, a launcher, a plugin host) without the wrapper needing anything beyond
standard process and socket APIs. This guide documents the contract. It applies
to any language or framework that can spawn a process with piped stdio and open
a WebSocket.

## The contract at a glance

Spawn phase4 with `--controller-mode headless` and piped stdio. Keep the write
end of its stdin open for as long as phase4 should run. Read stderr as
line-oriented human logs. Consume analysis data over the WebSocket. Close stdin
to shut phase4 down.

Phase4 accepts no runtime commands on any channel, the wrapper's verbs are
start (spawn the process) and stop (close stdin), by design.

## Spawning

Pass `--controller-mode headless` explicitly. Phase4 never infers the mode from
its environment, a wrapper that omits the flag gets term mode, which enables
terminal raw mode and is wrong for a child process.

Pass `--config` with an absolute path so the configuration file is unambiguous
regardless of the child's working directory. Without `--config`, phase4 falls
back to reading an optional `config.yaml` from the current working directory,
not from the binary's location, so a wrapper relying on the fallback must set
the child's working directory explicitly.

## stdin, the lifetime pipe

Phase4 in headless mode blocks until its stdin reaches end of file, then shuts
down. The wrapper holds the write end open to keep phase4 running and closes it
to stop phase4. Any data written before the close is ignored. Because the pipe
also closes automatically if the wrapper process dies, phase4 cannot be
orphaned, no signal handling or process-tree cleanup is required on any
platform.

Shutdown is bounded internally. Phase4 joins each worker with its own timeout,
so after closing stdin the wrapper can expect the process to exit within a few
seconds even in a worst case. A wrapper may add its own kill timeout as a final
backstop, but should treat needing it as a bug worth reporting.

## stderr, the log pipe

All logging goes to stderr as plain lines in the form `[LEVEL] message`, with
no carriage returns in headless mode. Verbosity follows the `RUST_LOG`
environment variable, defaulting to `info`. These lines are for humans and for
the wrapper's log window, their exact wording is not a stable interface, do not
parse them to drive wrapper logic.

## stdout, structured events

Pass `--stdout-events json` to have phase4 emit NDJSON on stdout: one JSON
object per line, flushed per event. Without the flag, stdout stays exactly
as silent as it has always been (device-listing output is the only
exception, and a wrapper does not serve and list in the same invocation).

There are exactly two event types:

```json
{"v":1,"event":"ready","pid":34112,"ws_addr":"127.0.0.1:8889","osc_addr":null}
{"v":1,"event":"fatal","reason":"port_in_use","detail":"Failed to bind WebSocket server to 127.0.0.1:8889"}
```

Field rules:

- `v`: schema version integer, currently `1`. Evolution is additive only,
  readers should ignore unknown fields.
- `ready.ws_addr`: the address the WebSocket listener actually bound to,
  read from the listener itself rather than echoing back the configured
  value. This is what makes `--ws-addr 127.0.0.1:0` usable: bind a
  kernel-assigned port and read the real one back from this field. `null`
  when the WebSocket output is not configured.
- `ready.osc_addr`: the configured OSC target, `null` when absent.
- `ready.pid`: the process ID, matching `std::process::id()`.
- `fatal.reason`: a closed, snake_case enum, see the table below.
- `fatal.detail`: a human-readable error string. Informational only, its
  wording is not a stable interface, do not parse it.

Ordering contract:

1. `ready` is emitted exactly once, after every configured output is
   bound/started and before phase4 starts waiting for a shutdown signal (in
   headless mode, before it begins waiting on stdin to close).
2. On a startup failure, at most one `fatal` event is emitted, followed by a
   non-zero exit. `ready` and `fatal` are never both emitted.
3. A crash after `ready` emits nothing further. Phase4's release profile
   aborts on panic (see "Crash behaviour" below), so a crash has no `fatal`
   event, only the process exiting unexpectedly.

`fatal.reason` is one of:

| Reason | Meaning | Typical fix |
|---|---|---|
| `port_in_use` | The WebSocket bind address is already in use. | Free the port, or bind a different one (`127.0.0.1:0` sidesteps this entirely). |
| `device_not_found` | The requested audio device wasn't found, the query was empty, or the device's format is unsupported. | Check `--audio-list`, pick another device. |
| `device_unsupported` | Reserved for a future finer-grained split of `device_not_found` (not currently emitted; today all of the above map to `device_not_found`). | — |
| `invalid_config` | A CLI flag, a `config.yaml` value, or their combination failed validation. | Fix the offending flag or file value. |
| `no_output_configured` | Neither `--ws-addr` nor `--osc-addr` was set. | Configure at least one output. |
| `startup_failed` | Any other startup failure. | Read `detail` for a hint. Treat this as the default case for any reason you don't otherwise handle, the enum evolves additively and new phase4 versions may add reasons a wrapper doesn't yet know about. |

## Device discovery

Both listing commands have a machine-readable mode intended for wrappers:
`--audio-list --audio-list-format json` and `--midi-list --midi-list-format
json` each print a single JSON array on stdout, one object per device, and
nothing else is written to stdout in these modes. Log lines still go to
stderr, so the wrapper can parse stdout directly without filtering. A listing
invocation exits immediately after printing, run it as a separate short-lived
process before spawning the serving process.

Each audio entry carries `index`, `name`, `sample_rate`, `channels`,
`sample_format`, and `supported` (whether the device's default configuration
is the f32 format phase4 requires); the three configuration fields are `null`
when the hardware could not be queried. Each MIDI entry carries `index` and
`name`.

## Readiness and data

WebSocket output is opt-in, pass `--ws-addr` (or set `network.ws_addr` in
`config.yaml`) so the wrapper has an address to connect to.

With `--stdout-events json`, the `ready` event above is the primary readiness
mechanism: it's emitted only once every configured output is actually bound,
and `ready.ws_addr` carries the real bound address, which is what makes
`--ws-addr 127.0.0.1:0` a practical default for a wrapper: bind an
OS-assigned port and read it back from `ready`, sidestepping port conflicts
entirely. A `fatal` event with a closed `reason` classifies startup failures
(port in use, unknown device, invalid configuration, no output transport
configured) instead of leaving them as an undifferentiated non-zero exit
code with unstable log wording.

Without `--stdout-events`, or against an older phase4 binary that predates
it, fall back to polling a WebSocket connection to the configured address
after spawning, retrying briefly until it accepts. This fallback has a real
gap the event handshake fixes: a successful connect only proves "someone
serves on this port", not "my child serves on this port", if a foreign
process already holds the port, the child bind-fails and exits while the
poll happily connects to the stranger. Prefer the event handshake whenever
the wrapper can assume a recent enough binary.

On connect, phase4 immediately sends the current snapshot, so a client renders
without waiting for the next frame. Each message is a JSON object of the form
`{"channels":[{"peak":0.0,"bins":[...]}]}`, one entry per analysed channel,
with bin values in the range 0.0 to 1.0. Frames arrive at the configured
broadcast rate. OSC output, if configured, behaves identically to standalone
use and needs nothing from the wrapper.

## Crash behaviour

Phase4's release profile aborts on panic. The panic message is logged to
stderr through the standard format before the process terminates, so the
wrapper's crash signal is simply the child exiting unexpectedly, with the
reason available in the captured log lines.

## Minimal pseudocode

    child = spawn("phase4",
                  args: ["--audio-device", "Duet 3", "--ws-addr", "127.0.0.1:0",
                         "--stdout-events", "json", "--controller-mode", "headless"],
                  cwd: config_dir,
                  stdin: pipe, stdout: pipe, stderr: pipe)

    forward_lines(child.stderr, log_window)

    event = read_json_line(child.stdout)
    if event.event == "fatal":
        fail(event.reason, event.detail)
    ws = connect("ws://" + event.ws_addr)
    render_frames(ws)

    # later, to stop:
    close(child.stdin)
    wait(child)
