//! `check`/`health` use case (T047): server + local-capability reporting (FR-14).
//!
//! Orchestrates the `ServerProbe` port (`GET /health`, `GET /v1/models`, and the
//! runtime realtime capability probe) and folds in the `accel` cross-cutting
//! probe — passed in as plain data per ADR-0003, never reached through a port —
//! to produce the data `speak health` and `speak check` print. The realtime
//! probe is best-effort: an unreachable capability endpoint reports "not
//! supported" rather than failing the whole report.

use anyhow::Result;

use crate::adapters::libav::accel::Report as AccelReport;
use crate::ports::probe::ServerProbe;

/// The server-side health snapshot (`speak health`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthOutcome {
    /// Whether `GET /health` reports the server healthy.
    pub healthy: bool,
    /// Model ids advertised by `GET /v1/models`.
    pub models: Vec<String>,
    /// Whether the realtime SSE endpoint is available.
    pub realtime: bool,
}

/// The full diagnostic snapshot (`speak check`): server health + local accel.
#[derive(Debug)]
pub struct CheckOutcome {
    /// The configured server base URL.
    pub host: String,
    /// The server-side health snapshot.
    pub server: HealthOutcome,
    /// The local acceleration / host report (cross-cutting data).
    pub accel: AccelReport,
}

/// The `check`/`health` use case over the [`ServerProbe`] port.
pub struct CheckUseCase<'a, P> {
    probe: &'a P,
}

impl<'a, P> CheckUseCase<'a, P>
where
    P: ServerProbe,
{
    /// Wire the use case to its port.
    #[must_use]
    pub fn new(probe: &'a P) -> Self {
        Self { probe }
    }

    /// Probe the server's health, advertised models, and realtime capability.
    pub async fn health(&self) -> Result<HealthOutcome> {
        let healthy = self.probe.health().await?;
        let models = self.probe.models().await?;
        // Capability is advisory: a missing/unreachable endpoint => not supported.
        let realtime = self.probe.supports_realtime().await.unwrap_or(false);
        Ok(HealthOutcome {
            healthy,
            models,
            realtime,
        })
    }

    /// Combine the server health snapshot with the local `accel` report.
    pub async fn check(&self, host: &str, accel: AccelReport) -> Result<CheckOutcome> {
        Ok(CheckOutcome {
            host: host.to_owned(),
            server: self.health().await?,
            accel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::FakeSpeech;

    fn accel() -> AccelReport {
        AccelReport {
            os: "macos".to_owned(),
            arch: "aarch64".to_owned(),
            cpu_cores: 10,
            libavcodec: "62.3.100".to_owned(),
            hwdevice_types: vec!["videotoolbox".to_owned()],
            audiotoolbox_decoders: vec!["mp3_at".to_owned()],
            policy: "Auto".to_owned(),
        }
    }

    #[tokio::test]
    async fn health_reports_server_state_and_models() {
        let speech = FakeSpeech::default();
        let out = CheckUseCase::new(&speech).health().await.unwrap();
        assert!(out.healthy);
        assert!(out.realtime);
        assert_eq!(out.models, vec!["tts-1", "whisper-1"]);
    }

    #[tokio::test]
    async fn check_folds_accel_and_host() {
        let speech = FakeSpeech {
            realtime: false,
            ..FakeSpeech::default()
        };
        let out = CheckUseCase::new(&speech)
            .check("http://solaris:8800", accel())
            .await
            .unwrap();
        assert_eq!(out.host, "http://solaris:8800");
        assert!(!out.server.realtime);
        assert_eq!(out.accel.cpu_cores, 10);
        assert_eq!(out.accel.audiotoolbox_decoders, vec!["mp3_at"]);
    }
}
