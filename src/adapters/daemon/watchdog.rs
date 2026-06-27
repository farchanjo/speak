//! Upstream health watchdog + self-recovery (ADR-0010).
//!
//! A running daemon spawns a background task that probes the upstream `/health`
//! endpoint every `[daemon].health_interval` seconds with a `SPEAK_HEALTH_TIMEOUT`
//! bound. The probe rides the `ServerProbe` port through the warm facade, so it
//! is the exact capability surface the CLI uses and is fully mockable.
//!
//! The transition logic is a small, pure state machine ([`Health`]) — `Healthy`
//! -> `Degraded` -> `Recovering` -> `Healthy` — unit-tested with scripted probe
//! outcomes and injected timestamps (no real sleeps). After
//! `[daemon].health_fails` consecutive failures it asks the daemon to
//! self-recover: rebuild the warm `openai`/`reqwest` client pool and re-run the
//! realtime capability probe, so a server that bounced is rediscovered without a
//! restart. A restored probe transitions back to `Healthy`. The loop backs off
//! through the shared `[retry]` policy while degraded.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::State;

/// The watchdog's coarse health state, surfaced in `daemon status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HealthState {
    /// The last probe succeeded.
    Healthy,
    /// At least one probe failed, but below the recovery threshold.
    Degraded,
    /// The failure threshold was crossed; the daemon is self-recovering.
    Recovering,
}

impl HealthState {
    /// The lowercase wire token for status reports.
    #[must_use]
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Recovering => "recovering",
        }
    }
}

/// The pure health state machine driven by probe outcomes (ADR-0010).
#[derive(Debug)]
pub(super) struct Health {
    state: HealthState,
    threshold: u32,
    consecutive_failures: u32,
    last_ok: Option<Instant>,
    last_err: Option<String>,
    recoveries: u64,
}

impl Health {
    /// A fresh, healthy watchdog that recovers after `threshold` (>=1) failures.
    #[must_use]
    pub(super) fn new(threshold: u32) -> Self {
        Self {
            state: HealthState::Healthy,
            threshold: threshold.max(1),
            consecutive_failures: 0,
            last_ok: None,
            last_err: None,
            recoveries: 0,
        }
    }

    /// Record a successful probe at `now`; returns `true` when this success is a
    /// transition back to `Healthy` from a degraded/recovering state (worth a log).
    pub(super) fn record_success(&mut self, now: Instant) -> bool {
        let recovered = self.state != HealthState::Healthy;
        self.state = HealthState::Healthy;
        self.consecutive_failures = 0;
        self.last_ok = Some(now);
        recovered
    }

    /// Record a failed probe (`err`) at `_now`; returns `true` exactly when the
    /// failure count first reaches the threshold (the daemon should self-recover).
    /// The timestamp is accepted for clock-injection symmetry with
    /// [`record_success`](Self::record_success) (only the last success time is kept).
    pub(super) fn record_failure(&mut self, err: String, _now: Instant) -> bool {
        self.consecutive_failures += 1;
        self.last_err = Some(err);
        if self.consecutive_failures >= self.threshold {
            let trigger = self.state != HealthState::Recovering;
            self.state = HealthState::Recovering;
            trigger
        } else {
            self.state = HealthState::Degraded;
            false
        }
    }

    /// Note that a self-recovery rebuild completed (counter for status).
    pub(super) fn note_recovery(&mut self) {
        self.recoveries += 1;
    }

    /// The current coarse state.
    #[must_use]
    pub(super) fn state(&self) -> HealthState {
        self.state
    }

    /// Consecutive failed probes since the last success.
    #[must_use]
    pub(super) fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Seconds since the last successful probe, or `None` if never healthy.
    #[must_use]
    pub(super) fn last_ok_elapsed_secs(&self) -> Option<u64> {
        self.last_ok.map(|t| t.elapsed().as_secs())
    }

    /// The most recent probe error message, if any.
    #[must_use]
    pub(super) fn last_error(&self) -> Option<&str> {
        self.last_err.as_deref()
    }

    /// How many self-recovery rebuilds the daemon has performed.
    #[must_use]
    pub(super) fn recoveries(&self) -> u64 {
        self.recoveries
    }
}

/// Spawn the background watchdog task for `state` (no-op when disabled).
pub(super) fn spawn(state: &Arc<State>) {
    if state.cfg.daemon.health_interval == 0 {
        tracing::debug!("health watchdog disabled (health_interval = 0)");
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move { run_loop(&state).await });
}

/// Probe on a cadence until the daemon shuts down; back off while degraded.
///
/// Probes first (so a fresh daemon reports real upstream health within one probe
/// rather than after a full interval), then waits the steady interval or, while
/// degraded, the bounded `[retry]` backoff for the current failure count.
async fn run_loop(state: &Arc<State>) {
    let interval = Duration::from_secs(state.cfg.daemon.health_interval);
    loop {
        probe_once(state).await;
        tokio::time::sleep(next_delay(state, interval)).await;
    }
}

/// The wait before the next probe: the steady interval when healthy, else the
/// bounded `[retry]` backoff for the current failure count (ADR-0010).
fn next_delay(state: &Arc<State>, interval: Duration) -> Duration {
    let failures = state.health_failures();
    if failures == 0 {
        return interval;
    }
    let policy = state.cfg.retry.policy;
    let entropy = crate::adapters::retry::jitter_entropy(state.cfg.retry.jitter_seed, failures);
    policy.delay_for(failures, entropy).min(interval)
}

