//! `speak` library core — the reusable, testable hexagonal core behind the
//! `speak` binary.
//!
//! The CLI (driving adapter) lives in `src/main.rs` and depends inward on these
//! modules; nothing here depends on the binary. Splitting the core into a
//! library makes the full configuration catalog, the daemon + HTTP client, the
//! libav codec layer, and the domain value objects directly reachable from the
//! integration test suite under `tests/`.
//!
//! Media path: server audio is decoded and resampled with linked `libav*` in the
//! `libav` adapter and played through the native macOS CoreAudio mixer in the
//! `coreaudio` adapter; the microphone is captured natively too. Nothing is
//! shelled out. Framework crates live only under `src/adapters` (and clap under
//! `src/cli`); the flat-root modules are this index, the composition root
//! (`main.rs`), and the `logging`/`paths` cross-cutting helpers.

pub mod adapters;
pub mod application;
pub mod domain;
pub mod logging;
pub mod paths;
pub mod ports;

/// Process-wide lock shared by every test that mutates `SPEAK_*` / `HOME` /
/// `XDG_CONFIG_HOME` process environment. libc `setenv`/`getenv` are not
/// thread-safe against each other, so all env-touching tests across modules
/// must serialise on this single mutex (not a per-module one).
#[cfg(test)]
pub(crate) mod testenv {
    use std::sync::Mutex;

    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());
}
