//! Composition-root `Speech` role selector (T053): in-process vs daemon-forward.
//!
//! The Factory injects ONE of these as the Facade's `Speech` role. Both arms
//! implement the same five driven network ports, so every use case runs
//! identically whether the speech calls execute in-process (the retry-wrapped
//! `openai` adapter over the local warm pool) or are forwarded to a running warm
//! daemon over its Unix socket ([`DaemonSpeechAdapter`]). Local audio stays in the
//! foreground CLI regardless — only the network speech ports are switched.

use anyhow::Result;

use speak::adapters::openai::OpenAiAdapter;
use speak::adapters::retry::Retry;
use speak::daemon::DaemonSpeechAdapter;
use speak::domain::language::Language;
use speak::domain::speech_spec::SpeechSpec;
use speak::domain::voice::Voice;
use speak::ports::probe::ServerProbe;
use speak::ports::synthesizer::{SynthesizedAudio, Synthesizer};
use speak::ports::transcriber::{TranscribeRequest, Transcriber};
use speak::ports::translator::Translator;
use speak::ports::voice::VoiceRepository;

/// The interchangeable `Speech` adapter role (Strategy) the composition root
/// selects at dispatch time. `Direct` is boxed because the in-process adapter is
/// far larger than the thin socket forwarder.
pub enum SpeechRole {
    /// In-process: the retry-wrapped `openai` adapter over the local warm pool.
    Direct(Box<Retry<OpenAiAdapter>>),
    /// Forwarded: every speech-port call rides a running daemon's warm pool.
    Daemon(DaemonSpeechAdapter),
}

impl Synthesizer for SpeechRole {
    async fn synthesize(&self, spec: &SpeechSpec) -> Result<SynthesizedAudio> {
        match self {
            Self::Direct(a) => a.synthesize(spec).await,
            Self::Daemon(a) => a.synthesize(spec).await,
        }
    }
}

impl Transcriber for SpeechRole {
    async fn transcribe(&self, req: &TranscribeRequest<'_>) -> Result<String> {
        match self {
            Self::Direct(a) => a.transcribe(req).await,
            Self::Daemon(a) => a.transcribe(req).await,
        }
    }
}

impl Translator for SpeechRole {
    async fn translate(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String> {
        match self {
            Self::Direct(a) => a.translate(audio, filename, target).await,
            Self::Daemon(a) => a.translate(audio, filename, target).await,
        }
    }
}

impl VoiceRepository for SpeechRole {
    async fn add(&self, name: &str, audio: &[u8], ref_text: Option<&str>) -> Result<()> {
        match self {
            Self::Direct(a) => a.add(name, audio, ref_text).await,
            Self::Daemon(a) => a.add(name, audio, ref_text).await,
        }
    }

    async fn list(&self) -> Result<Vec<Voice>> {
        match self {
            Self::Direct(a) => a.list().await,
            Self::Daemon(a) => a.list().await,
        }
    }

    async fn remove(&self, name: &str) -> Result<()> {
        match self {
            Self::Direct(a) => a.remove(name).await,
            Self::Daemon(a) => a.remove(name).await,
        }
    }
}

impl ServerProbe for SpeechRole {
    async fn health(&self) -> Result<bool> {
        match self {
            Self::Direct(a) => a.health().await,
            Self::Daemon(a) => a.health().await,
        }
    }

    async fn models(&self) -> Result<Vec<String>> {
        match self {
            Self::Direct(a) => a.models().await,
            Self::Daemon(a) => a.models().await,
        }
    }

    async fn supports_realtime(&self) -> Result<bool> {
        match self {
            Self::Direct(a) => a.supports_realtime().await,
            Self::Daemon(a) => a.supports_realtime().await,
        }
    }
}
