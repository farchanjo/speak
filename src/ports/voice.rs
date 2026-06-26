//! `VoiceRepository` driven port (T022).
//!
//! A **Repository** abstracting saved-voice persistence on the server (FR-5):
//! `POST /voices` (multipart `name,audio,ref_text?`), `GET /voices`, and
//! `DELETE /voices/{name}`. The openai adapter implements it; the retry
//! decorator wraps it (ADR-0004).

use anyhow::Result;

use crate::domain::voice::Voice;

/// Driven port: register, list, and delete cloneable voices.
#[expect(
    async_fn_in_trait,
    reason = "driven port consumed by generic retry decorators, not as a trait object (ADR-0004)"
)]
pub trait VoiceRepository {
    /// Register a voice from reference `audio` with an optional `ref_text`.
    async fn add(&self, name: &str, audio: &[u8], ref_text: Option<&str>) -> Result<()>;

    /// List the saved voices.
    async fn list(&self) -> Result<Vec<Voice>>;

    /// Delete the saved voice named `name`.
    async fn remove(&self, name: &str) -> Result<()>;
}
