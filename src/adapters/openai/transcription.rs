//! `Transcriber` over the typed `async-openai` `/v1/audio/transcriptions`
//! request group (T030).

use anyhow::Result;
use async_openai::types::InputSource;
use async_openai::types::audio::{AudioInput, AudioResponseFormat, CreateTranscriptionRequest};

use super::client::{OpenAiAdapter, decode_text};
use crate::ports::transcriber::{TranscribeRequest, Transcriber};

/// Map the wire `response_format` token to the typed request enum.
fn response_format(format: &str) -> AudioResponseFormat {
    match format {
        "text" => AudioResponseFormat::Text,
        "srt" => AudioResponseFormat::Srt,
        "vtt" => AudioResponseFormat::Vtt,
        "verbose_json" => AudioResponseFormat::VerboseJson,
        _ => AudioResponseFormat::Json,
    }
}

impl Transcriber for OpenAiAdapter {
    async fn transcribe(&self, req: &TranscribeRequest<'_>) -> Result<String> {
        let request = CreateTranscriptionRequest {
            file: AudioInput {
                source: InputSource::VecU8 {
                    filename: req.filename.to_owned(),
                    vec: req.audio.to_vec(),
                },
            },
            model: self.asr_model.clone(),
            language: req.language.map(|l| l.as_str().to_owned()),
            response_format: Some(response_format(req.format)),
            ..Default::default()
        };
        let bytes = self
            .openai
            .audio()
            .transcription()
            .create_raw(request)
            .await?;
        Ok(decode_text(&bytes, req.format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_formats_else_json() {
        assert_eq!(response_format("text"), AudioResponseFormat::Text);
        assert_eq!(response_format("srt"), AudioResponseFormat::Srt);
        assert_eq!(response_format("vtt"), AudioResponseFormat::Vtt);
        assert_eq!(
            response_format("verbose_json"),
            AudioResponseFormat::VerboseJson
        );
        assert_eq!(response_format("json"), AudioResponseFormat::Json);
        assert_eq!(response_format("bogus"), AudioResponseFormat::Json);
    }
}
