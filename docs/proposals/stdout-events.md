# Implement: structured stdout events (`--stdout-events json`)

Status: approved, land on main before tagging 0.1.0. This brief is
self-contained — no prior conversation context is needed. Read it fully
before writing code. All file:line references were verified against the
repo at commit f261d62 (v0.0.11).

## Why

The wrapper contract (docs/tutorials/wrapper.md) reserves stdout for future
"structured machine-readable events" (wrapper.md:57). The macOS wrapper
(`~/Source/github/phase4-macos`, context only — do not modify it) exposed
why they're needed:

1. **Readiness is inferred, not announced.** A wrapper polls the WebSocket
   port, but a successful connect proves "someone serves on this port", not
   "*my child* serves on this port". If a foreign process already holds the
   port, the child bind-fails and exits while the poll happily connects to
   the stranger. Correct behaviour for a socket-serving process is to
   announce its own readiness.
2. **Startup failures are unclassifiable.** Exit code 1 covers port-in-use,
   unknown device, and invalid config alike; log wording is explicitly
   non-contractual (wrapper.md:49-51), so wrappers legally cannot tell them
   apart.

This change fixes both, stays strictly opt-in (stdout remains byte-for-byte
silent without the flag), and unlocks `--ws-addr 127.0.0.1:0`
(kernel-assigned port; conflicts become impossible for wrappers).

## The contract being added

With `--stdout-events json`, phase4 emits NDJSON on stdout: one JSON object
per line, flushed per event. Exactly two event types:

```json
{"v":1,"event":"ready","pid":34112,"ws_addr":"127.0.0.1:8889","osc_addr":null}
{"v":1,"event":"fatal","reason":"port_in_use","detail":"Failed to bind WebSocket server to 127.0.0.1:8889"}
```

Field rules:
- `v`: schema version integer, `1`. Additive evolution only; readers ignore
  unknown fields.
- `ready.ws_addr`: the **actually bound** address obtained from the
  listener (`local_addr()`), not the configured one — this is what makes
  port 0 usable. `null` when the WS output is not configured.
- `ready.osc_addr`: the configured OSC target, `null` when absent.
- `ready.pid`: `std::process::id()`.
- `fatal.reason`: closed enum, snake_case (mapping table below).
- `fatal.detail`: the human error string (`format!("{e}")` /
  `format!("{e:#}")` for anyhow chains). Informational; wording NOT stable.

Ordering contract (this is the substance — enforce it and test it):
1. `ready` is emitted exactly once, after **all** outputs are bound/started
   and before the controller blocks (headless: before
   `HeadlessController::run` waits on stdin, src/controller.rs:146).
2. On startup failure: at most one `fatal`, then exit non-zero. Never both
   a `ready` and a `fatal`. A crash after `ready` emits nothing (release
   profile aborts on panic; the wrapper's crash signal remains unexpected
   exit, wrapper.md:94-99).
3. Without the flag, stdout stays exactly as silent as today, in every path.

## Implementation steps

Zero new dependencies: serde + serde_json are already direct deps
(Cargo.toml:41-42). Work through these in order; each step compiles.

### 1. CLI flag

`NetworkArgs` in src/lib.rs (the group at lib.rs:108-137, next to
`no_browser_origin` at lib.rs:130):

```rust
/// Emit structured machine-readable events on stdout (for wrapper
/// processes). Currently the only format is json: one JSON object per
/// line. See docs/tutorials/wrapper.md.
#[arg(long, value_enum)]
pub stdout_events: Option<EventFormat>,
```

with `EventFormat` as a `#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum EventFormat { Json }` alongside the existing `ListFormat` /
`ControllerMode` enums in lib.rs. The flag is **CLI-only** — do NOT add it
to `FileConfig`. Same policy and same reason as `no_browser_origin`; put a
comment mirroring resolve.rs:67-70. It is valid in both controller modes
(term mode stdout is unused, so nothing conflicts) and valid without
`--ws-addr` (an OSC-only run still gets a `ready` with `"ws_addr":null`).

Note this flag does NOT go through `AppConfig`/`resolve_config` at all —
it's read directly off `Args` in main.rs (step 4). Config resolution can
fail, and `fatal` must still be emittable for those failures, so the
emitter must exist before resolution runs.

### 2. Event module

New file `src/events.rs`, exported from lib.rs (`pub mod events;`):

```rust
use serde::Serialize;
use std::net::SocketAddr;

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready { pid: u32, ws_addr: Option<SocketAddr>, osc_addr: Option<SocketAddr> },
    Fatal { reason: FatalReason, detail: String },
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FatalReason {
    PortInUse,
    DeviceNotFound,
    DeviceUnsupported,
    InvalidConfig,
    NoOutputConfigured,
    StartupFailed,
}

pub struct Emitter { enabled: bool }
```