/// Run one timed probe, fold the outcome into the state machine, and recover when
/// the failure threshold is crossed.
async fn probe_once(state: &Arc<State>) {
    let facade = state.facade();
    let timeout = Duration::from_secs(state.cfg.daemon.health_timeout);
    let outcome = match tokio::time::timeout(timeout, facade.probe_health()).await {
        Ok(Ok(true)) => Ok(()),
        Ok(Ok(false)) => Err("upstream /health returned a non-success status".to_owned()),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(_) => Err("upstream health probe timed out".to_owned()),
    };
    let trigger = fold_outcome(state, outcome);
    if trigger {
        recover(state).await;
    }
}

/// Apply a probe outcome to the shared [`Health`]; returns whether to recover.
fn fold_outcome(state: &Arc<State>, outcome: Result<(), String>) -> bool {
    let now = Instant::now();
    let mut health = state.health_lock();
    match outcome {
        Ok(()) => {
            if health.record_success(now) {
                tracing::info!("upstream health restored");
            }
            false
        }
        Err(err) => {
            tracing::warn!(
                failures = health.consecutive_failures() + 1,
                "upstream health probe failed: {err}"
            );
            health.record_failure(err, now)
        }
    }
}

/// Self-recovery: rebuild the warm client pool and re-run the realtime capability
/// probe so a recovered server is rediscovered, then swap the warm facade in.
async fn recover(state: &Arc<State>) {
    tracing::warn!(
        threshold = state.cfg.daemon.health_fails,
        "upstream degraded past threshold: rebuilding warm client pool"
    );
    match super::build_facade(&state.cfg) {
        Ok(facade) => {
            let facade = Arc::new(facade);
            reprobe_capability(state, facade.as_ref()).await;
            state.set_facade(facade);
            state.health_lock().note_recovery();
            tracing::info!("warm client pool rebuilt; resuming probes on the fresh pool");
        }
        Err(e) => tracing::error!("recovery failed to rebuild the warm facade: {e:#}"),
    }
}

/// Best-effort realtime-capability re-probe on the freshly built facade so the
/// SSE endpoint is rediscovered when the server returns (failures are tolerated).
async fn reprobe_capability(state: &Arc<State>, facade: &super::DaemonFacade) {
    let timeout = Duration::from_secs(state.cfg.daemon.health_timeout);
    match tokio::time::timeout(timeout, facade.supports_realtime()).await {
        Ok(Ok(realtime)) => {
            tracing::info!(realtime, "realtime capability re-probed after recovery");
        }
        Ok(Err(e)) => tracing::warn!("capability re-probe failed: {e:#}"),
        Err(_) => tracing::warn!("capability re-probe timed out"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn starts_healthy_with_no_failures() {
        let h = Health::new(3);
        assert_eq!(h.state(), HealthState::Healthy);
        assert_eq!(h.consecutive_failures(), 0);
        assert!(h.last_ok_elapsed_secs().is_none());
        assert!(h.last_error().is_none());
    }

    #[test]
    fn one_failure_degrades_without_triggering_recovery() {
        let mut h = Health::new(3);
        let trigger = h.record_failure("connection refused".to_owned(), now());
        assert!(!trigger);
        assert_eq!(h.state(), HealthState::Degraded);
        assert_eq!(h.consecutive_failures(), 1);
        assert_eq!(h.last_error(), Some("connection refused"));
    }

    #[test]
    fn reaching_the_threshold_triggers_recovery_once() {
        let mut h = Health::new(3);
        assert!(!h.record_failure("e1".to_owned(), now()));
        assert!(!h.record_failure("e2".to_owned(), now()));
        // Third consecutive failure crosses the threshold -> recover.
        assert!(h.record_failure("e3".to_owned(), now()));
        assert_eq!(h.state(), HealthState::Recovering);
        // A further failure stays Recovering and does NOT re-trigger.
        assert!(!h.record_failure("e4".to_owned(), now()));
        assert_eq!(h.consecutive_failures(), 4);
    }

    #[test]
    fn threshold_of_one_recovers_on_first_failure() {
        let mut h = Health::new(1);
        assert!(h.record_failure("boom".to_owned(), now()));
        assert_eq!(h.state(), HealthState::Recovering);
    }

    #[test]
    fn zero_threshold_is_clamped_to_one() {
        let mut h = Health::new(0);
        assert!(h.record_failure("boom".to_owned(), now()));
        assert_eq!(h.state(), HealthState::Recovering);
    }

    #[test]
    fn success_after_failures_returns_to_healthy_and_reports_recovery() {
        let mut h = Health::new(3);
        h.record_failure("e1".to_owned(), now());
        h.record_failure("e2".to_owned(), now());
        h.record_failure("e3".to_owned(), now());
        // First success after a degraded episode is a recovery transition.
        assert!(h.record_success(now()));
        assert_eq!(h.state(), HealthState::Healthy);
        assert_eq!(h.consecutive_failures(), 0);
        assert!(h.last_ok_elapsed_secs().is_some());
        // A second success is steady-state, not a transition.
        assert!(!h.record_success(now()));
    }

    #[test]
    fn note_recovery_increments_the_counter() {
        let mut h = Health::new(2);
        assert_eq!(h.recoveries(), 0);
        h.note_recovery();
        h.note_recovery();
        assert_eq!(h.recoveries(), 2);
    }

    #[test]
    fn state_tokens_render() {
        assert_eq!(HealthState::Healthy.as_str(), "healthy");
        assert_eq!(HealthState::Degraded.as_str(), "degraded");
        assert_eq!(HealthState::Recovering.as_str(), "recovering");
    }
}
