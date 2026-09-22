# Phase4 XPC service

Branch: `feature/xpc-service`

## Summary

Phase4 gains an XPC service, `Phase4Engine.xpc`, for embedding in the Phase4 macOS app. The
service implements version 1 of the Phase4Engine link contract: XPC requests and replies for
control, XPC events for the engine's lifecycle, and a shared memory frame region that receives
every analysis snapshot together with the MIDI state.

The contract is `docs/link-contract.md` in the Phase4 app repository, committed at `9a69b07`. This
ticket copies it into this repository as `docs/xpc.md`.

The interactive and headless modes, the WebSocket and OSC outputs and every existing payload are
unchanged.

## Resolved design

The service is a separate binary, `phase4-xpc`, from `src/bin/phase4-xpc.rs`. launchd starts an
XPC service with no arguments, so it shares neither `main.rs` nor clap parsing. The binary calls
`phase4::xpc::run()`, which never returns.

The XPC code lives in the library under `src/xpc/`, so tests exercise the whole service in process
over an anonymous XPC listener. libxpc is called through hand-written `extern "C"` declarations in
`src/xpc/ffi.rs`. Event handlers are blocks built with `block2`. `libc` provides `mmap`, `munmap`
and `syslog`.

The XPC service, the frame region and the frame writer are macOS only, behind
`#[cfg(target_os = "macos")]`. On other platforms `phase4-xpc` prints that it runs only on macOS
and exits with status 1. CI runs on Linux, so the macOS tests run locally through the pre-push
hook.

The frame region is a new output transport, `OutputConfig::FrameRegion`. Bootstrap creates the
region once the channel count and sample rate are resolved, fills the output's
`FrameRegionSlot` with it, and spawns the frame writer. The frame writer publishes every
`RawPayload` snapshot, about every 10 ms, independent of the mapper's 60 Hz timer. The service
takes the region from the slot after the app is built.

The frame carries the MIDI transport through two new `AppState` atomics,
`midi_transport_count` and `midi_transport_latest`, which `record_byte` writes alongside
`midi_last_transport`. The mapper's read-and-clear of `midi_last_transport` is unchanged.

A `start` request becomes an `AppConfig` through the existing resolver. The request's
`config_yaml` is parsed as a `FileConfig` and its `network` section is cleared. The request's
source and MIDI are mapped onto an `Args` value, and the frame region output is passed in as an
extra output. The request therefore overrides the config exactly as the command line overrides a
config file, and every validation is the one already in use.

Error codes are the existing ones, recovered with `headless::error_code`, plus three link codes
from `xpc::LinkError`: `ContractMismatch`, `InvalidRequest` and `AlreadyRunning`.

The service logs through `syslog(3)` under the ident `Phase4Engine`, which the unified log
collects. A panic is logged the same way before the process aborts.

The service holds an XPC transaction for the length of each run, with `xpc_transaction_begin` on a
successful `start` and `xpc_transaction_end` when the run ends, so the system does not end it
while it streams. `XPC_TRANSACTION_DEPRECATED` expands to nothing in the public macOS 27 SDK.

The service serves one client. A second peer connection while one is active is cancelled. When the
client's connection becomes invalid, or the runtime reports termination is imminent, the run stops
without sending events.

`scripts/build-xpc.sh` builds `phase4-xpc` in release and assembles the unsigned bundle at
`target/xpc/Phase4Engine.xpc` from `resources/xpc/Info.plist`, with the version taken from
`Cargo.toml`. The Phase4 app signs it with its own entitlements when it embeds it.

## Step 0. Confirm the current code

Confirm each site before changing it. All were read at `9c1e24f` on `main`.

- `src/main.rs`. `run` at line 55 dispatches listing, headless and interactive modes.
  `phase4-xpc` does not touch it.
- `src/lib.rs`. Module declarations at lines 9 through 16. `Args` at line 162 flattens
  `CalibrationArgs` (line 36), `InputArgs` (line 57), `MidiArgs` (line 79), `NetworkArgs`
  (line 98) and `VocoderArgs` (line 127). Every field is `pub`, so an `Args` value can be built
  directly, as `config::types::test_support::args_with_device` does at line 372.
- `src/app.rs`. `AppState` at line 42 with `midi_last_transport` and `midi_steps`, and its
  `Default` at line 65. `App::build` at line 163 matches `OutputConfig` exhaustively at lines
  166 through 169. `run_headless` at line 230 handles `HeadlessStop::Engine` at lines 254
  through 258 using `ENGINE_STOPPED_MESSAGE` from line 37. `shutdown` at line 298 clears
  `keep_running` and joins workers.
- `src/bootstrap.rs`. The raw watch channel is created at line 118. `Mapper::spawn` at line 144
  takes `raw_rx` by value. `spawn_outputs` is called at line 162 and defined at line 200, where
  it matches `OutputConfig` exhaustively. `analyser_specs.sample_rate` holds the resolved sample
  rate.
- `src/config/types.rs`. Output names at lines 24 and 25. `OutputConfig` at line 76.
  `ConfigOutputs::new` at line 108 matches `OutputConfig` exhaustively. `FileConfig` at line 347.
  `test_support::websocket_output` at line 409 matches `OutputConfig` exhaustively.
- `src/config/resolve.rs`. `TryFrom<&Args>` at line 13. `load_file_config` at line 29 parses
  YAML at lines 39 through 42. `resolve_config` at line 50 builds the output set at lines 123
  through 139.
- `src/config/validate.rs`. `validate_app_config` at line 60 walks outputs with an `if let` at
  line 89, which a new variant does not affect.
- `src/config/mod.rs`. Public and crate re-exports at lines 13 through 19.
- `src/dsp/mod.rs`. `pub const BAND_COUNT: usize = 32` at line 13. `ChannelLevel` is
  re-exported from `payload`.
- `src/managers/midi.rs`. `MidiDeviceInfo` at line 30 and `MidiListener::enumerate_devices` at
  line 172 are private. Transport codes at lines 56 through 59. `record_byte` at line 86. Its
  tests start at line 405.
- `src/managers/audio.rs`. `DeviceInfo` at line 97 and `Input::enumerate_devices` at line 261 are
  private.
- `src/managers/mapper.rs`. `read_midi_snapshot` at line 109 swaps `midi_last_transport` to
  `MIDI_TRANSPORT_NONE`.
- `src/managers/mod.rs`. `spawn_async_worker` at line 46.
- `src/worker.rs`. `WorkerKind` at line 65 and `WorkerKind::spec` at line 75. Shutdown timeouts at
  lines 17 through 34.
