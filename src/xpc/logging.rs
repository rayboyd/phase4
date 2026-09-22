//! Logging for the XPC service. launchd gives an XPC service no terminal, so
//! records go through `syslog(3)`, which the unified log collects under the
//! service's ident.

use super::LOG_IDENT;
use std::ffi::CString;

/// The character that replaces an interior NUL byte in a log message.
const NUL_REPLACEMENT: &str = "\u{FFFD}";

/// The most detailed level the service logs.
const MAX_LEVEL: log::LevelFilter = log::LevelFilter::Info;

/// A `log::Log` that writes every record through `syslog(3)`.
pub(crate) struct SyslogLogger;

static LOGGER: SyslogLogger = SyslogLogger;

impl log::Log for SyslogLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= MAX_LEVEL
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let priority = match record.level() {
            log::Level::Error => libc::LOG_ERR,
            log::Level::Warn => libc::LOG_WARNING,
            log::Level::Info => libc::LOG_INFO,
            log::Level::Debug | log::Level::Trace => libc::LOG_DEBUG,
        };
        let text = record.args().to_string().replace('\0', NUL_REPLACEMENT);
        let Ok(message) = CString::new(text) else {
            return;
        };
        // SAFETY: the message is passed as the argument of a "%s" format, so
        // no user text is ever read as a format string.
        unsafe { libc::syslog(priority, c"%s".as_ptr(), message.as_ptr()) };
    }

    fn flush(&self) {}
}

/// Opens syslog under `LOG_IDENT`, installs `SyslogLogger` at `Info`, and
/// installs a panic hook that logs the panic at `LOG_ERR`.
pub(crate) fn install() {
    // SAFETY: LOG_IDENT is a static C string, so the pointer openlog keeps
    // stays valid for the life of the process.
    unsafe { libc::openlog(LOG_IDENT.as_ptr(), libc::LOG_PID, libc::LOG_USER) };
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(MAX_LEVEL);
    }
    std::panic::set_hook(Box::new(|info| log::error!("Panic: {info}")));
}