`Emitter::emit(&self, event: &Event)` is a no-op when disabled; when
enabled it serializes with the `v` field injected (either wrap in
`#[derive(Serialize)] struct Envelope { v: u32, #[serde(flatten)] event:
&Event }` or add `v` manually), writes one line to
`std::io::stdout().lock()` with `writeln!`, and flushes. Serialization of
these types cannot fail in practice; `expect` with a clear message is
acceptable, matching the codebase's style of panicking only on programmer
error.

`serde(flatten)` + `tag` interplay can be fiddly — lock the exact output
shape with the serialization tests in step 6 first (`{"v":1,"event":
"ready",...}` all on one line) and adjust the derive strategy to match.

### 3. Surface the bound WS address

- `Server::spawn` (src/managers/server.rs:217-244) binds eagerly at
  server.rs:224. Capture `std_listener.local_addr()?` after the bind and
  change the return type from `Result<JoinHandle<()>>` to
  `Result<(SocketAddr, JoinHandle<()>)>`.
- `spawn_outputs` (src/bootstrap.rs:150+, WS arm at bootstrap.rs:161-170)
  destructures it, and the existing log line at bootstrap.rs:168 switches
  from the configured `addr` to the bound address (so `--ws-addr :0` logs
  the real port for humans too). Return the bound addr out of
  `spawn_outputs` alongside the thread list.
- Add `ws_bound_addr: Option<SocketAddr>` to `Bootstrapped`
  (src/bootstrap.rs:44-58), populate it in `bootstrap()`
  (bootstrap.rs:129-140).
- Add the same field to `App` (src/app.rs:67-83) with a public accessor
  `pub fn ws_bound_addr(&self) -> Option<SocketAddr>`, populated in
  `App::new` (app.rs:97-108).
- `osc_addr` for the ready event: read it from `AppConfig`'s outputs before
  the config is moved into `App::new` (main.rs:57), or expose it from `App`
  the same way — whichever is cleaner given `ConfigOutputs`
  (src/config/types.rs:66-76).

### 4. Emit in main.rs

`main()` is at src/main.rs:23-61. After the list-mode early returns
(main.rs:39-47), construct the emitter from
`args.network.stdout_events`. Then:

- Config resolution failure (the `Err` arm at main.rs:49-55): emit
  `Fatal { reason: map_config_error(&e), detail: format!("{e}") }` before
  the existing `log::error!` + `exit(1)`.
- `App::new(config)` failure (currently `?` at main.rs:57 — change to a
  match): map the anyhow chain (helper below), emit `Fatal`, log, exit(1).
- Success: emit `Ready { pid: std::process::id(), ws_addr:
  app.ws_bound_addr(), osc_addr }` **before** `app.run_until_shutdown()`.

Mapping helpers live in events.rs (testable, pure):

```rust
pub fn map_config_error(e: &AppConfigError) -> FatalReason
pub fn map_startup_error(e: &anyhow::Error) -> FatalReason
```

### 5. Reason mapping

`map_config_error` (`AppConfigError`, src/config/types.rs:122-198):

| Variant | Reason |
|---|---|
| `MissingDevice` | `device_not_found` |
| `NoOutputConfigured` | `no_output_configured` |
| `NonLoopbackBindAddress`, `ConfigFileNotFound`, `ConfigFileParseError`, `EmptyChannelSelection`, `ChannelIndexOutOfRange`, `InvalidMaxClients`, all `Invalid*` vocoder/rate/tempo variants | `invalid_config` |

`map_startup_error` (anyhow from `App::new` → bootstrap):
- walk the chain (`e.chain()`) for an `std::io::Error` with
  `ErrorKind::AddrInUse` → `port_in_use`
- if the chain contains an `AppConfigError` (bootstrap re-validates:
  `ChannelIndexOutOfRange` at bootstrap.rs:210, Nyquist checks at
  bootstrap.rs:82) → reuse `map_config_error`
- device open/resolution failures: inspect what
  `resolve_audio_hardware`/`Input::get_device` (src/managers/audio.rs)
  actually return — if a typed error distinguishes not-found vs non-F32,
  map to `device_not_found` / `device_unsupported`; if it's stringly, use
  `device_not_found` for the resolve path and don't force it
- anything else → `startup_failed`

Do not over-fit: the closed enum plus `startup_failed` fallback IS the
contract. Wrappers must treat unknown reasons as `startup_failed` anyway.

