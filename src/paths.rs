//! Filesystem locations for `speak`, unified under `~/.speak`.
//!
//! Overridable via env: `SPEAK_HOME`, `SPEAK_CONFIG`, `SPEAK_LOG_DIR`,
//! `SPEAK_DAEMON_SOCKET`. The legacy `~/.config/speak/config.toml` is read as a
//! fallback when no new config exists yet.

use std::path::PathBuf;

/// Project home directory (`SPEAK_HOME` or `~/.speak`).
#[must_use]
pub fn home() -> PathBuf {
    if let Some(dir) = env_path("SPEAK_HOME") {
        return dir;
    }
    base_home().join(".speak")
}

/// Active config file (`SPEAK_CONFIG` or `~/.speak/config.toml`).
#[must_use]
pub fn config_file() -> PathBuf {
    env_path("SPEAK_CONFIG").unwrap_or_else(|| home().join("config.toml"))
}

/// Legacy config file (`$XDG_CONFIG_HOME/speak/config.toml` or
/// `~/.config/speak/config.toml`), read only as a migration fallback.
#[must_use]
pub fn legacy_config_file() -> PathBuf {
    let base = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| base_home().join(".config"));
    base.join("speak").join("config.toml")
}

/// Log directory (`SPEAK_LOG_DIR` or `~/.speak/logs`).
#[must_use]
pub fn log_dir() -> PathBuf {
    env_path("SPEAK_LOG_DIR").unwrap_or_else(|| home().join("logs"))
}

/// Default daemon socket (`SPEAK_DAEMON_SOCKET` or `~/.speak/speak.sock`).
#[must_use]
pub fn default_socket() -> PathBuf {
    env_path("SPEAK_DAEMON_SOCKET").unwrap_or_else(|| home().join("speak.sock"))
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn base_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
