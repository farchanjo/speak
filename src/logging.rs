//! Diagnostics logging (ADR-0002 / ADR-0009): a rotating `~/.speak/logs` file
//! plus an optional console (stderr) layer gated by verbosity.
//!
//! DIAGNOSTICS (trace/debug/info/warn/error) ride `tracing` and are kept apart
//! from command RESULTS, which flow through the `Presenter` port to stdout.
//!
//! - File layer (ALWAYS, unless disabled): level from `SPEAK_LOG` (e.g. `info`,
//!   `debug`, `speak=debug`); default `info`. `SPEAK_LOG=off` disables it.
//!   Directory `SPEAK_LOG_DIR`, default `~/.speak/logs`; files rotate daily and
//!   are capped (retention 7); output is non-blocking (a background thread).
//! - Console layer (stderr): emitted only when `-v`/`--verbose` is given (or
//!   `RUST_LOG` is set). The verbosity count maps `1 => info`, `2 => debug`,
//!   `3+ => trace` (deps pinned to `warn`); `RUST_LOG` overrides the derivation.

use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

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

/// Initialise diagnostics logging: the rotating file layer plus, when `verbose`
/// is non-zero (or `RUST_LOG` is set), a stderr console layer. The returned guard
/// keeps the file writer thread alive for the program's lifetime; `None` means no
/// file layer is active (logging disabled, the file path was unavailable, or only
/// the console layer is running).
#[must_use]
pub fn init(verbose: u8) -> Option<WorkerGuard> {
    let level = std::env::var(ENV_LEVEL).ok();
    let (file_layer, guard) = match level.as_deref() {
        Some("off") => (None, None),
        _ => build_file_layer(level),
    };
    let console = console_filter(verbose).map(|filter| {
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .with_filter(filter)
    });
    if file_layer.is_none() && console.is_none() {
        return None;
    }
    tracing_subscriber::registry()
        .with(file_layer)
        .with(console)
        .try_init()
        .ok()?;
    guard
}

/// Build the rotating-file layer + its worker guard, or `(None, None)` on failure.
type FileLayer = tracing_subscriber::filter::Filtered<
    fmt::Layer<
        tracing_subscriber::Registry,
        fmt::format::DefaultFields,
        fmt::format::Format,
        tracing_appender::non_blocking::NonBlocking,
    >,
    EnvFilter,
    tracing_subscriber::Registry,
>;

fn build_file_layer(level: Option<String>) -> (Option<FileLayer>, Option<WorkerGuard>) {
    let dir = log_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return (None, None);
    }
    let Ok(appender) = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("speak")
        .filename_suffix("log")
        .max_log_files(retention())
        .build(&dir)
    else {
        return (None, None);
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = level
        .and_then(|l| EnvFilter::try_new(l).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    let layer = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .with_filter(filter);
    (Some(layer), Some(guard))
}

/// The stderr filter: `RUST_LOG` when set, else a verbosity-derived directive
/// (`None` when no console diagnostics were requested).
fn console_filter(verbose: u8) -> Option<EnvFilter> {
    if let Ok(spec) = std::env::var("RUST_LOG")
        && !spec.is_empty()
        && let Ok(filter) = EnvFilter::try_new(&spec)
    {
        return Some(filter);
    }
    if verbose == 0 {
        return None;
    }
    let level = match verbose {
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    Some(EnvFilter::new(format!("warn,speak={level}")))
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

    #[test]
    fn console_filter_absent_without_verbose_or_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("RUST_LOG").ok();
        // SAFETY: env mutation serialised on ENV_LOCK across all tests.
        unsafe { std::env::remove_var("RUST_LOG") };
        assert!(console_filter(0).is_none());
        assert!(console_filter(1).is_some(), "verbose enables the console");
        if let Some(v) = prev {
            unsafe { std::env::set_var("RUST_LOG", v) };
        }
    }

    #[test]
    fn console_filter_honours_rust_log_over_verbose() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("RUST_LOG").ok();
        // SAFETY: env mutation serialised on ENV_LOCK across all tests.
        unsafe { std::env::set_var("RUST_LOG", "speak=trace") };
        assert!(
            console_filter(0).is_some(),
            "RUST_LOG forces a console layer"
        );
        match prev {
            Some(v) => unsafe { std::env::set_var("RUST_LOG", v) },
            None => unsafe { std::env::remove_var("RUST_LOG") },
        }
    }
}
