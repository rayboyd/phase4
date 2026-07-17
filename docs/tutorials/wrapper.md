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

Set the child's working directory explicitly. Phase4 reads `config.yaml` from
the current working directory, not from the binary's location, so the wrapper
controls which configuration is found by controlling where the child starts.

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

## stdout, reserved

Phase4 writes nothing to stdout while serving (device-listing output is the
exception, and a wrapper does not serve and list in the same invocation).
Structured machine-readable events may be added on stdout in a future release,
wrappers should leave the pipe connected and unread rather than closing it.

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
`config.yaml`) so the wrapper has an address to connect to. The listener is
bound eagerly during startup, before the headless controller begins waiting on
stdin, and startup failures (port in use, unknown device, invalid
configuration, no output transport configured) exit with a non-zero code and
an error line on stderr. The robust readiness check is therefore to poll a
WebSocket connection to the configured address after spawning, retrying
briefly until it accepts, rather than watching stderr for a particular line.

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
                  args: ["--audio-device", "Duet 3", "--ws-addr", "127.0.0.1:8889",
                         "--controller-mode", "headless"],
                  cwd: config_dir,
                  stdin: pipe, stdout: pipe, stderr: pipe)

    forward_lines(child.stderr, log_window)
    ws = connect_with_retry("ws://127.0.0.1:8889")
    render_frames(ws)

    # later, to stop:
    close(child.stdin)
    wait(child)