- `src/headless.rs`. `Event::from_anyhow` at line 167 recovers codes from `AppConfigError`,
  `DeviceError` and `MidiDeviceError`, falling back to `Unknown`.
- `Cargo.toml` has neither `libc` nor `block2`. `Cargo.lock` already holds `libc` 0.2.189 and
  `block2` 0.6.2. `deny.toml` allows MIT and Apache-2.0, which covers both.
- The macOS 27 SDK's `xpc/xpc.h` declares `xpc_main` at line 2698, `xpc_transaction_begin` at
  line 2736 and `xpc_shmem_create` at line 1112, which requires memory from `mmap` with
  `MAP_SHARED`. `libc` does not declare `clock_gettime_nsec_np`.

Search for every other exhaustive match on `OutputConfig` and `WorkerKind` and add the new arms.

## New public surface

### Dependencies

```toml
[target.'cfg(target_os = "macos")'.dependencies]
block2 = "0.6.2"
libc = "0.2.189"
```

### `src/lib.rs`

```rust
#[cfg(target_os = "macos")]
pub mod frames;
#[cfg(target_os = "macos")]
pub mod xpc;
```

### `src/frames.rs`

```rust
//! The shared memory frame region of the Phase4Engine link contract, layout version 1.

use crate::dsp::{ChannelLevel, BAND_COUNT};
use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};

/// The ASCII bytes at the start of every frame region.
pub const FRAME_MAGIC: [u8; 4] = *b"P4FR";

/// The frame region layout version.
pub const FRAME_LAYOUT_VERSION: u32 = 1;

/// Bytes before the first channel record.
pub const FRAME_HEADER_BYTES: usize = 64;

/// Bytes in one channel record, the peak followed by every bin.
pub const CHANNEL_RECORD_BYTES: usize = (1 + BAND_COUNT) * 4;

/// Header field offsets in bytes.
pub const OFFSET_MAGIC: usize = 0;
pub const OFFSET_LAYOUT_VERSION: usize = 4;
pub const OFFSET_SEQUENCE: usize = 8;
pub const OFFSET_PUBLISHED_NS: usize = 16;
pub const OFFSET_CHANNEL_COUNT: usize = 24;
pub const OFFSET_BAND_COUNT: usize = 28;
pub const OFFSET_SAMPLE_RATE: usize = 32;
pub const OFFSET_MIDI_FLAGS: usize = 36;
pub const OFFSET_MIDI_STEPS: usize = 40;
pub const OFFSET_MIDI_TRANSPORT_COUNT: usize = 44;
pub const OFFSET_MIDI_TRANSPORT_LAST: usize = 48;

/// Bit 0 of `midi_flags`, set when MIDI input is configured.
pub const MIDI_FLAG_CONFIGURED: u32 = 1;

/// Read attempts a reader makes before it keeps its previous frame.
pub const READ_ATTEMPTS: usize = 3;

/// The MIDI state carried by one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameMidi {
    /// MIDI 1/16 note steps since the last Start, wrapping at 2^32.
    pub steps: u32,

    /// Start, Stop and Continue events received, wrapping at 2^32.
    pub transport_count: u32,

    /// The most recent transport event, 0 none, 1 Start, 2 Stop, 3 Continue.
    pub transport_last: u32,
}

/// The fields a region's creator writes once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub layout_version: u32,
    pub channel_count: u32,
    pub band_count: u32,
    pub sample_rate: u32,
    pub midi_flags: u32,
}

/// One frame copied out of a region by the sequence lock reader.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub sequence: u64,
    pub published_ns: u64,
    pub midi: FrameMidi,
    pub channels: Vec<ChannelLevel>,
}

/// Bytes the layout uses for `channel_count` channels.
#[must_use]
pub const fn frame_bytes(channel_count: usize) -> usize;

/// The current `CLOCK_UPTIME_RAW` time in nanoseconds.
#[must_use]
pub fn uptime_raw_ns() -> u64;

/// A frame region in page aligned anonymous `MAP_SHARED` memory, which
/// `xpc_shmem_create` accepts. One writer publishes into it, and it is
/// unmapped on drop.
#[derive(Debug)]
pub struct FrameRegion {
    base: NonNull<u8>,
    mapped_len: usize,
    channel_count: usize,
}

// The region is plain shared memory accessed only through atomics.
unsafe impl Send for FrameRegion {}
unsafe impl Sync for FrameRegion {}

impl FrameRegion {
    /// Maps a zeroed region of `frame_bytes(channel_count)` rounded up to a
    /// whole number of pages, then writes the fields the contract marks Once.
    /// `sequence` starts at zero.
    ///
    /// # Errors
    ///
    /// Returns the OS error when `mmap` fails.
    pub fn new(channel_count: usize, sample_rate: u32, midi_configured: bool) -> io::Result<Self>;

    /// Bytes the layout uses.
    #[must_use]
    pub fn frame_bytes(&self) -> usize;

    /// Bytes mapped, a whole number of pages.
    #[must_use]
    pub fn mapped_len(&self) -> usize;

    /// The start of the mapping, for `xpc_shmem_create`.
    #[must_use]
    pub fn as_mut_ptr(&self) -> *mut c_void;

    /// Publishes one frame with the sequence lock. Returns `false` and
    /// publishes nothing when any peak or bin is not finite. Only the frame
    /// writer thread calls this.
    ///
    /// # Panics
    ///
    /// Panics if `channels.len()` differs from the region's channel count.
    pub fn publish(&self, channels: &[ChannelLevel], midi: FrameMidi, published_ns: u64) -> bool;

    /// Reads the latest frame with the sequence lock.
    #[must_use]
    pub fn read(&self) -> Option<Frame>;
}

impl Drop for FrameRegion {
    fn drop(&mut self); // munmap
}

/// Reads the fields written once. Returns `None` when `len` is shorter than
/// the header or the magic does not match.
///
/// # Safety
///
/// `base` must point to at least `len` readable bytes that stay mapped for
/// the call.
#[must_use]
pub unsafe fn read_header(base: *const u8, len: usize) -> Option<FrameHeader>;

/// Reads the latest frame with the sequence lock. Returns `None` when the
/// header is invalid, `len` is too short for its channel count, no frame has
/// been published, or every one of `READ_ATTEMPTS` attempts overlapped a write.
///
/// # Safety
///
/// `base` must point to a frame region of at least `len` readable bytes,
/// aligned to 8 bytes, that stays mapped for the call.
#[must_use]
pub unsafe fn read_frame(base: *const u8, len: usize) -> Option<Frame>;

/// Carries a frame region from bootstrap, which creates it once the channel
/// count and sample rate are known, to the owner that shares it.
#[derive(Debug, Clone, Default)]
pub struct FrameRegionSlot(Arc<OnceLock<Arc<FrameRegion>>>);

impl PartialEq for FrameRegionSlot {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl FrameRegionSlot {
    #[must_use]
    pub fn new() -> Self;

    /// The region bootstrap created, or `None` before bootstrap has run.
    #[must_use]
    pub fn region(&self) -> Option<Arc<FrameRegion>>;

    /// Stores the region. Returns `false` and leaves the first region in
    /// place when the slot is already filled.
    pub(crate) fn fill(&self, region: Arc<FrameRegion>) -> bool;
}
```

