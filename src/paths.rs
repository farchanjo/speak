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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::ENV_LOCK;

    /// Run `body` under one lock acquisition with the given vars `set` and the
    /// given names removed, restoring every touched var afterwards. A single,
    /// non-nesting helper avoids re-entrant locking on the non-reentrant Mutex.
    fn scoped<T>(set: &[(&str, &str)], unset: &[&str], body: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut saved: Vec<(String, Option<String>)> = Vec::new();
        for (k, v) in set {
            saved.push(((*k).to_owned(), std::env::var(k).ok()));
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var(k, v) };
        }
        for k in unset {
            saved.push(((*k).to_owned(), std::env::var(k).ok()));
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var(k) };
        }
        let out = body();
        for (k, prev) in saved.into_iter().rev() {
            match prev {
                // TODO: Audit that the environment access only happens in single-threaded code.
                Some(v) => unsafe { std::env::set_var(&k, v) },
                // TODO: Audit that the environment access only happens in single-threaded code.
                None => unsafe { std::env::remove_var(&k) },
            }
        }
        out
    }

    #[test]
    fn home_honours_speak_home_override() {
        scoped(&[("SPEAK_HOME", "/tmp/speak-home")], &[], || {
            assert_eq!(home(), PathBuf::from("/tmp/speak-home"));
        });
    }

    #[test]
    fn home_defaults_under_dot_speak() {
        scoped(&[], &["SPEAK_HOME"], || {
            assert!(home().ends_with(".speak"));
        });
    }

    #[test]
    fn config_file_honours_explicit_override() {
        scoped(&[("SPEAK_CONFIG", "/etc/speak.toml")], &[], || {
            assert_eq!(config_file(), PathBuf::from("/etc/speak.toml"));
        });
    }

    #[test]
    fn config_file_defaults_under_home() {
        scoped(&[("SPEAK_HOME", "/tmp/h")], &["SPEAK_CONFIG"], || {
            assert_eq!(config_file(), PathBuf::from("/tmp/h/config.toml"));
        });
    }

    #[test]
    fn default_socket_honours_override() {
        scoped(&[("SPEAK_DAEMON_SOCKET", "/run/speak.sock")], &[], || {
            assert_eq!(default_socket(), PathBuf::from("/run/speak.sock"));
        });
    }

    #[test]
    fn default_socket_defaults_under_home() {
        scoped(
            &[("SPEAK_HOME", "/tmp/h")],
            &["SPEAK_DAEMON_SOCKET"],
            || {
                assert_eq!(default_socket(), PathBuf::from("/tmp/h/speak.sock"));
            },
        );
    }

    #[test]
    fn empty_override_is_ignored() {
        scoped(&[("SPEAK_HOME", "")], &[], || {
            // An empty value must not shadow the default `~/.speak` resolution.
            assert!(home().ends_with(".speak"));
        });
    }

    #[test]
    fn legacy_config_uses_xdg_config_home() {
        scoped(&[("XDG_CONFIG_HOME", "/tmp/xdg")], &[], || {
            assert_eq!(
                legacy_config_file(),
                PathBuf::from("/tmp/xdg/speak/config.toml")
            );
        });
    }
}
