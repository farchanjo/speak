//! `translate` (file) use case (T041): foreign-language audio to text.
//!
//! A thin application seam over the `Translator` **Strategy** port (FR-7): the
//! composition root injects either the openai/Whisper English strategy or the
//! chat-MT arbitrary-target strategy, and this use case drives whichever it
//! holds. The CLI and daemon share this single entry; the target [`Language`]
//! selects the spoken result language.

use anyhow::Result;

use crate::domain::language::Language;
use crate::ports::translator::Translator;

/// The `translate` use case over the [`Translator`] Strategy port.
pub struct TranslateUseCase<'a, T> {
    translator: &'a T,
}

impl<'a, T> TranslateUseCase<'a, T>
where
    T: Translator,
{
    /// Wire the use case to its port.
    #[must_use]
    pub fn new(translator: &'a T) -> Self {
        Self { translator }
    }

    /// Translate `audio` (advertised as `filename`) into `target`-language text.
    pub async fn execute(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String> {
        self.translator.translate(audio, filename, target).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::FakeSpeech;

    #[tokio::test]
    async fn returns_the_translated_text() {
        let speech = FakeSpeech {
            translation: "good morning".to_owned(),
            ..FakeSpeech::default()
        };
        let target = Language::parse("en").unwrap();
        let text = TranslateUseCase::new(&speech)
            .execute(b"\x00\x01", "clip.wav", &target)
            .await
            .unwrap();
        assert_eq!(text, "good morning");
    }
}