`uptime_raw_ns` calls `clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW)` through this declaration in
`src/frames.rs`:

```rust
extern "C" {
    fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
}
```

Every header and payload word is accessed through `AtomicU32::from_ptr` and `AtomicU64::from_ptr`.
Floats are stored as their bits with `f32::to_bits` and read with `f32::from_bits`. All values are
little endian, which is the native order on every Mac the app supports.

`publish` follows the contract exactly. It stores `sequence + 1` with `Relaxed` ordering, issues
`fence(Release)`, writes `published_ns`, the three MIDI fields and every channel record with
`Relaxed` stores, then stores `sequence + 2` with `Release` ordering.

`read_frame` loads `sequence` with `Acquire`. A zero sequence returns `None`. An odd sequence counts
as a failed attempt. It then copies `published_ns`, the MIDI fields and every channel record with
`Relaxed` loads, issues `fence(Acquire)`, and loads `sequence` with `Relaxed`. A changed sequence
counts as a failed attempt. After `READ_ATTEMPTS` failed attempts it returns `None`.

### `src/managers/frame_writer.rs`

```rust
//! Publishes every analysis snapshot into the frame region.

/// Copies each `RawPayload` snapshot and the MIDI state into a frame region.
pub struct FrameWriter;

impl FrameWriter {
    /// Spawns the frame writer on a dedicated thread through
    /// `spawn_async_worker`.
    ///
    /// # Panics
    ///
    /// Panics if the OS thread cannot be spawned or its runtime cannot be built.
    pub fn spawn(
        raw_rx: tokio::sync::watch::Receiver<crate::dsp::RawPayload>,
        region: std::sync::Arc<crate::frames::FrameRegion>,
        state: std::sync::Arc<crate::app::AppState>,
    ) -> std::thread::JoinHandle<()>;
}
```

The writer loops while `keep_running` is set. It awaits `raw_rx.changed()` and exits when the
analyser's sender is gone. It skips an empty snapshot. It builds `FrameMidi` from `midi_steps`,
`midi_transport_count` and `midi_transport_latest`, each loaded with `Acquire`, and publishes with
`uptime_raw_ns()`. The first snapshot `publish` rejects in a run is logged once as a warning.

`src/managers/mod.rs` declares `#[cfg(target_os = "macos")] pub mod frame_writer;` and re-exports
`FrameWriter` under the same `cfg`.

### `src/xpc/mod.rs`

```rust
//! The Phase4Engine XPC service, implementing link contract version 1.

pub mod protocol;
mod ffi;
mod logging;
mod service;
#[cfg(test)]
mod tests;

use std::ffi::CStr;

/// The service's bundle identifier, which the client connects to by name.
pub const SERVICE_NAME: &str = "com.rayboyd.Phase4.Engine";

/// The link contract version carried as `v` in every message.
pub const CONTRACT_VERSION: i64 = 1;

/// The syslog ident the service logs under.
pub const LOG_IDENT: &CStr = c"Phase4Engine";

/// Installs the syslog logger and panic hook, then hands the process to the
/// XPC runtime. Called once from the `phase4-xpc` binary.
pub fn run() -> !;
```

### `src/xpc/protocol.rs`

```rust
//! Message types and keys of the link contract.

use std::ffi::CStr;

pub const KEY_VERSION: &CStr = c"v";
pub const KEY_TYPE: &CStr = c"type";
pub const KEY_OK: &CStr = c"ok";
pub const KEY_ERROR: &CStr = c"error";
pub const KEY_CODE: &CStr = c"code";
pub const KEY_MESSAGE: &CStr = c"message";
pub const KEY_ENGINE_VERSION: &CStr = c"engine_version";
pub const KEY_CONTRACT_VERSION: &CStr = c"contract_version";
pub const KEY_BAND_COUNT: &CStr = c"band_count";
pub const KEY_DEVICES: &CStr = c"devices";
pub const KEY_NAME: &CStr = c"name";
pub const KEY_SAMPLE_RATE: &CStr = c"sample_rate";
pub const KEY_CHANNELS: &CStr = c"channels";
pub const KEY_SUPPORTED: &CStr = c"supported";
pub const KEY_SOURCE: &CStr = c"source";
pub const KEY_KIND: &CStr = c"kind";
pub const KEY_DEVICE_NAME: &CStr = c"device_name";
pub const KEY_HZ: &CStr = c"hz";
pub const KEY_RATE_HZ: &CStr = c"rate_hz";
pub const KEY_MIDI: &CStr = c"midi";
pub const KEY_BPM: &CStr = c"bpm";
pub const KEY_CONFIG_YAML: &CStr = c"config_yaml";
pub const KEY_AUDIO_DEVICE: &CStr = c"audio_device";
pub const KEY_MIDI_DEVICE: &CStr = c"midi_device";
pub const KEY_FRAMES: &CStr = c"frames";
pub const KEY_FRAME_BYTES: &CStr = c"frame_bytes";
pub const KEY_REASON: &CStr = c"reason";

pub const TYPE_HELLO: &str = "hello";
pub const TYPE_LIST_AUDIO_DEVICES: &str = "list_audio_devices";
pub const TYPE_LIST_MIDI_DEVICES: &str = "list_midi_devices";
pub const TYPE_START: &str = "start";
pub const TYPE_STOP: &str = "stop";
pub const EVENT_ERROR: &str = "error";
pub const EVENT_STOPPED: &str = "stopped";

pub const SOURCE_DEVICE: &str = "device";
pub const SOURCE_TEST_TONE: &str = "test_tone";
pub const SOURCE_TEST_SWEEP: &str = "test_sweep";
pub const MIDI_DEVICE: &str = "device";
pub const MIDI_TEST_CLOCK: &str = "test_clock";

pub const REASON_REQUESTED: &str = "requested";
pub const REASON_FAILED: &str = "failed";

/// A parsed request.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Hello,
    ListAudioDevices,
    ListMidiDevices,
    Start(StartRequest),
    Stop,
}

/// The fields of a `start` request.
#[derive(Debug, Clone, PartialEq)]
pub struct StartRequest {
    pub source: Source,
    pub midi: Option<MidiRequest>,
    pub config_yaml: Option<String>,
}

/// What to analyse.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Device { name: String, channels: Option<Vec<u16>> },
    TestTone { hz: f32 },
    TestSweep { rate_hz: f32 },
}

/// The MIDI input to attach.
#[derive(Debug, Clone, PartialEq)]
pub enum MidiRequest {
    Device { name: String },
    TestClock { bpm: f32 },
}

/// Failures the link itself reports. Variant names are the contract's codes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("This engine speaks link contract version {supported}, and the request used version {requested}")]
    ContractMismatch { requested: i64, supported: i64 },

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("The engine is already running. Send stop before start.")]
    AlreadyRunning,
}

impl crate::headless::EventCode for LinkError {
    fn event_code(&self) -> &'static str; // the variant name
}

/// Parses a request message.
pub(crate) fn parse_request(message: super::ffi::DictionaryRef<'_>) -> Result<Request, LinkError>;

/// Resolves a `start` request into an `AppConfig` whose outputs are exactly
/// one `OutputConfig::FrameRegion` carrying `slot`.
pub(crate) fn to_app_config(
    start: &StartRequest,
    slot: &crate::frames::FrameRegionSlot,
) -> Result<crate::config::AppConfig, crate::config::AppConfigError>;
```

