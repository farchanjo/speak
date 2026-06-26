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
/// Env var overriding the rotated-file retention count.
pub const ENV_RETENTION: &str = "SPEAK_LOG_RETENTION";
/// Default number of rotated files to keep (overridable via [`ENV_RETENTION`]).
const DEFAULT_RETENTION: usize = 7;

/// Resolve the rotated-file retention from `SPEAK_LOG_RETENTION`, else default.
#[must_use]
pub fn retention() -> usize {
    std::env::var(ENV_RETENTION)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_RETENTION)
}

/// Resolve the log directory (`SPEAK_LOG_DIR` or `~/.speak/logs`).
#[must_use]
pub fn log_dir() -> PathBuf {
    crate::paths::log_dir()
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
        .max_log_files(retention())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::ENV_LOCK;

    fn with_retention<T>(value: Option<&str>, body: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(ENV_RETENTION).ok();
        match value {
            // SAFETY: env mutation is serialised on ENV_LOCK across all tests.
            Some(v) => unsafe { std::env::set_var(ENV_RETENTION, v) },
            None => unsafe { std::env::remove_var(ENV_RETENTION) },
        }
        let out = body();
        match prev {
            Some(v) => unsafe { std::env::set_var(ENV_RETENTION, v) },
            None => unsafe { std::env::remove_var(ENV_RETENTION) },
        }
        out
    }

    #[test]
    fn retention_defaults_when_unset() {
        with_retention(None, || assert_eq!(retention(), DEFAULT_RETENTION));
    }

    #[test]
    fn retention_honours_env_override() {
        with_retention(Some("3"), || assert_eq!(retention(), 3));
    }

    #[test]
    fn retention_ignores_zero_and_garbage() {
        with_retention(Some("0"), || assert_eq!(retention(), DEFAULT_RETENTION));
        with_retention(Some("nope"), || assert_eq!(retention(), DEFAULT_RETENTION));
    }
}
