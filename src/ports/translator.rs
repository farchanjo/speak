//! `Translator` driven port (T020).
//!
//! A **Strategy** with two interchangeable implementations (ADR-0004): the
//! openai/Whisper translate path (English target, `/v1/audio/translations`) and
//! the chat-MT path (arbitrary `--to` target over `[http].translate_url`). The
//! composition root selects the strategy; the use case is unaware which one it
//! holds. The retry decorator wraps either.

use anyhow::Result;

use crate::domain::language::Language;

/// Driven port (Strategy): translate uploaded audio into text in `target`.
#[expect(
    async_fn_in_trait,
    reason = "driven port consumed by generic retry decorators, not as a trait object (ADR-0004)"
)]
pub trait Translator {
    /// Translate `audio` (advertised as `filename`) into `target`-language text.
    async fn translate(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String>;
}
