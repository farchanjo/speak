//! `Transcriber` driven port (T020).
//!
//! Speech-to-text over uploaded audio (FR-6). The openai adapter implements it
//! over `/v1/audio/transcriptions` (multipart); the retry decorator wraps it
//! (ADR-0004).

use anyhow::Result;

use crate::domain::language::Language;

/// Parameters for a transcription request.
pub struct TranscribeRequest<'a> {
    /// Raw audio bytes to upload.
    pub audio: &'a [u8],
    /// File name advertised in the multipart part.
    pub filename: &'a str,
    /// Optional source-language hint.
    pub language: Option<&'a Language>,
    /// Output format (`json|text|srt|vtt|verbose_json`).
    pub format: &'a str,
}

/// Driven port: transcribe uploaded audio into text.
#[expect(
    async_fn_in_trait,
    reason = "driven port consumed by generic retry decorators, not as a trait object (ADR-0004)"
)]
pub trait Transcriber {
    /// Transcribe `req`'s audio and return the recognized text.
    async fn transcribe(&self, req: &TranscribeRequest<'_>) -> Result<String>;
}