The enum granularity rule (from the 0.1.x error-message work this feeds):
a reason earns a slot only if the user's *fix* differs. `port_in_use` →
"free the port / change it"; `device_not_found` → "plug it in / pick
another"; `invalid_config` → "fix the file/flags".

### 6. Port 0

With step 3 done, verify `--ws-addr 127.0.0.1:0` end to end: the loopback
validation (resolve.rs:145 → `validate_bind_addr`, types.rs) checks the IP
only, so port 0 should already pass — confirm, and add the integration
test. `ready.ws_addr` must carry the real port.

### 7. Tests

Unit (in events.rs `#[cfg(test)]` and config tests where fitting):
- Exact serialization: `ready` with/without ws_addr, `fatal` — assert the
  full JSON string including `"v":1` and single-line framing.
- `map_config_error` covers every `AppConfigError` variant (exhaustive
  match, no `_` arm, so new variants force a mapping decision).
- `map_startup_error` AddrInUse detection through an anyhow chain.

Integration (new file `tests/stdout_events.rs`, following the existing
patterns in tests/ — spawn the real binary with
`env!("CARGO_BIN_EXE_phase4")` and `std::process::Command`):
1. **Ready + port 0**: spawn `--stdout-events json --test-hz 440
   --ws-addr 127.0.0.1:0 --controller-mode headless` with piped stdio.
   First stdout line parses as `ready` with port != 0; TCP-connect to that
   port succeeds; close stdin; process exits 0; no further stdout lines.
   (`--test-hz` needs no audio hardware — safe in CI.)
2. **Fatal port_in_use**: bind a `std::net::TcpListener` on an ephemeral
   port first, spawn phase4 against that addr; single stdout line with
   `"reason":"port_in_use"`; exit non-zero; no `ready`.
3. **Fatal invalid_config**: `--config /nonexistent.yaml` →
   `"reason":"invalid_config"`.
4. **Silence without the flag**: repeat 1 and 2 without `--stdout-events`;
   stdout is empty (zero bytes) in both.
Use generous timeouts (existing tests show the house style); always close
stdin/kill in test cleanup so failures don't leak processes.

### 8. Documentation

- **docs/tutorials/wrapper.md**: rewrite the "stdout, reserved" section
  (wrapper.md:53-58) to document the flag, both events, field rules, and
  the ordering contract. Rewrite "Readiness and data" (wrapper.md:76-92) to
  make the event handshake the primary readiness mechanism, keep WS-polling
  as the documented fallback for wrappers targeting older binaries, and
  mention port 0. Update the pseudocode block (wrapper.md:101-115).
- **README.md**: add the flag to the CLI/options documentation alongside
  the other network flags; one sentence + pointer to wrapper.md.
- **example.config.yaml**: add `--stdout-events` to the CLI-only note
  (example.config.yaml:49-51, where `--no-browser-origin` is listed).

## Verification before declaring done

```sh
cargo fmt --check
cargo clippy --all-targets   # [lints.clippy] in Cargo.toml is strict; fix, don't allow
cargo test                   # unit + integration, all green
# manual smoke:
cargo build --release
./target/release/phase4 --stdout-events json --test-hz 440 \
  --ws-addr 127.0.0.1:0 --controller-mode headless < /dev/null | head -1
# → single ready line with a real port, then clean exit (stdin is EOF)
./target/release/phase4 --test-hz 440 --ws-addr 127.0.0.1:8889 \
  --controller-mode headless < /dev/null | wc -c   # → 0 bytes
```

## Guardrails

- stderr logging: untouched. Not one format or wording change.
- Without the flag, stdout behaviour is byte-identical to v0.0.11.
- stdin stays lifetime-only; nothing reads it. No new channels.
- No new dependencies. No changes to the DSP/hot path, payload shapes, WS
  wire format, or OSC.
- Device-list modes (`--audio-list`/`--midi-list`) are unaffected; they
  exit before the emitter matters, keep it that way.
- Conventional-commit style per git log (e.g. `feat(events): ...`). Do NOT
  hand-edit CHANGELOG.md (it's generated by the release chore) and do NOT
  bump the version — both belong to the release process.
- Do not commit unless asked; leave the work reviewed-and-staged with a
  suggested commit message (repo owner commits).
- Delete this file as part of the change once wrapper.md documents the
  contract (the proposal's content graduates into the docs).

## Out of scope

The Swift wrapper's consumption of these events (Phase4Kit event-based
probe, typed failure surfacing, port-0 default) is a separate task in the
phase4-macos repo. Nothing in this task touches that repo.
