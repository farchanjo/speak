//! Bounded reconnect for the realtime SSE [`RealtimeStream`] port (T046).
//!
//! A dropped realtime stream is not a request that can simply be replayed: it
//! must be re-established. [`ReconnectingStream`] wraps a [`StreamFactory`] and,
//! on a transient `recv` failure, rebuilds the stream under the SAME bounded
//! [`RetryPolicy`](crate::ports::retry::RetryPolicy) Strategy the request
//! decorators use (connect/timeout/5xx/429 classification, exponential backoff +
//! jitter). A normal end-of-stream (`Ok(None)`) is terminal — it never
//! reconnects. The reconnect budget resets after each frame that makes progress.
//!
//! The eventsource-stream `RealtimeStream` adapter (T036) is not yet landed, so
//! this wrapper currently has no production wiring; it ships as a tested,
//! ready-to-inject capability so the SSE path rides the same policy the moment
//! that adapter exists.

use anyhow::Result;

use super::{classify, jitter_entropy};
use crate::ports::realtime::{RealtimeFrame, RealtimeStream};
use crate::ports::retry::RetryPolicy;

/// Builds a fresh [`RealtimeStream`] for a (re)connect attempt.
#[expect(
    async_fn_in_trait,
    reason = "consumed by the generic ReconnectingStream, not as a trait object (ADR-0004)"
)]
pub trait StreamFactory {
    /// The realtime stream this factory produces.
    type Stream: RealtimeStream;

    /// Establish a new realtime stream (a fresh SSE connection).
    async fn connect(&self) -> Result<Self::Stream>;
}

/// A [`RealtimeStream`] that transparently reconnects its inner stream under the
/// retry policy when a transient failure interrupts it.
pub struct ReconnectingStream<F: StreamFactory, P> {
    factory: F,
    policy: P,
    jitter_seed: Option<u64>,
    current: Option<F::Stream>,
    attempt: u32,
}

impl<F, P> ReconnectingStream<F, P>
where
    F: StreamFactory,
    P: RetryPolicy,
{
    /// Wrap `factory`, reconnecting under `policy` with optional `jitter_seed`.
    #[must_use]
    pub fn new(factory: F, policy: P, jitter_seed: Option<u64>) -> Self {
        Self {
            factory,
            policy,
            jitter_seed,
            current: None,
            attempt: 0,
        }
    }

    /// Establish the stream, retrying transient connect failures under the policy.
    async fn ensure_connected(&mut self) -> Result<()> {
        while self.current.is_none() {
            match self.factory.connect().await {
                Ok(stream) => {
                    self.current = Some(stream);
                    self.attempt = 0;
                }
                Err(err) => self.backoff_or_fail(err).await?,
            }
        }
        Ok(())
    }

    /// Sleep under the policy for a retryable error, or surface it when the
    /// bounded budget is exhausted or the error is not transient.
    async fn backoff_or_fail(&mut self, err: anyhow::Error) -> Result<()> {
        let kind = classify(&err);
        if !self.policy.should_retry(self.attempt, kind) {
            return Err(err);
        }
        let delay = self
            .policy
            .delay_for(self.attempt, jitter_entropy(self.jitter_seed, self.attempt));
        tracing::debug!(
            attempt = self.attempt,
            ?kind,
            "reconnecting dropped realtime stream"
        );
        tokio::time::sleep(delay).await;
        self.attempt += 1;
        Ok(())
    }
}

impl<F, P> RealtimeStream for ReconnectingStream<F, P>
where
    F: StreamFactory,
    P: RetryPolicy,
{
    async fn recv(&mut self) -> Result<Option<RealtimeFrame>> {
        loop {
            self.ensure_connected().await?;
            let stream = self
                .current
                .as_mut()
                .expect("ensure_connected guarantees a live stream");
            match stream.recv().await {
                Ok(Some(frame)) => {
                    self.attempt = 0;
                    return Ok(Some(frame));
                }
                Ok(None) => return Ok(None),
                Err(err) => {
                    self.current = None;
                    self.backoff_or_fail(err).await?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use crate::adapters::retry::HttpStatusError;
    use crate::domain::retry::RetryPolicy as Policy;

    /// A scripted stream that yields a fixed sequence of `recv` results.
    struct ScriptedStream {
        items: VecDeque<Result<Option<RealtimeFrame>>>,
    }

    impl RealtimeStream for ScriptedStream {
        async fn recv(&mut self) -> Result<Option<RealtimeFrame>> {
            // Return the scripted result verbatim so the typed `HttpStatusError`
            // survives for the reconnect classifier (no `.to_string()` erasure).
            self.items.pop_front().unwrap_or(Ok(None))
        }
    }

    /// A factory that hands out pre-scripted streams in order, counting connects.
    struct ScriptedFactory {
        scripts: RefCell<VecDeque<Vec<Result<Option<RealtimeFrame>>>>>,
        connects: RefCell<u32>,
    }

    impl StreamFactory for ScriptedFactory {
        type Stream = ScriptedStream;

        async fn connect(&self) -> Result<Self::Stream> {
            *self.connects.borrow_mut() += 1;
            let items = self.scripts.borrow_mut().pop_front().unwrap_or_default();
            Ok(ScriptedStream {
                items: items.into(),
            })
        }
    }

    fn transcript(text: &str) -> Result<Option<RealtimeFrame>> {
        Ok(Some(RealtimeFrame::Transcript {
            text: text.to_owned(),
        }))
    }

    fn fast_policy() -> Policy {
        Policy {
            max_retries: 3,
            backoff_initial_ms: 0,
            backoff_max_ms: 0,
            jitter: false,
            ..Policy::default()
        }
    }

    #[tokio::test]
    async fn reconnects_after_a_transient_drop() {
        // First connection yields one frame then a 5xx drop; the reconnect yields
        // a second frame then completes.
        let scripts = VecDeque::from(vec![
            vec![
                transcript("one"),
                Err(anyhow::anyhow!(HttpStatusError::new(503, "drop".into()))),
            ],
            vec![transcript("two"), Ok(None)],
        ]);
        let factory = ScriptedFactory {
            scripts: RefCell::new(scripts),
            connects: RefCell::new(0),
        };
        let mut stream = ReconnectingStream::new(factory, fast_policy(), Some(1));

        assert_eq!(
            stream.recv().await.unwrap(),
            Some(RealtimeFrame::Transcript { text: "one".into() })
        );
        assert_eq!(
            stream.recv().await.unwrap(),
            Some(RealtimeFrame::Transcript { text: "two".into() })
        );
        assert_eq!(stream.recv().await.unwrap(), None);
        assert_eq!(
            *stream.factory.connects.borrow(),
            2,
            "exactly one reconnect"
        );
    }

    #[tokio::test]
    async fn normal_end_of_stream_does_not_reconnect() {
        let scripts = VecDeque::from(vec![vec![transcript("only"), Ok(None)]]);
        let factory = ScriptedFactory {
            scripts: RefCell::new(scripts),
            connects: RefCell::new(0),
        };
        let mut stream = ReconnectingStream::new(factory, fast_policy(), None);
        assert!(stream.recv().await.unwrap().is_some());
        assert!(stream.recv().await.unwrap().is_none());
        assert_eq!(
            *stream.factory.connects.borrow(),
            1,
            "no reconnect on Ok(None)"
        );
    }
}