`parse_request` returns `ContractMismatch` when `v` is an int64 other than `CONTRACT_VERSION`, for
every request type. It returns `InvalidRequest`, naming the key, when:

- the message is not a dictionary
- `v` or `type` is missing or has another XPC type
- `type` is not one of the five request types
- `source` is missing or not a dictionary, or its `kind` is unknown
- a field a `kind` requires is missing or has another XPC type
- `channels` is not an array of int64 values from 0 to 65,535
- `midi` is present and not a dictionary, or its `kind` is unknown
- `config_yaml` is present and not a string

`hz`, `rate_hz` and `bpm` must be doubles and are narrowed to `f32`. Their ranges are checked by the
existing validation during resolution, not by `parse_request`.

`to_app_config` parses `config_yaml` with `config::parse_file_config`, or uses
`FileConfig::default()` when it is absent. It replaces `file.network` with
`FileNetworkConfig::default()`. It builds an `Args` value with `config: None`, `headless: false`,
both list flags false with `ListFormat::Text`, every network and vocoder field `None` or false, and
these fields from the request:

| Request | `Args` field |
| --- | --- |
| `Source::Device { name, channels }` | `input.audio_device = Some(name)`, `input.audio_analyse_channels = channels` |
| `Source::TestTone { hz }` | `calibration.test_hz = Some(hz)` |
| `Source::TestSweep { rate_hz }` | `calibration.test_sweep = Some(rate_hz)` |
| `MidiRequest::Device { name }` | `midi.midi_device = Some(name)` |
| `MidiRequest::TestClock { bpm }` | `calibration.test_midi_clock = Some(bpm)` |

It then calls `config::resolve_with_outputs(&args, file, vec![OutputConfig::FrameRegion(slot.clone())])`.

### `src/xpc/ffi.rs`

Crate-private. It declares the libxpc functions below, which libSystem provides, and wraps them.
Functions that create objects return a retained reference. `xpc_dictionary_get_value` and
`xpc_array_get_value` return borrowed references.

