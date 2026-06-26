//! `RetryPolicy` driven port (T023).
//!
//! The resilience **Strategy** the `adapters/retry` decorators consult around
//! every network call (ADR-0004). It is interchangeable (no-op / fixed /
//! exponential); the default production strategy is the pure
//! [`RetryPolicyVo`](crate::domain::retry::RetryPolicy) value object, for which
//! a blanket port implementation is provided here so the composition root can
//! inject the domain object directly. The decorator calls [`should_retry`] then
//! sleeps [`delay_for`]; it is NOT itself this port.
//!
//! [`should_retry`]: RetryPolicy::should_retry
//! [`delay_for`]: RetryPolicy::delay_for

use std::time::Duration;

use crate::domain::retry::{ErrorKind, RetryPolicy as RetryPolicyVo};

/// Driven port (Strategy): decide whether and how long to wait before a retry.
pub trait RetryPolicy {
    /// Whether to retry after the zero-based `attempt` that failed with `kind`.
    fn should_retry(&self, attempt: u32, kind: ErrorKind) -> bool;

    /// The delay before the retry following `attempt`; `seed` is jitter entropy
    /// in `[0.0, 1.0)`.
    fn delay_for(&self, attempt: u32, seed: f64) -> Duration;
}

/// The pure value object IS the default exponential-backoff Strategy.
impl RetryPolicy for RetryPolicyVo {
    fn should_retry(&self, attempt: u32, kind: ErrorKind) -> bool {
        RetryPolicyVo::should_retry(self, attempt, kind)
    }

    fn delay_for(&self, attempt: u32, seed: f64) -> Duration {
        RetryPolicyVo::delay_for(self, attempt, seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercise the value object through the port trait (dynamic dispatch).
    fn via_port(policy: &dyn RetryPolicy, attempt: u32, kind: ErrorKind) -> bool {
        policy.should_retry(attempt, kind)
    }

    #[test]
    fn value_object_satisfies_the_port() {
        let policy = RetryPolicyVo::default();
        assert!(via_port(&policy, 0, ErrorKind::Connect));
        assert!(!via_port(&policy, 0, ErrorKind::Other));
    }

    #[test]
    fn port_delay_matches_value_object() {
        let policy = RetryPolicyVo {
            jitter: false,
            ..RetryPolicyVo::default()
        };
        assert_eq!(
            RetryPolicy::delay_for(&policy, 0, 0.0),
            policy.delay_for(0, 0.0)
        );
    }
}
