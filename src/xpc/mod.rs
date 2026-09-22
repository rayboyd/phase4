//! The `Phase4Engine` XPC service, implementing link contract version 1.
//!
//! The Phase4 macOS app embeds `Phase4Engine.xpc` and talks to it through
//! XPC requests and replies for control, XPC events for the engine's
//! lifecycle, and a shared memory frame region for every analysis snapshot.
//! See `docs/xpc.md`.

mod ffi;
mod logging;
pub mod protocol;
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
pub fn run() -> ! {
    logging::install();
    log::info!(
        "Phase4Engine {} starting as {SERVICE_NAME}",
        env!("CARGO_PKG_VERSION")
    );
    // SAFETY: xpc_main takes a plain function pointer, sets up the service's
    // listener and never returns.
    unsafe { ffi::xpc_main(service::accept_peer_connection) }
}