```rust
#![allow(non_camel_case_types, non_upper_case_globals)]

use block2::Block;
use std::ffi::{c_char, c_void, CStr};

pub(crate) type xpc_object_t = *mut c_void;
pub(crate) type xpc_connection_t = xpc_object_t;
pub(crate) type xpc_type_t = *const c_void;
pub(crate) type xpc_handler_t = Block<dyn Fn(xpc_object_t)>;

/// The index `xpc_array_set_int64` treats as append.
pub(crate) const XPC_ARRAY_APPEND: usize = usize::MAX;

extern "C" {
    pub(crate) static _xpc_type_dictionary: u8;
    pub(crate) static _xpc_type_array: u8;
    pub(crate) static _xpc_type_string: u8;
    pub(crate) static _xpc_type_int64: u8;
    pub(crate) static _xpc_type_double: u8;
    pub(crate) static _xpc_type_bool: u8;
    pub(crate) static _xpc_type_shmem: u8;
    pub(crate) static _xpc_type_error: u8;
    pub(crate) static _xpc_type_connection: u8;
    pub(crate) static _xpc_error_connection_invalid: u8;
    pub(crate) static _xpc_error_connection_interrupted: u8;
    pub(crate) static _xpc_error_termination_imminent: u8;

    pub(crate) fn xpc_main(handler: extern "C" fn(xpc_connection_t)) -> !;
    pub(crate) fn xpc_connection_create(name: *const c_char, targetq: *mut c_void) -> xpc_connection_t;
    pub(crate) fn xpc_connection_create_from_endpoint(endpoint: xpc_object_t) -> xpc_connection_t;
    pub(crate) fn xpc_endpoint_create(connection: xpc_connection_t) -> xpc_object_t;
    pub(crate) fn xpc_connection_set_event_handler(connection: xpc_connection_t, handler: &xpc_handler_t);
    pub(crate) fn xpc_connection_resume(connection: xpc_connection_t);
    pub(crate) fn xpc_connection_cancel(connection: xpc_connection_t);
    pub(crate) fn xpc_connection_send_message(connection: xpc_connection_t, message: xpc_object_t);
    pub(crate) fn xpc_connection_send_message_with_reply_sync(connection: xpc_connection_t, message: xpc_object_t) -> xpc_object_t;
    pub(crate) fn xpc_get_type(object: xpc_object_t) -> xpc_type_t;
    pub(crate) fn xpc_retain(object: xpc_object_t) -> xpc_object_t;
    pub(crate) fn xpc_release(object: xpc_object_t);
    pub(crate) fn xpc_dictionary_create(keys: *const *const c_char, values: *const xpc_object_t, count: usize) -> xpc_object_t;
    pub(crate) fn xpc_dictionary_create_reply(original: xpc_object_t) -> xpc_object_t;
    pub(crate) fn xpc_dictionary_get_value(dictionary: xpc_object_t, key: *const c_char) -> xpc_object_t;
    pub(crate) fn xpc_dictionary_set_value(dictionary: xpc_object_t, key: *const c_char, value: xpc_object_t);
    pub(crate) fn xpc_dictionary_set_int64(dictionary: xpc_object_t, key: *const c_char, value: i64);
    pub(crate) fn xpc_dictionary_set_double(dictionary: xpc_object_t, key: *const c_char, value: f64);
    pub(crate) fn xpc_dictionary_set_bool(dictionary: xpc_object_t, key: *const c_char, value: bool);
    pub(crate) fn xpc_dictionary_set_string(dictionary: xpc_object_t, key: *const c_char, string: *const c_char);
    pub(crate) fn xpc_int64_get_value(object: xpc_object_t) -> i64;
    pub(crate) fn xpc_double_get_value(object: xpc_object_t) -> f64;
    pub(crate) fn xpc_bool_get_value(object: xpc_object_t) -> bool;
    pub(crate) fn xpc_string_get_string_ptr(object: xpc_object_t) -> *const c_char;
    pub(crate) fn xpc_array_create(objects: *const xpc_object_t, count: usize) -> xpc_object_t;
    pub(crate) fn xpc_array_append_value(array: xpc_object_t, value: xpc_object_t);
    pub(crate) fn xpc_array_set_int64(array: xpc_object_t, index: usize, value: i64);
    pub(crate) fn xpc_array_get_count(array: xpc_object_t) -> usize;
    pub(crate) fn xpc_array_get_value(array: xpc_object_t, index: usize) -> xpc_object_t;
    pub(crate) fn xpc_shmem_create(region: *mut c_void, length: usize) -> xpc_object_t;
    pub(crate) fn xpc_shmem_map(xshmem: xpc_object_t, region: *mut *mut c_void) -> usize;
    pub(crate) fn xpc_transaction_begin();
    pub(crate) fn xpc_transaction_end();
}

/// The XPC types the service distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Dictionary,
    Array,
    String,
    Int64,
    Double,
    Bool,
    Shmem,
    Error,
    Connection,
    Other,
}

/// The type of `object`, compared by address against the `_xpc_type_*` symbols.
pub(crate) fn kind(object: xpc_object_t) -> Kind;

/// An owned XPC object reference, released on drop.
#[derive(Debug)]
pub(crate) struct Owned(xpc_object_t);

// XPC objects are reference counted and thread safe to retain, release and send.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Owned {
    /// Takes ownership of a retained reference. `None` for a null pointer.
    pub(crate) unsafe fn from_retained(object: xpc_object_t) -> Option<Self>;

    /// Retains a borrowed reference. `None` for a null pointer.
    pub(crate) unsafe fn retain(object: xpc_object_t) -> Option<Self>;

    pub(crate) fn as_ptr(&self) -> xpc_object_t;

    /// A typed view when this object is a dictionary.
    pub(crate) fn as_dictionary(&self) -> Option<DictionaryRef<'_>>;
}

impl Drop for Owned {
    fn drop(&mut self); // xpc_release
}

/// A borrowed dictionary with type checked reads. Each getter returns `None`
/// when the key is missing or holds another XPC type.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DictionaryRef<'a> {
    pointer: xpc_object_t,
    _owner: std::marker::PhantomData<&'a Owned>,
}

impl<'a> DictionaryRef<'a> {
    /// A view of `object` when it is a dictionary.
    pub(crate) unsafe fn from_ptr(object: xpc_object_t) -> Option<Self>;
    pub(crate) fn contains(&self, key: &CStr) -> bool;
    pub(crate) fn get_i64(&self, key: &CStr) -> Option<i64>;
    pub(crate) fn get_f64(&self, key: &CStr) -> Option<f64>;
    pub(crate) fn get_bool(&self, key: &CStr) -> Option<bool>;
    /// `None` also when the string is not valid UTF-8.
    pub(crate) fn get_string(&self, key: &CStr) -> Option<String>;
    pub(crate) fn get_dictionary(&self, key: &CStr) -> Option<DictionaryRef<'a>>;
    pub(crate) fn get_array(&self, key: &CStr) -> Option<ArrayRef<'a>>;
    pub(crate) fn get_value(&self, key: &CStr) -> Option<xpc_object_t>;
}

/// A borrowed array with type checked reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArrayRef<'a> {
    pointer: xpc_object_t,
    _owner: std::marker::PhantomData<&'a Owned>,
}

impl<'a> ArrayRef<'a> {
    pub(crate) fn len(&self) -> usize;
    pub(crate) fn get_i64(&self, index: usize) -> Option<i64>;
    pub(crate) fn get_dictionary(&self, index: usize) -> Option<DictionaryRef<'a>>;
}

/// A dictionary under construction, created empty or as a reply.
pub(crate) struct DictionaryBuilder(Owned);

impl DictionaryBuilder {
    pub(crate) fn new() -> Self;
    /// Creates a reply to `message`. `None` when `message` expects no reply.
    pub(crate) fn reply_to(message: DictionaryRef<'_>) -> Option<Self>;
    pub(crate) fn set_i64(&mut self, key: &CStr, value: i64) -> &mut Self;
    pub(crate) fn set_f64(&mut self, key: &CStr, value: f64) -> &mut Self;
    pub(crate) fn set_bool(&mut self, key: &CStr, value: bool) -> &mut Self;
    /// Interior NUL bytes are replaced with U+FFFD.
    pub(crate) fn set_str(&mut self, key: &CStr, value: &str) -> &mut Self;
    pub(crate) fn set_value(&mut self, key: &CStr, value: &Owned) -> &mut Self;
    pub(crate) fn into_owned(self) -> Owned;
}

/// An array under construction.
pub(crate) struct ArrayBuilder(Owned);

impl ArrayBuilder {
    pub(crate) fn new() -> Self;
    pub(crate) fn push_i64(&mut self, value: i64) -> &mut Self;
    pub(crate) fn push(&mut self, value: &Owned) -> &mut Self;
    pub(crate) fn into_owned(self) -> Owned;
}

/// Sends `message` on `connection`.
pub(crate) fn send(connection: &Owned, message: &Owned);

/// Sets `handler` as the connection's event handler, then resumes it.
pub(crate) fn activate(connection: &Owned, handler: impl Fn(xpc_object_t) + Send + Sync + 'static);
```

`activate` wraps the closure in `block2::RcBlock::new`, passes `&*block` to
`xpc_connection_set_event_handler`, which copies the block, then calls `xpc_connection_resume`.

### `src/xpc/service.rs`

