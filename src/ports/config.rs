//! `ConfigProvider` driven port (T022).
//!
//! Resolves the layered configuration (flag > env > `~/.speak/config.toml` >
//! default) into the [`Config`] catalog with per-key origins (ADR-0006). The
//! config adapter implements it; use cases depend on this port rather than the
//! concrete resolver.
//!
//! NOTE: the resolved [`Config`] is plain data (no `reqwest`/`ffmpeg`/`objc2`
//! types). It still lives in the flat `crate::config` module today; when that
//! module moves under `adapters/config` in a later rebuild stage, this port and
//! the resolved POD move with it. The port is introduced now so the
//! application layer can target the abstraction immediately.

use anyhow::Result;

use crate::config::Config;

/// Driven port: load the fully-resolved configuration catalog.
pub trait ConfigProvider {
    /// Resolve flags + env + file + defaults into a [`Config`] with origins.
    fn load(&self) -> Result<Config>;
}
