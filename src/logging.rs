//! Rotating file logging under `~/.speak/logs`, controlled entirely by env.
//!
//! - Level: `SPEAK_LOG` (e.g. `info`, `debug`, or `speak=debug`); default
//!   `info`. `SPEAK_LOG=off` disables file logging entirely.
//! - Directory: `SPEAK_LOG_DIR`, default `~/.speak/logs`.
//! - Files rotate daily and are capped (retention 7) so the directory never
//!   grows unbounded. Output is non-blocking (a background writer thread).

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

/// Env var selecting the log level / filter.
pub const ENV_LEVEL: &str = "SPEAK_LOG";
/// Env var overriding the log directory.
pub const ENV_DIR: &str = "SPEAK_LOG_DIR";
/// How many rotated files to keep.
const RETENTION: usize = 7;

/// Resolve the log directory (`SPEAK_LOG_DIR` or `~/.speak/logs`).
#[must_use]
pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ENV_DIR).filter(|s| !s.is_empty()) {
        return PathBuf::from(dir);
    }
    speak_home().join("logs")
}

/// The `speak` project home (`~/.speak`).
#[must_use]
pub fn speak_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".speak")
}

/// Initialise rotating file logging. The returned guard must be kept alive for
/// the program's lifetime; `None` means logging is disabled or unavailable.
#[must_use]
pub fn init() -> Option<WorkerGuard> {
    let level = std::env::var(ENV_LEVEL).ok();
    if level.as_deref() == Some("off") {
        return None;
    }
    let dir = log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("speak")
        .filename_suffix("log")
        .max_log_files(RETENTION)
        .build(&dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = level
        .and_then(|l| EnvFilter::try_new(l).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(false)
        .try_init()
        .ok()?;
    Some(guard)
}