```rust
//! Serves one client over XPC and owns the engine run.

use crate::app::App;
use crate::frames::FrameRegion;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// How often the monitor checks whether the engine stopped itself.
const ENGINE_POLL_INTERVAL_MS: u64 = 50;

/// The service state shared by the peer connection's handler and the monitor.
pub(crate) struct Service {
    peer: Mutex<Option<super::ffi::Owned>>,
    run: Mutex<Option<Running>>,
}

/// One engine run.
struct Running {
    app: App,
    region: Arc<FrameRegion>,
    monitor_stop: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
}

impl Service {
    pub(crate) fn new() -> Arc<Self>;

    /// The process-wide service `xpc_main` hands peers to.
    pub(crate) fn shared() -> &'static Arc<Self>;

    /// Accepts a peer connection, or cancels it when a peer is already active.
    pub(crate) fn accept_peer(self: &Arc<Self>, connection: super::ffi::xpc_connection_t);

    /// Whether a run is active.
    pub(crate) fn is_running(&self) -> bool;
}

/// The handler `xpc::run` passes to `xpc_main`.
pub(crate) extern "C" fn accept_peer_connection(connection: super::ffi::xpc_connection_t);
```

### `src/xpc/logging.rs`

```rust
/// A `log::Log` that writes every record through `syslog(3)`.
pub(crate) struct SyslogLogger;

impl log::Log for SyslogLogger { /* enabled, log, flush */ }

/// Opens syslog under `LOG_IDENT`, installs `SyslogLogger` at `Info`, and
/// installs a panic hook that logs the panic at `LOG_ERR`.
pub(crate) fn install();
```

`install` calls `libc::openlog(LOG_IDENT.as_ptr(), libc::LOG_PID, libc::LOG_USER)`. Levels map
`Error` to `LOG_ERR`, `Warn` to `LOG_WARNING`, `Info` to `LOG_INFO`, and `Debug` and `Trace` to
`LOG_DEBUG`. Each message is passed as the argument of a `"%s"` format, with interior NUL bytes
replaced.

### `src/bin/phase4-xpc.rs`

```rust
//! The Phase4Engine XPC service entry point.

#[cfg(target_os = "macos")]
fn main() {
    phase4::xpc::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("phase4-xpc runs only on macOS");
    std::process::exit(1);
}
```

### `resources/xpc/Info.plist`

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleExecutable</key>
	<string>Phase4Engine</string>
	<key>CFBundleIdentifier</key>
	<string>com.rayboyd.Phase4.Engine</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>Phase4Engine</string>
	<key>CFBundlePackageType</key>
	<string>XPC!</string>
	<key>CFBundleShortVersionString</key>
	<string>0.0.0</string>
	<key>CFBundleVersion</key>
	<string>0.0.0</string>
	<key>LSMinimumSystemVersion</key>
	<string>27.0</string>
	<key>XPCService</key>
	<dict>
		<key>ServiceType</key>
		<string>Application</string>
	</dict>
</dict>
</plist>
```

### `scripts/build-xpc.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail

# Builds Phase4Engine.xpc, the unsigned XPC service bundle the Phase4 app embeds and signs.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE="$ROOT/target/xpc/Phase4Engine.xpc"
PLIST="$BUNDLE/Contents/Info.plist"
VERSION="$(cargo pkgid --manifest-path "$ROOT/Cargo.toml" | sed 's/.*@//')"

cargo build --release --bin phase4-xpc --manifest-path "$ROOT/Cargo.toml"

rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS"
cp "$ROOT/resources/xpc/Info.plist" "$PLIST"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" -c "Set :CFBundleVersion $VERSION" "$PLIST"
cp "$ROOT/target/release/phase4-xpc" "$BUNDLE/Contents/MacOS/Phase4Engine"
plutil -lint "$PLIST"

echo "$BUNDLE"
```

## API changes other code depends on

These change existing signatures or public types. Update every caller.

- `OutputConfig` gains `#[cfg(target_os = "macos")] FrameRegion(crate::frames::FrameRegionSlot)`.
  It is a public enum without `#[non_exhaustive]`, so every exhaustive match gains a `cfg` arm:
  - `App::build` at `src/app.rs` line 166 returns `None` for it.
  - `spawn_outputs` in `src/bootstrap.rs` creates the region, as described in Behaviour.
  - `ConfigOutputs::new` in `src/config/types.rs` treats it as a transport named `Frame region`,
    through a new `FRAME_REGION_OUTPUT_NAME` constant, and rejects a repeat with
    `DuplicateOutputTransport`.
  - `test_support::websocket_output` returns `None` for it.
- `AppState` gains two public fields, which a struct literal must now set.
  ```rust
  /// Start, Stop and Continue events received, wrapping on overflow. Written
  /// by the MIDI callback or synthetic clock and never cleared.
  pub midi_transport_count: AtomicU32,

  /// The most recent transport event, one of the `MIDI_TRANSPORT_*` codes.
  /// Written with `midi_last_transport` and never cleared by the mapper.
  pub midi_transport_latest: AtomicU8,
  ```
  `Default` initialises them to `0` and `MIDI_TRANSPORT_NONE`.
- `App` gains a public method.
  ```rust
  /// Why the engine stopped itself, for a host that polls instead of
  /// blocking. `None` while `keep_running` is set. Call only before
  /// `shutdown`, which also clears `keep_running`.
  #[must_use]
  pub fn engine_failure(&self) -> Option<anyhow::Error>;
  ```
  It returns `DeviceError::HardwareStreamError` when an error was recorded, and otherwise
  `ENGINE_STOPPED_MESSAGE`. The `HeadlessStop::Engine` arm of `run_headless` returns the error
  `engine_failure` gives, so the logic exists once.
- `headless` gains a public function, and `Event::from_anyhow` uses it for its code.
  ```rust
  /// The stable code of the typed error in `error`'s chain, or `Unknown`.
  #[must_use]
  pub fn error_code(error: &anyhow::Error) -> &'static str;
  ```
- `config` gains two crate functions, re-exported from `src/config/mod.rs` as `pub(crate)`.
  ```rust
  /// Parses YAML text as a `FileConfig`, rejecting unknown keys.
  pub(crate) fn parse_file_config(text: &str) -> Result<FileConfig, AppConfigError>;

  /// Merges the CLI, file and default layers and validates the result, with
  /// `extra_outputs` added to the outputs the layers name.
  pub(crate) fn resolve_with_outputs(
      args: &Args,
      file: FileConfig,
      extra_outputs: Vec<OutputConfig>,
  ) -> Result<AppConfig, AppConfigError>;
  ```
  `resolve_config` is renamed to `resolve_with_outputs`, and `TryFrom<&Args>` passes an empty
  vector. `load_file_config` calls `parse_file_config`, keeping its log line and errors.
- `spawn_outputs` in `src/bootstrap.rs` gains `raw_rx: &watch::Receiver<RawPayload>` and
  `sample_rate: u32` parameters. Bootstrap clones `raw_rx` before `Mapper::spawn` consumes it.
