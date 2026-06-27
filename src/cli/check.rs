//! `check` / `health` handlers.
//!
//! `check` reports the host plus the local `accel` acceleration probe and the
//! resolved config/log/socket paths (offline, cross-cutting data per ADR-0003).
//! `health` prints the server's `/health` JSON over the request transport. Both
//! emit their RESULT through the [`Presenter`] port (ADR-0009) — never `println!`.

use anyhow::Result;

use speak::config::Config;
use speak::ports::presenter::{Presenter, Report};
use speak::transport::Transport;
use speak::{accel, logging, paths};

/// Run the `health` subcommand: print the server `/health` JSON.
pub async fn health(cfg: &Config, presenter: &mut dyn Presenter) -> Result<()> {
    let transport = Transport::connect(cfg).await?;
    let value = transport.proxy("GET", "/health", None).await?.into_json()?;
    presenter.line(&serde_json::to_string_pretty(&value)?)
}

/// Run the `check` subcommand: report host + local acceleration + paths.
pub fn check(cfg: &Config, presenter: &mut dyn Presenter) -> Result<()> {
    let report = accel::probe();
    let result = Report::titled("check")
        .entry("host", cfg.server.host.as_str())
        .entry("os / arch", format!("{} / {}", report.os, report.arch))
        .entry("cpu cores (threading)", report.cpu_cores.to_string())
        .entry("libavcodec", report.libavcodec.as_str())
        .entry("hwdevice types", list_or(&report.hwdevice_types, "none"))
        .entry(
            "audiotoolbox decoders",
            list_or(&report.audiotoolbox_decoders, "none"),
        )
        .entry(
            "hwaccel policy",
            format!(
                "{} (override: {}=auto|off|<decoder>)",
                report.policy,
                accel::ENV_HWACCEL
            ),
        )
        .entry("config", paths::config_file().display().to_string())
        .entry("logs", logging::log_dir().display().to_string())
        .entry("daemon socket", cfg.daemon.socket.display().to_string())
        .entry(
            "note",
            "audio decode has no GPU/NVENC path (that hardware is server-side); \
             local acceleration = all-core frame threading + AudioToolbox decoders.",
        );
    presenter.report(&result)
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
