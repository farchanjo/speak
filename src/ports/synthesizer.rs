//! `Synthesizer` driven port (T020).
//!
//! Turns a validated [`SpeechSpec`] aggregate into encoded audio (FR-1). The
//! openai adapter implements it over `/v1/audio/speech` (`_byot`) and native
//! `/tts`; the retry decorator wraps it transparently (ADR-0004). No framework
//! type crosses this boundary — only the domain aggregate in, owned bytes out.

use anyhow::Result;

use crate::domain::speech_spec::SpeechSpec;

/// Synthesized audio bytes plus the server's inference-timing metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesizedAudio {
    /// Encoded audio bytes in the requested [`crate::domain::audio_format::AudioFormat`].
    pub bytes: Vec<u8>,
    /// Codec hint from the response `Content-Type`.
    pub content_type: String,
    /// `X-RTF` real-time factor, surfaced on `--json` (FR-1).
    pub rtf: Option<String>,
    /// `X-Audio-Seconds` synthesized duration, surfaced on `--json` (FR-1).
    pub audio_seconds: Option<String>,
}

/// Driven port: synthesize speech from a [`SpeechSpec`].
#[expect(
    async_fn_in_trait,
    reason = "driven port consumed by generic retry decorators, not as a trait object (ADR-0004)"
)]
pub trait Synthesizer {
    /// Synthesize `spec` and return the encoded audio with its metadata.
    async fn synthesize(&self, spec: &SpeechSpec) -> Result<SynthesizedAudio>;
}
