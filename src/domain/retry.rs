//! Retry/backoff value object (T015, FR-17).
//!
//! A pure, IO-free resilience primitive: it computes the bounded exponential
//! backoff schedule (with optional equal jitter) and classifies which transient
//! error kinds are retryable. The randomness for jitter is injected as a seed in
//! `[0.0, 1.0)` so the computation stays deterministic and unit-testable; the
//! caller supplies the entropy. This type only answers "retry?" and "how long do
//! I wait?"; the retry *loop* lives in the HTTP client today and will move to a
//! generic transport-agnostic port decorator (T046).

use std::time::Duration;

/// Transient error kinds a network call may fail with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// TCP/TLS connect failure.
    Connect,
    /// Request/response timeout.
    Timeout,
    /// HTTP 5xx server error.
    Server5xx,
    /// HTTP 429 Too Many Requests.
    TooMany429,
    /// Anything else (never retried).
    Other,
}

/// Which transient error classes are eligible for retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryOn {
    /// Retry on connect failures.
    pub connect: bool,
    /// Retry on timeouts.
    pub timeout: bool,
    /// Retry on HTTP 5xx.
    pub server_5xx: bool,
    /// Retry on HTTP 429.
    pub too_many: bool,
}

impl Default for RetryOn {
    fn default() -> Self {
        Self {
            connect: true,
            timeout: true,
            server_5xx: true,
            too_many: true,
        }
    }
}

impl RetryOn {
    /// Parse the `connect+timeout+5xx+429` token list (`+` or `,` separated).
    #[must_use]
    pub fn parse(spec: &str) -> Self {
        let mut out = Self {
            connect: false,
            timeout: false,
            server_5xx: false,
            too_many: false,
        };
        for token in spec.split(['+', ',', ' ']).filter(|t| !t.is_empty()) {
            match token.trim().to_ascii_lowercase().as_str() {
                "connect" => out.connect = true,
                "timeout" => out.timeout = true,
                "5xx" | "server" => out.server_5xx = true,
                "429" | "rate" | "ratelimit" => out.too_many = true,
                _ => {}
            }
        }
        out
    }

    /// Whether the given error kind is eligible for retry.
    #[must_use]
    pub fn allows(self, kind: ErrorKind) -> bool {
        match kind {
            ErrorKind::Connect => self.connect,
            ErrorKind::Timeout => self.timeout,
            ErrorKind::Server5xx => self.server_5xx,
            ErrorKind::TooMany429 => self.too_many,
            ErrorKind::Other => false,
        }
    }
}

/// A configurable bounded exponential-backoff-with-jitter retry policy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Maximum number of retries (total tries = `max_retries + 1`).
    pub max_retries: u32,
    /// First-retry delay floor, in milliseconds.
    pub backoff_initial_ms: u64,
    /// Delay ceiling, in milliseconds.
    pub backoff_max_ms: u64,
    /// Geometric growth factor between successive retries.
    pub multiplier: f64,
    /// Apply equal jitter to each delay.
    pub jitter: bool,
    /// Retryable error classification.
    pub retry_on: RetryOn,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            backoff_initial_ms: 200,
            backoff_max_ms: 5_000,
            multiplier: 2.0,
            jitter: true,
            retry_on: RetryOn::default(),
        }
    }
}

impl RetryPolicy {
    /// Whether to retry after the zero-based `attempt` that just failed with `kind`.
    #[must_use]
    pub fn should_retry(&self, attempt: u32, kind: ErrorKind) -> bool {
        attempt < self.max_retries && self.retry_on.allows(kind)
    }

    /// Delay before the retry following the zero-based `attempt`.
    ///
    /// `seed` is jitter entropy in `[0.0, 1.0)`; ignored when `jitter` is off.
    /// With equal jitter the result lies in `[capped/2, capped]`.
    #[must_use]
    pub fn delay_for(&self, attempt: u32, seed: f64) -> Duration {
        let capped = self.capped_ms(attempt);
        if !self.jitter {
            return Duration::from_millis(capped);
        }
        let half = capped / 2;
        let span = (capped - half) as f64;
        let jittered = half as f64 + seed.clamp(0.0, 1.0) * span;
        Duration::from_millis(jittered as u64)
    }

    /// The un-jittered, ceiling-capped delay in milliseconds for `attempt`.
    fn capped_ms(&self, attempt: u32) -> u64 {
        let grown = self.backoff_initial_ms as f64 * self.multiplier.powi(attempt as i32);
        let bounded = grown.min(self.backoff_max_ms as f64).max(0.0);
        bounded as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed() -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            backoff_initial_ms: 100,
            backoff_max_ms: 10_000,
            multiplier: 2.0,
            jitter: false,
            retry_on: RetryOn::default(),
        }
    }

    #[test]
    fn geometric_delay_growth() {
        let p = fixed();
        assert_eq!(p.delay_for(0, 0.0), Duration::from_millis(100));
        assert_eq!(p.delay_for(1, 0.0), Duration::from_millis(200));
        assert_eq!(p.delay_for(2, 0.0), Duration::from_millis(400));
        assert_eq!(p.delay_for(3, 0.0), Duration::from_millis(800));
    }

    #[test]
    fn delay_is_capped() {
        let p = fixed();
        assert_eq!(p.delay_for(20, 0.0), Duration::from_millis(10_000));
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let p = RetryPolicy {
            jitter: true,
            ..fixed()
        };
        // attempt 0 -> capped 100ms -> equal jitter in [50, 100].
        assert_eq!(p.delay_for(0, 0.0), Duration::from_millis(50));
        assert_eq!(p.delay_for(0, 1.0), Duration::from_millis(100));
        let mid = p.delay_for(0, 0.5).as_millis();
        assert!((50..=100).contains(&mid), "jitter {mid} out of [50,100]");
    }

    #[test]
    fn attempt_count_respects_max() {
        let p = fixed();
        assert!(p.should_retry(0, ErrorKind::Connect));
        assert!(p.should_retry(2, ErrorKind::Connect));
        assert!(!p.should_retry(3, ErrorKind::Connect));
    }

    #[test]
    fn retry_on_classification() {
        let on = RetryOn::parse("connect+timeout+5xx+429");
        assert!(on.allows(ErrorKind::Connect));
        assert!(on.allows(ErrorKind::Timeout));
        assert!(on.allows(ErrorKind::Server5xx));
        assert!(on.allows(ErrorKind::TooMany429));
        assert!(!on.allows(ErrorKind::Other));

        let only_connect = RetryOn::parse("connect");
        assert!(only_connect.allows(ErrorKind::Connect));
        assert!(!only_connect.allows(ErrorKind::Timeout));
    }

    #[test]
    fn non_retryable_kind_never_retries() {
        let p = fixed();
        assert!(!p.should_retry(0, ErrorKind::Other));
    }
}
