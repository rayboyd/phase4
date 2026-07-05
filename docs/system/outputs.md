# Output Transport Design

Status: accepted, targeted at 0.0.8. Nothing below is implemented yet.

## Context

WebSocket output is always on with a default address, while OSC output is opt-in
via a target address. The asymmetry is historical, WebSocket was the first
transport and inherited always-on status. Phase4 is a consumer tool, outputs
should exist because the user named them, matching the explicit-over-inferred
principle used elsewhere (controller mode, calibration flags). A raw binary UDP
bridge for C++ and game engine consumers is a definite future transport, so the
design must extend without churn.

## Decision

Both transports become opt-in. The resolved configuration carries a non-empty
collection of per-transport descriptors, built exactly once in `resolve_config`.

    /// One configured output transport, carrying everything that
    /// transport needs.
    pub enum OutputConfig {
        WebSocket {
            addr: SocketAddr,
            max_clients: usize,
            no_browser_origin: bool,
        },
        Osc {
            addr: SocketAddr,
        },
    }

    /// The resolved output set, non-empty by construction.
    pub struct ConfigOutputs(Vec<OutputConfig>);

The constructor rejects an empty collection with a new
`AppConfigError::NoOutputConfigured`, reported at startup with a non-zero exit,
alongside the existing `MissingDevice` precedent. The check cannot live in clap,
either address may arrive from `config.yaml` alone, so it belongs after the
three-tier merge.

The WebSocket flag is renamed `--ws-addr` with config key `network.ws_addr`.
Flags name what comes out of the socket (WebSocket JSON, OSC messages), never
the mechanism, since every transport is a socket and more than one is UDP.
`default_bind_addr()` and the embedded 8889 default are removed from code, the
commented `example.config.yaml` becomes the surface that teaches defaults, with
the network fields present but blank.

## Direction asymmetry

`ws_addr` is a listen address, phase4 binds it and clients connect in.
`osc_addr` is a target address, phase4 sends to it. The names stay a uniform
`-addr` family, and direction is stated where consumers actually read, the flag
help strings (listens on, sends to) and the example file comments.

## Alternatives considered

An enum over transport combinations (WebSocket, Osc, Both) makes zero outputs
unrepresentable and gives downstream code a compile-time proof that at least one
transport exists, but scales combinatorially, a third transport forces seven
variants or a redesign. Rejected because the raw bridge is definite.

A struct with two private optional address fields and a validated constructor
scales linearly but repeats per-transport fields, requires the emptiness check
to be amended for every new transport (a forgotten amendment wrongly rejects
valid configs, loud but avoidable), and leaves WebSocket-only settings as loose
siblings on `AppConfig`. Rejected in favour of descriptors.

A builder was considered for the construction ergonomics and remains available
as surface styling over the descriptor design, it does not change the invariant.

## Why descriptors

The emptiness check is structural, collection emptiness never needs amending
when a transport is added. Extension is one enum variant and one spawn arm,
touching nothing that exists. `App::new` collapses to a single loop matching
each descriptor to its spawn. Transport-specific settings live inside their
variant instead of loose on `AppConfig`. The config layer ends up saying what
the runtime already says, outputs are a homogeneous set of subscribers on the
`DisplayPayload` watch channel.

## Costs accepted

Duplicate variants become representable, a collection could hold two WebSocket
entries. Named field access is gone, finding a specific transport is a
find-and-match, though the only consumer is the spawn loop, which iterates
anyway. `resolve_config` assembles descriptors rather than passing options
through.

## Open questions for 0.0.8

Duplicate-variant rule, reject in the constructor (discriminant comparison) or
document last-wins. Exact help string and example comment wording for the
listen against target direction. Whether a builder fronts the constructor.

## Consequences beyond the code

Breaking change to CLI semantics and the config schema, lands before any
consumer packaging exists. The bare quickstart command grows a flag or moves to
`config.yaml`, which is the intended consumer path. The startup lifecycle
diagram gains conditional branches for both spawns. README, calibration
tutorial, and `lifecycle.md` updates ride along in the same cycle.

## Future transports

The raw binary UDP bridge (working name `--raw-addr`, one datagram per frame,
small header then packed little-endian f32s) is post-0.1.0 and is the reason
this design was chosen. It arrives as one `OutputConfig` variant, one spawn
arm, and a datagram specification document.
