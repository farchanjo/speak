//! `check` / `health` handlers.
//!
//! `check` reports the host plus the local `accel` acceleration probe and the
//! resolved config/log/socket paths (offline, cross-cutting data per ADR-0003).
//! `health` prints the server's `/health` JSON over the request transport.

use anyhow::Result;

use speak::config::Config;
use speak::transport::Transport;
use speak::{accel, logging, paths};

/// Run the `health` subcommand: print the server `/health` JSON.
pub async fn health(cfg: &Config) -> Result<()> {
    let transport = Transport::connect(cfg).await?;
    let value = transport.proxy("GET", "/health", None).await?.into_json()?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// Run the `check` subcommand: report host + local acceleration + paths.
pub fn check(cfg: &Config) -> Result<()> {
    let report = accel::probe();
    println!("host:                  {}", cfg.server.host);
    println!("os / arch:             {} / {}", report.os, report.arch);
    println!("cpu cores (threading): {}", report.cpu_cores);
    println!("libavcodec:            {}", report.libavcodec);
    println!(
        "hwdevice types:        {}",
        list_or(&report.hwdevice_types, "none")
    );
    println!(
        "audiotoolbox decoders: {}",
        list_or(&report.audiotoolbox_decoders, "none")
    );
    println!(
        "hwaccel policy:        {} (override: {}=auto|off|<decoder>)",
        report.policy,
        accel::ENV_HWACCEL
    );
    println!("config:                {}", paths::config_file().display());
    println!("logs:                  {}", logging::log_dir().display());
    println!("daemon socket:         {}", cfg.daemon.socket.display());
    println!(
        "note: audio decode has no GPU/NVENC path (that hardware is server-side); \
         local acceleration = all-core frame threading + AudioToolbox decoders."
    );
    Ok(())
}

/// Render a list as a comma-joined string, or `empty` when there are none.
fn list_or(items: &[String], empty: &str) -> String {
    if items.is_empty() {
        empty.to_owned()
    } else {
        items.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_or_renders_items_or_placeholder() {
        assert_eq!(list_or(&["a".to_owned(), "b".to_owned()], "none"), "a, b");
        assert_eq!(list_or(&[], "none"), "none");
    }
}
