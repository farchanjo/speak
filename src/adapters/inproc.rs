//! `inproc` driven adapter (ADR-0010): the in-process warm speech stack.
//!
//! One reusable composite that bundles the retry-wrapped `openai` adapter (all
//! five driven network ports) with the optional chat-MT translate **Strategy**
//! and routes [`Translator::translate`] by target language: an English target
//! stays on Whisper translate; a non-English target uses chat-MT when
//! `[http].translate_url` is configured, else degrades to the source transcript
//! (FR-8 / ADR-0004).
//!
//! Both in-process callers share it so the routing lives in exactly one place:
//! the CLI's `SpeechRole::Direct` (foreground one-shot) and the daemon's warm
//! [`SpeakFacade`](crate::application::SpeakFacade). Previously the daemon facade
//! held a bare `Retry<OpenAiAdapter>`, so a forwarded `translate`/`realtime`
//! request to a non-English `--to` was silently answered by Whisper's
//! English-only path; routing the daemon through this composite makes a forwarded
//! request honour `--to` exactly like the in-process path (ADR-0010).

use anyhow::Result;

use crate::adapters::chatmt::ChatMtTranslator;
use crate::adapters::config::Config;
use crate::adapters::openai::OpenAiAdapter;
use crate::adapters::retry::Retry;
use crate::domain::language::Language;
use crate::domain::speech_spec::SpeechSpec;
use crate::domain::voice::Voice;
use crate::ports::probe::ServerProbe;
use crate::ports::synthesizer::{SynthesizedAudio, Synthesizer};
use crate::ports::transcriber::{TranscribeRequest, Transcriber};
use crate::ports::translator::Translator;
use crate::ports::voice::VoiceRepository;

/// The in-process warm speech stack: retry-wrapped `openai` for every port plus
/// the chat-MT translate Strategy selected per target language.
pub struct InProcessSpeech {
    /// The retry-wrapped `openai` adapter (all five driven ports).
    speech: Retry<OpenAiAdapter>,
    /// Arbitrary-target chat-MT translator; `None` when `[http].translate_url`
    /// is unset, in which case a non-English target degrades to the transcript.
    chatmt: Option<ChatMtTranslator<OpenAiAdapter>>,
}

impl InProcessSpeech {
    /// Build the stack from resolved config (Factory).
    ///
    /// `native` forces the in-process `say` through the server's `/tts` endpoint
    /// (a `say --native`); it is OR-ed with the `[tts].native` default.
    pub fn new(cfg: &Config, native: bool) -> Result<Self> {
        let openai = OpenAiAdapter::new(cfg)?.with_native(native || cfg.tts.native);
        let speech = Retry::new(openai, cfg.retry.policy, cfg.retry.jitter_seed);
        let chatmt = ChatMtTranslator::new(OpenAiAdapter::new(cfg)?, cfg)?;
        Ok(Self { speech, chatmt })
    }

    /// Route translation by target: English -> Whisper translate; non-English ->
    /// chat-MT when configured, else degrade to the source transcript (FR-8).
    async fn route_translate(
        &self,
        audio: &[u8],
        filename: &str,
        target: &Language,
    ) -> Result<String> {
        if target.is_english() {
            return self.speech.translate(audio, filename, target).await;
        }
        match &self.chatmt {
            Some(mt) => mt.translate(audio, filename, target).await,
            None => {
                self.speech
                    .transcribe(&TranscribeRequest {
                        audio,
                        filename,
                        language: None,
                        format: "json",
                    })
                    .await
            }
        }
    }
}

impl Synthesizer for InProcessSpeech {
    async fn synthesize(&self, spec: &SpeechSpec) -> Result<SynthesizedAudio> {
        self.speech.synthesize(spec).await
    }
}

impl Transcriber for InProcessSpeech {
    async fn transcribe(&self, req: &TranscribeRequest<'_>) -> Result<String> {
        self.speech.transcribe(req).await
    }
}

impl Translator for InProcessSpeech {
    async fn translate(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String> {
        self.route_translate(audio, filename, target).await
    }
}

impl VoiceRepository for InProcessSpeech {
    async fn add(&self, name: &str, audio: &[u8], ref_text: Option<&str>) -> Result<()> {
        self.speech.add(name, audio, ref_text).await
    }

    async fn list(&self) -> Result<Vec<Voice>> {
        self.speech.list().await
    }

    async fn remove(&self, name: &str) -> Result<()> {
        self.speech.remove(name).await
    }
}

impl ServerProbe for InProcessSpeech {
    async fn health(&self) -> Result<bool> {
        self.speech.health().await
    }

    async fn models(&self) -> Result<Vec<String>> {
        self.speech.models().await
    }

    async fn supports_realtime(&self) -> Result<bool> {
        self.speech.supports_realtime().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::config::GlobalFlags;
    use crate::testenv::ENV_LOCK;

    /// Construction wires the retry-wrapped `openai` adapter without a chat-MT
    /// strategy when `[http].translate_url` is unset (the degrade-to-transcript
    /// arm), and with one when it is set. No network is touched.
    #[test]
    fn new_selects_chat_mt_strategy_from_config() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("SPEAK_TRANSLATE_URL") };
        let cfg = Config::load(GlobalFlags::default()).unwrap();
        let stack = InProcessSpeech::new(&cfg, false).unwrap();
        assert!(
            stack.chatmt.is_none(),
            "no chat-MT strategy without translate_url"
        );

        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("SPEAK_TRANSLATE_URL", "http://mt.example/v1") };
        let cfg = Config::load(GlobalFlags::default()).unwrap();
        let stack = InProcessSpeech::new(&cfg, false).unwrap();
        assert!(
            stack.chatmt.is_some(),
            "chat-MT strategy present when translate_url is configured"
        );
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("SPEAK_TRANSLATE_URL") };
    }
}
