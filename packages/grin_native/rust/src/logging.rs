//! Minimal stderr logger and `grin_init_logs`.

use std::io::Write;
use std::os::raw::c_char;
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

use crate::error::{read_opt_str, GrinError, GrinFfiResult};

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        // A failed write to stderr has nowhere better to go.
        let _ = writeln!(
            std::io::stderr().lock(),
            "{ts} {:<5} {}: {}",
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

static LOGGER: StderrLogger = StderrLogger;
static INIT: Once = Once::new();

fn parse_level(level: Option<&str>) -> Result<LevelFilter, GrinError> {
    match level.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("info") => Ok(LevelFilter::Info),
        Some("trace") => Ok(LevelFilter::Trace),
        Some("debug") => Ok(LevelFilter::Debug),
        Some("warn") | Some("warning") => Ok(LevelFilter::Warn),
        Some("error") => Ok(LevelFilter::Error),
        Some("off") => Ok(LevelFilter::Off),
        Some(other) => Err(GrinError::invalid_argument(format!(
            "log_level must be trace, debug, info, warn, error or off, got {other:?}"
        ))),
    }
}

/// Installs the logger and a panic hook that logs panics. The first call
/// installs them; later calls only change the level. If the host process
/// already installed a `log` logger, that one keeps receiving records.
fn init(level: LevelFilter) {
    INIT.call_once(|| {
        // An error means the host already set a logger; the level below
        // still applies to it.
        let _ = log::set_logger(&LOGGER);
        std::panic::set_hook(Box::new(|info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "unknown location".to_owned());
            let msg = crate::error::panic_message(info.payload());
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("unnamed");
            log::error!("thread '{name}' panicked at {location}: {msg}");
        }));
    });
    log::set_max_level(level);
}

/// Sets up logging. `log_level` may be NULL (info) or one of trace, debug,
/// info, warn, error, off. Safe to call more than once.
// # Safety
// Every pointer argument must be NULL or point to a NUL-terminated string
// that stays valid and unchanged until the call returns.
#[no_mangle]
pub unsafe extern "C" fn grin_init_logs(log_level: *const c_char) -> GrinFfiResult {
    ffi_guard! {
        let level = parse_level(read_opt_str(&log_level, "log_level")?)?;
        init(level);
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(parse_level(None).unwrap(), LevelFilter::Info);
        assert_eq!(parse_level(Some("DEBUG")).unwrap(), LevelFilter::Debug);
        assert!(parse_level(Some("loud")).is_err());
    }
}