- `WorkerKind` gains `#[cfg(target_os = "macos")] FrameWriter`, named `frame-writer`, with the
  success line `- Frame writer shutdown complete` and a new `FRAME_WRITER_SHUTDOWN_TIMEOUT_MS`
  of 1,000.
- `DeviceInfo`, `Input::enumerate_devices`, `MidiDeviceInfo` and
  `MidiListener::enumerate_devices` become `pub(crate)`, with `pub(crate)` fields.

## Behaviour

### Frame region output

`spawn_outputs` handles `OutputConfig::FrameRegion(slot)` by creating
`Arc::new(FrameRegion::new(display_channels, sample_rate, midi_enabled)?)`, with the error
wrapped as `Failed to map the frame region`. It fills `slot` and bails with
`The frame region slot was already filled` if `fill` returns `false`. It spawns
`FrameWriter::spawn(raw_rx.clone(), region, Arc::clone(state))`, logs
`Frame region publishing {display_channels} channels`, and registers the handle as
`WorkerKind::FrameWriter`. The output starts no network transport.

### MIDI

`record_byte` stores the code in `midi_transport_latest` and adds one to `midi_transport_count`
with `fetch_add` and `AcqRel` ordering for every Start, Stop and Continue, next to its existing
`midi_last_transport` store. Timing clock bytes do not touch either field.

### Connections

`xpc::run` calls `logging::install`, then
`xpc_main(service::accept_peer_connection)`. `accept_peer_connection` passes each new connection
to `Service::shared().accept_peer`.

`accept_peer` cancels the connection when a peer is already stored. Otherwise it retains and stores
it as the peer and activates it with a handler that dispatches each event:

- A dictionary is a request, handled as below.
- `XPC_ERROR_CONNECTION_INVALID` or `XPC_ERROR_TERMINATION_IMMINENT` stops any run without sending
  events and clears the peer.
- Any other event is logged at `debug` and ignored.

Every reply sets `v` to `CONTRACT_VERSION` and `ok`. A failure sets `ok` to `false` and `error` to
a dictionary with `code` and `message`. A `LinkError` uses its `EventCode` and `Display`. An
`AppConfigError` uses its `EventCode` and `Display`. An `anyhow::Error` uses
`headless::error_code` and `format!("{error:#}")`.

### Requests

- **hello** replies `engine_version` set to `env!("CARGO_PKG_VERSION")`, `contract_version` set
  to `CONTRACT_VERSION` and `band_count` set to `BAND_COUNT`.
- **list_audio_devices** replies `devices`, an array holding one dictionary per
  `Input::enumerate_devices` entry, in order: `name`, `sample_rate` and `channels` as int64 when
  present, and `supported`.
- **list_midi_devices** replies `devices`, an array holding one dictionary with `name` per
  `MidiListener::enumerate_devices` entry, in order.
- **start**
  1. Replies `AlreadyRunning` when a run is active.
  2. Creates a `FrameRegionSlot`, then calls `to_app_config` and `App::new_headless`. A failure
     replies with its code and nothing is kept.
  3. Takes the region from the slot. If the slot is empty, it calls `App::shutdown` and replies
     `Unknown` with the message `The frame region was not created`.
  4. Replies `sample_rate`, `channels` and, when present, `audio_device` and `midi_device` from
     `App::ready_report`, with `frames` set to
     `xpc_shmem_create(region.as_mut_ptr(), region.mapped_len())` and `frame_bytes` set to
     `region.frame_bytes()`.
  5. Calls `xpc_transaction_begin`, starts the monitor and stores the run.
- **stop** replies `ok` at once when no run is active and sends no event. Otherwise it takes the
  run out of the mutex, releases the lock, sets `monitor_stop`, joins the monitor, calls
  `App::shutdown`, replies `ok`, sends `stopped` with reason `requested` and calls
  `xpc_transaction_end`.

### Monitor

The monitor thread exits when `monitor_stop` is set. Otherwise, every `ENGINE_POLL_INTERVAL_MS`, it
takes the run lock. When the run is gone because `stop` took it, the monitor exits without sending
anything. When `App::engine_failure` returns an error, the monitor takes the run out of the mutex,
releases the lock, sends `error` with the failure's code and message, calls `App::shutdown`, sends
`stopped` with reason `failed` and calls `xpc_transaction_end`. It never joins itself.

Taking the run out of the mutex before joining the monitor means `stop` never holds the lock while
it waits, so the two paths cannot deadlock. Exactly one of them ends each run, and
`xpc_transaction_end` is called once per run.

### Events

Events are dictionaries with `v` and `type`, sent with `xpc_connection_send_message` on the stored
peer. With no peer stored, nothing is sent.

## Steps

1. Write the frame region tests in `src/frames.rs`.
   - Every offset constant, `FRAME_HEADER_BYTES`, `CHANNEL_RECORD_BYTES` and `frame_bytes(2) == 328`
     match the contract.
   - A new region has the magic, layout version, channel count, band count, sample rate and MIDI
     flag, a zero sequence and a page multiple `mapped_len` of at least `frame_bytes`.
   - `read` returns `None` before the first publish.
   - Two publishes leave the sequence at 2, then 4.
   - A published frame reads back with the same channels, MIDI and `published_ns`.
   - A non-finite peak, and separately a non-finite bin, makes `publish` return `false` and leaves
     the sequence unchanged.
   - With the sequence forced odd through the raw pointer, `read` returns `None` after
     `READ_ATTEMPTS` attempts.
   - `read_header` returns `None` for a wrong magic and for a length shorter than the header.
   - A writer thread publishing 10,000 frames, each with every value equal to its frame number,
     against a reader thread that checks every frame it reads holds one number throughout.
   - `FrameRegionSlot::fill` stores the first region and returns `false` for a second.
2. Write the MIDI tests next to the existing `record_byte` tests: Start, Stop and Continue each add
   one to `midi_transport_count` and set `midi_transport_latest`. Timing clock bytes change
   neither. `read_midi_snapshot` clears `midi_last_transport` and leaves `midi_transport_latest`.
3. Write the config tests in `src/config/resolve.rs`: `parse_file_config` rejects malformed YAML
   and unknown keys with `ConfigFileParseError`, and `resolve_with_outputs` with an empty vector
   matches the old `TryFrom` results.
