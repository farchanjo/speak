//! `Translator` (OpenAI/Whisper Strategy) over the typed `async-openai`
//! `/v1/audio/translations` request group (T030).
//!
//! This is the default English-target Strategy (ADR-0004): Whisper translates
//! foreign-language audio to English text. A non-English `--to` target selects
//! the separate chat-MT Strategy (the `chatmt` adapter, T039) at the composition
//! root, so this implementation always drives the English endpoint and decodes
//! the transcript text.

use anyhow::Result;
use async_openai::types::InputSource;
use async_openai::types::audio::{AudioInput, CreateTranslationRequest, TranslationResponseFormat};

use super::client::{OpenAiAdapter, decode_text};
use crate::domain::language::Language;
use crate::ports::translator::Translator;

impl Translator for OpenAiAdapter {
    async fn translate(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String> {
        if !target.is_english() {
            tracing::debug!(
                target = target.as_str(),
                "openai translator only emits English (Whisper); non-English needs chat-MT"
            );
        }
        let request = CreateTranslationRequest {
            file: AudioInput {
                source: InputSource::VecU8 {
                    filename: filename.to_owned(),
                    vec: audio.to_vec(),
                },
            },
            model: self.asr_model.clone(),
            response_format: Some(TranslationResponseFormat::Json),
            ..Default::default()
        };
        let bytes = self
            .openai
            .audio()
            .translation()
            .create_raw(request)
            .await?;
        Ok(decode_text(&bytes, "json"))
    }
}
