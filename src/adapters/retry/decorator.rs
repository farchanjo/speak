//! The generic, port-preserving [`Retry`] decorator.
//!
//! `Retry<Inner, P>` wraps any adapter and re-implements each driven network port
//! it covers, delegating to the wrapped adapter through a single bounded
//! retry loop ([`Retry::run`]) that consults the injected
//! [`RetryPolicy`](crate::ports::retry::RetryPolicy) Strategy. Because it
//! implements the SAME ports, it is substitutable for the concrete adapter at the
//! composition root — the use cases never know they hold a decorator.

use std::future::Future;

use anyhow::Result;

use super::{classify, jitter_entropy};
use crate::domain::language::Language;
use crate::domain::retry::RetryPolicy as RetryPolicyVo;
use crate::domain::speech_spec::SpeechSpec;
use crate::domain::voice::Voice;
use crate::ports::probe::ServerProbe;
use crate::ports::retry::RetryPolicy;
use crate::ports::synthesizer::{SynthesizedAudio, Synthesizer};
use crate::ports::transcriber::{TranscribeRequest, Transcriber};
use crate::ports::translator::Translator;
use crate::ports::voice::VoiceRepository;

/// A port-preserving retry decorator around `Inner`, driven by policy `P`.
///
/// `P` defaults to the pure [`domain::RetryPolicy`](RetryPolicyVo) value object,
/// which is itself the production exponential-backoff Strategy (the blanket port
/// impl lives in [`crate::ports::retry`]).
pub struct Retry<Inner, P = RetryPolicyVo> {
    inner: Inner,
    policy: P,
    jitter_seed: Option<u64>,
}

impl<Inner, P> Retry<Inner, P>
where
    P: RetryPolicy,
{
    /// Wrap `inner`, consulting `policy` with optional reproducible `jitter_seed`.
    #[must_use]
    pub fn new(inner: Inner, policy: P, jitter_seed: Option<u64>) -> Self {
        Self {
            inner,
            policy,
            jitter_seed,
        }
    }

    /// Borrow the wrapped adapter (for ports the decorator does not itself wrap).
    #[must_use]
    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    /// Run `op`, retrying its failures under the policy's bounded schedule.
    async fn run<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempt = 0u32;
        loop {
            match op().await {
                Ok(value) => return Ok(value),
                Err(err) => {
                    let kind = classify(&err);
                    if !self.policy.should_retry(attempt, kind) {
                        return Err(err);
                    }
                    let delay = self
                        .policy
                        .delay_for(attempt, jitter_entropy(self.jitter_seed, attempt));
                    tracing::debug!(
                        attempt,
                        ?kind,
                        delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        "retrying transient network failure"
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }
}

impl<Inner, P> Synthesizer for Retry<Inner, P>
where
    Inner: Synthesizer,
    P: RetryPolicy,
{
    async fn synthesize(&self, spec: &SpeechSpec) -> Result<SynthesizedAudio> {
        self.run(|| self.inner.synthesize(spec)).await
    }
}

impl<Inner, P> Transcriber for Retry<Inner, P>
where
    Inner: Transcriber,
    P: RetryPolicy,
{
    async fn transcribe(&self, req: &TranscribeRequest<'_>) -> Result<String> {
        self.run(|| self.inner.transcribe(req)).await
    }
}

impl<Inner, P> Translator for Retry<Inner, P>
where
    Inner: Translator,
    P: RetryPolicy,
{
    async fn translate(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String> {
        self.run(|| self.inner.translate(audio, filename, target))
            .await
    }
}

impl<Inner, P> VoiceRepository for Retry<Inner, P>
where
    Inner: VoiceRepository,
    P: RetryPolicy,
{
    async fn add(&self, name: &str, audio: &[u8], ref_text: Option<&str>) -> Result<()> {
        self.run(|| self.inner.add(name, audio, ref_text)).await
    }

    async fn list(&self) -> Result<Vec<Voice>> {
        self.run(|| self.inner.list()).await
    }

    async fn remove(&self, name: &str) -> Result<()> {
        self.run(|| self.inner.remove(name)).await
    }
}

impl<Inner, P> ServerProbe for Retry<Inner, P>
where
    Inner: ServerProbe,
    P: RetryPolicy,
{
    async fn health(&self) -> Result<bool> {
        self.run(|| self.inner.health()).await
    }

    async fn models(&self) -> Result<Vec<String>> {
        self.run(|| self.inner.models()).await
    }

    async fn supports_realtime(&self) -> Result<bool> {
        self.run(|| self.inner.supports_realtime()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::adapters::retry::HttpStatusError;
    use crate::domain::retry::RetryPolicy as Policy;

    /// A fake `Synthesizer` that fails `fail` times (with `kind`) before succeeding.
    struct FlakySynth {
        calls: AtomicU32,
        fail: u32,
        err: fn() -> anyhow::Error,
    }

    impl FlakySynth {
        fn new(fail: u32, err: fn() -> anyhow::Error) -> Self {
            Self {
                calls: AtomicU32::new(0),
                fail,
                err,
            }
        }
        fn count(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Synthesizer for FlakySynth {
        async fn synthesize(&self, _spec: &SpeechSpec) -> Result<SynthesizedAudio> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail {
                return Err((self.err)());
            }
            Ok(SynthesizedAudio {
                bytes: vec![1, 2, 3],
                content_type: "audio/mpeg".into(),
                rtf: None,
                audio_seconds: None,
            })
        }
    }

    fn fast_policy() -> Policy {
        Policy {
            max_retries: 3,
            backoff_initial_ms: 0,
            backoff_max_ms: 0,
            multiplier: 2.0,
            jitter: false,
            ..Policy::default()
        }
    }

    fn server_err() -> anyhow::Error {
        anyhow::anyhow!(HttpStatusError::new(503, "busy".into()))
    }

    fn fatal_err() -> anyhow::Error {
        anyhow::anyhow!(HttpStatusError::new(400, "bad request".into()))
    }

    fn spec() -> SpeechSpec {
        use crate::domain::voice::{StandardVoice, VoiceMode};
        SpeechSpec::builder("hi")
            .voice(VoiceMode::Standard(StandardVoice::new("alloy").unwrap()))
            .language(Language::parse("en").unwrap())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn retries_transient_5xx_until_success() {
        let inner = FlakySynth::new(2, server_err);
        let retry = Retry::new(inner, fast_policy(), Some(1));
        let out = retry.synthesize(&spec()).await.unwrap();
        assert_eq!(out.bytes, vec![1, 2, 3]);
        // Two failures + one success = three calls.
        assert_eq!(retry.inner().count(), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        // Always fails (5xx): 1 initial + 3 retries = 4 calls, then surfaces error.
        let inner = FlakySynth::new(u32::MAX, server_err);
        let retry = Retry::new(inner, fast_policy(), Some(1));
        assert!(retry.synthesize(&spec()).await.is_err());
        assert_eq!(retry.inner().count(), 4);
    }

    #[tokio::test]
    async fn non_retryable_error_fails_immediately() {
        let inner = FlakySynth::new(u32::MAX, fatal_err);
        let retry = Retry::new(inner, fast_policy(), Some(1));
        assert!(retry.synthesize(&spec()).await.is_err());
        // A 400 is not retryable: exactly one call.
        assert_eq!(retry.inner().count(), 1);
    }
}
