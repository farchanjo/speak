//! Request transport: a warm daemon when one is live, else a direct client.
//!
//! Both variants expose the same `proxy` / `proxy_multipart` surface so command
//! handlers are agnostic to whether the HTTP runs in-process or is forwarded to
//! the persistent daemon over its Unix socket.
//!
//! Daemon **autostart** is OFF by default and gated behind the
//! `[daemon].autostart` config flag (`SPEAK_DAEMON_AUTOSTART`): only when it is
//! enabled does [`Transport::connect`] spawn a background daemon. The spawn runs
//! THIS binary's own `daemon` subcommand ([`autostart`]) — never an external
//! media tool — so the zero-process-exec-for-media invariant (ADR-0001) holds.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::client::{Field, ProxyReply, SpeechClient};
use crate::config::Config;
use crate::daemon;

/// A request transport.
pub enum Transport {
    /// In-process one-shot client.
    Direct(SpeechClient),
    /// Forward to a running daemon at this socket.
    Daemon(PathBuf),
}

impl Transport {
    /// Pick the daemon when its socket is live (optionally autostarting one),
    /// otherwise a direct client.
    pub async fn connect(cfg: &Config) -> Result<Self> {
        let socket = cfg.daemon.socket.clone();
        if daemon::is_running(&socket).await {
            tracing::debug!(socket = %socket.display(), "transport: daemon");
            return Ok(Self::Daemon(socket));
        }
        if cfg.daemon.autostart
            && let Some(transport) = autostart(&socket).await
        {
            return Ok(transport);
        }
        tracing::debug!("transport: direct");
        Ok(Self::Direct(SpeechClient::new(cfg)?))
    }

    /// Transport kind for diagnostics.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Direct(_) => "direct",
            Self::Daemon(_) => "daemon",
        }
    }

    /// Proxy a JSON/bodyless request.
    pub async fn proxy(
        &self,
        method: &str,
        endpoint: &str,
        json: Option<Value>,
    ) -> Result<ProxyReply> {
        match self {
            Self::Direct(client) => client.proxy(method, endpoint, json).await,
            Self::Daemon(socket) => daemon::forward_json(socket, method, endpoint, json).await,
        }
    }

    /// Proxy a multipart upload with a named file part.
    pub async fn proxy_multipart(
        &self,
        endpoint: &str,
        fields: &[Field],
        file: Option<(Vec<u8>, String)>,
        file_part: &str,
    ) -> Result<ProxyReply> {
        match self {
            Self::Direct(client) => {
                client
                    .proxy_multipart(endpoint, fields, file, file_part)
                    .await
            }
            Self::Daemon(socket) => {
                daemon::forward_multipart(socket, endpoint, fields, file, file_part).await
            }
        }
    }
}

/// Spawn a background daemon by re-executing THIS binary's `daemon` subcommand,
/// then wait briefly for its socket to come up. Reached only when
/// `[daemon].autostart` is enabled (see [`Transport::connect`]); the spawned
/// process is our own binary, not any external/media tool (ADR-0001).
async fn autostart(socket: &Path) -> Option<Transport> {
    let exe = std::env::current_exe().ok()?;
    std::process::Command::new(exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if daemon::is_running(socket).await {
            tracing::debug!("transport: autostarted daemon");
            return Some(Transport::Daemon(socket.to_path_buf()));
        }
    }
    None
}
