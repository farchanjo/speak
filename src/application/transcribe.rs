//! `transcribe` (file) use case (T041): speech-to-text over an uploaded file.
//!
//! A thin application seam over the `Transcriber` port (FR-6): it is the single
//! entry the CLI and the daemon share for file transcription, keeping the
//! driving adapters free of any direct adapter coupling. The output format and
//! language hint travel in the [`TranscribeRequest`]; the port decodes the
//! response into trimmed text.

use anyhow::Result;

use crate::ports::transcriber::{TranscribeRequest, Transcriber};

/// The `transcribe` use case over the [`Transcriber`] port.
pub struct TranscribeUseCase<'a, T> {
    transcriber: &'a T,
}

impl<'a, T> TranscribeUseCase<'a, T>
where
    T: Transcriber,
{
    /// Wire the use case to its port.
    #[must_use]
    pub fn new(transcriber: &'a T) -> Self {
        Self { transcriber }
    }

    /// Transcribe `req`'s audio into recognized text.
    pub async fn execute(&self, req: &TranscribeRequest<'_>) -> Result<String> {
        self.transcriber.transcribe(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::FakeSpeech;
    use crate::domain::language::Language;

    #[tokio::test]
    async fn returns_the_recognized_text() {
        let speech = FakeSpeech {
            transcript: "bom dia".to_owned(),
            ..FakeSpeech::default()
        };
        let lang = Language::parse("pt-BR").unwrap();
        let req = TranscribeRequest {
            audio: b"\x00\x01",
            filename: "clip.wav",
            language: Some(&lang),
            format: "json",
        };
        let text = TranscribeUseCase::new(&speech).execute(&req).await.unwrap();
        assert_eq!(text, "bom dia");
    }
}
