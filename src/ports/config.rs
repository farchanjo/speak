//! `ConfigProvider` driven port (T022).
//!
//! Resolves the layered configuration (flag > env > `~/.speak/config.toml` >
//! default) into the [`Config`] catalog with per-key origins (ADR-0006). The
//! config adapter implements it; use cases depend on this port rather than the
//! concrete resolver.
//!
//! NOTE: the resolved [`Config`] is plain data (no `reqwest`/`ffmpeg`/`objc2`
//! types). The serde/toml resolver lives in the [`crate::adapters::config`]
//! driven adapter, so this port names only the POD it returns. The port lets the
//! application layer target the abstraction rather than the concrete resolver.

use anyhow::Result;

use crate::adapters::config::Config;

/// Driven port: load the fully-resolved configuration catalog.
pub trait ConfigProvider {
    /// Resolve flags + env + file + defaults into a [`Config`] with origins.
    fn load(&self) -> Result<Config>;
}