4. Write the protocol tests in `src/xpc/protocol.rs`, building XPC dictionaries in process with
   `DictionaryBuilder`.
   - Each request type parses, including every `Source` and `MidiRequest` kind and `channels`.
   - Each rule under `parse_request` returns `InvalidRequest`, and `v` of 2 returns
     `ContractMismatch` for `hello` and for `start`.
   - `to_app_config`:
     - a tone gives `ConfigInput::Calibration(TestSignal::FixedTone)`
     - a device with channels gives sorted `analyse_channels`
     - the request's device overrides `audio.device_name_match` in the YAML
     - a test clock gives `ConfigMidiInput::TestClock`
     - YAML with `network.ws_addr` resolves with no WebSocket output
     - the outputs are exactly one `FrameRegion` equal to the given slot
     - `vocoder.attack_ms: 0` returns `InvalidAttackTime`
5. Write the service tests in `src/xpc/tests.rs`. Each test creates an anonymous listener with
   `xpc_connection_create(null, null)`, activates it with a handler that passes each peer to a fresh
   `Service::new().accept_peer`, and connects a client through `xpc_endpoint_create` and
   `xpc_connection_create_from_endpoint`. The client handler forwards events to an `mpsc` channel.
   Requests use `xpc_connection_send_message_with_reply_sync`.
   - `hello` replies `ok`, the package version, contract version 1 and band count 32.
   - `v` of 2 replies `ContractMismatch`. An unknown type replies `InvalidRequest`.
   - `list_audio_devices` and `list_midi_devices` reply a `devices` array whose entries have the
     documented keys and types. The arrays may be empty.
   - `start` with a 440 Hz tone:
     - replies `sample_rate` 44,100, `channels` `[0, 1]`, no `audio_device`, `frame_bytes` 328 and
       a shared memory `frames`
     - `xpc_shmem_map` maps it, and `read_header` reports layout 1, 2 channels, 32 bands, 44,100 Hz
       and no MIDI flag
     - within 2 seconds `read_frame` returns an even sequence of at least 2, a `published_ns` no
       later than `uptime_raw_ns()` and finite values
     - a second read 100 ms later has a higher sequence
   - `start` while running replies `AlreadyRunning`.
   - `stop` replies `ok`, a `stopped` event with reason `requested` arrives within 2 seconds, and
     `is_running` is `false`.
   - `stop` while idle replies `ok`, and no event arrives within 200 ms.
   - `start` with a tone and a 300 BPM test clock sets the MIDI flag, and within 1 second
     `midi_steps` is above 0, `midi_transport_count` is at least 1 and `midi_transport_last` is 1.
   - `config_yaml` of `vocoder: [` replies `ConfigFileParseError`. `vocoder.attack_ms: 0` replies
     `InvalidAttackTime`. `network.ws_addr` set replies `ok`.
   - A device source with `channels` `[]` replies `EmptyChannelSelection`. A device named
     `Phase4 Test No Such Device` replies `NoMatch`.
   - A second client's first request receives an XPC error object in place of a reply.
   - Cancelling the client during a run makes `is_running` `false` within 2 seconds.
6. Add `libc` and `block2` under the macOS target dependencies.
7. Write `src/frames.rs`.
8. Add the `AppState` fields and the `record_byte` stores.
9. Add `parse_file_config` and `resolve_with_outputs`, and route `TryFrom` and `load_file_config`
   through them.
10. Add `OutputConfig::FrameRegion`, `FRAME_REGION_OUTPUT_NAME`, `WorkerKind::FrameWriter`,
    `FrameWriter` and the `spawn_outputs` arm, and update every exhaustive match.
11. Add `headless::error_code` and `App::engine_failure`, and route `Event::from_anyhow` and
    `run_headless` through them.
12. Make the device listings `pub(crate)`.
13. Write `src/xpc/ffi.rs`, `src/xpc/logging.rs`, `src/xpc/protocol.rs`, `src/xpc/service.rs` and
    `src/xpc/mod.rs`, then `src/bin/phase4-xpc.rs`.
14. Add `resources/xpc/Info.plist` and `scripts/build-xpc.sh`, and make the script executable.
    Run it and confirm it prints the bundle path and `plutil -lint` passes.
15. Write `docs/xpc.md`: the link contract text from the app repository, followed by building the
    bundle, logging with `log stream --predicate 'process == "Phase4Engine"'` and running the
    macOS tests. Add the XPC service to the modes in `README.md` with a link to `docs/xpc.md`.
16. Run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings` and
    `cargo test` on macOS. CI on Linux confirms the non-macOS build after the branch is pushed.

## Done criteria

- [ ] `scripts/build-xpc.sh` produces `target/xpc/Phase4Engine.xpc` with
      `Contents/MacOS/Phase4Engine`, and an `Info.plist` that passes `plutil -lint` and carries
      the package version.
- [ ] Over an in-process XPC connection, `hello`, both device listings, `start`, `stop` and every
      error case reply exactly as the link contract states.
- [ ] A `start` reply's `frames` maps in the client with the documented header, and the sequence
      advances with even values while the run is live.
- [ ] Every frame a concurrent reader accepts is internally consistent under a writer publishing
      10,000 frames.
- [ ] A snapshot with a non-finite value is never published.
- [ ] MIDI steps, transport count and latest transport reach the frame from a test clock, and the
      WebSocket and OSC MIDI output is unchanged.
- [ ] A `stop` produces one `stopped` event with reason `requested`. An idle `stop` produces none.
- [ ] A second client is refused, and the first client's run continues.
- [ ] The run stops when the client's connection is cancelled.
- [ ] Config from `config_yaml` is validated with the existing codes, the request overrides the
      YAML's audio and MIDI sections, and the `network` section starts no transport.
- [ ] `phase4-xpc` on a non-macOS target prints that it runs only on macOS and exits 1, and the
      crate builds there without the XPC, frames or frame writer code.
- [ ] The interactive and headless modes, their events and exit codes, and the WebSocket and OSC
      payloads are unchanged, and the existing tests pass unmodified apart from the `AppState`
      and `resolve_config` changes above.

## Commit message

```text
feat(xpc): add the Phase4Engine XPC service

Run Phase4 as an XPC service for the Phase4 macOS app, implementing
version 1 of the link contract. The phase4-xpc binary answers hello,
device listing, start and stop over XPC, sends error and stopped events,
and shares a frame region the engine writes every analysis snapshot into
under a sequence lock, with the MIDI step count and transport state.

The frame region is a new output transport, so a start request resolves
through the existing config layers and validation. MIDI transport gains
a running count and a latest value that the mapper never clears. The
service logs to the unified log and holds a transaction while it runs.

scripts/build-xpc.sh assembles the unsigned Phase4Engine.xpc bundle for
the app to embed and sign. The XPC service and frame region build on
macOS only. Interactive and headless modes, both network outputs and
every existing payload are unchanged.
```
