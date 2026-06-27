//! `retry` driven adapters (T046, ADR-0004): transport-agnostic, port-preserving
//! retry decorators.
//!
//! For every driven NETWORK port (`Synthesizer`, `Transcriber`, `Translator`,
//! `VoiceRepository`, `ServerProbe`) the generic [`Retry`] decorator **implements
//! that same port** so it is a drop-in substitute for the concrete adapter, and
//! consults the injected [`RetryPolicy`](crate::ports::retry::RetryPolicy)
//! **Strategy** (driven by [`domain::retry::RetryPolicy`](crate::domain::retry::RetryPolicy))
//! for the bounded exponential-backoff-with-jitter schedule. The decorator is NOT
//! itself the policy port — it is a wrapper that calls the policy.
//!
//! Classification ([`classify`]) maps a failed call's `anyhow::Error` to the pure
//! [`ErrorKind`](crate::domain::retry::ErrorKind) the policy understands
//! (`connect`/`timeout`/`5xx`/`429`); the openai adapter tags non-2xx responses
//! with [`HttpStatusError`] so the status survives the `anyhow` boundary. The SSE
//! [`ReconnectingStream`] rides the same policy for bounded reconnect.
//!
//! Every tunable (retry count, backoff floor/ceiling, multiplier, jitter,
//! `retry_on` classes, jitter seed) comes from `[retry]` config — env + default,
//! no magic numbers (FR-18). Injected at the composition-root Factory.

mod classify;
mod decorator;
mod stream;

pub use classify::classify;
pub use decorator::Retry;
pub use stream::{ReconnectingStream, StreamFactory};

/// A non-2xx HTTP response surfaced as a typed error so the retry [`classify`]
/// pass can recover the status code after it crosses the `anyhow` boundary.
///
/// The openai adapter returns this from `send_ok` instead of an opaque string,
/// keeping the same `Display` text ("server returned <status>: <body>") while
/// letting the decorator classify `5xx`/`429` for retry.
#[derive(Debug, Clone)]
pub struct HttpStatusError {
    /// The HTTP status code of the failing response.
    pub status: u16,
    /// The (trimmed) response body, for diagnostics.
    pub body: String,
}

impl HttpStatusError {
    /// Build a status error from the response `status` and `body`.
    #[must_use]
    pub fn new(status: u16, body: String) -> Self {
        Self {
            status,
            body: body.trim().to_owned(),
        }
    }
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server returned {}: {}", self.status, self.body)
    }
}

impl std::error::Error for HttpStatusError {}

/// Jitter entropy in `[0.0, 1.0)`: deterministic when a `seed` is configured (so
/// runs are reproducible), else derived from the OS clock so the pure policy
/// stays testable without owning a clock.
pub(crate) fn jitter_entropy(seed: Option<u64>, attempt: u32) -> f64 {
    match seed {
        Some(seed) => deterministic_seed(seed, attempt),
        None => os_seed(),
    }
}

/// OS-clock-derived jitter entropy in `[0.0, 1.0)`.
fn os_seed() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos) / 1_000_000_000.0
}

/// Reproducible jitter entropy in `[0.0, 1.0)` from a fixed seed + attempt.
fn deterministic_seed(seed: u64, attempt: u32) -> f64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (seed, attempt).hash(&mut hasher);
    (hasher.finish() % 1_000_000) as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_error_displays_status_and_trimmed_body() {
        let err = HttpStatusError::new(503, "  upstream busy \n".to_owned());
        assert_eq!(err.status, 503);
        assert_eq!(err.to_string(), "server returned 503: upstream busy");
    }

    #[test]
    fn deterministic_seed_is_reproducible_and_in_range() {
        assert_eq!(deterministic_seed(7, 2), deterministic_seed(7, 2));
        for attempt in 0..8u32 {
            assert!((0.0..1.0).contains(&deterministic_seed(42, attempt)));
        }
    }

    #[test]
    fn jitter_entropy_uses_seed_when_present() {
        assert_eq!(
            jitter_entropy(Some(11), 3),
            jitter_entropy(Some(11), 3),
            "seeded entropy must be reproducible"
        );
        assert!((0.0..1.0).contains(&jitter_entropy(None, 0)));
    }
}
