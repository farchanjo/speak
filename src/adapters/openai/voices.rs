//! `VoiceRepository` over the server's non-OpenAI `POST/GET/DELETE /voices`
//! surface (T032).
//!
//! `async-openai`'s typed voices group targets `/audio/voices` and only creates
//! a voice, so the saved-voice **Repository** drives the server's own `/voices`
//! endpoints directly over the shared warm pool.

use anyhow::{Context, Result};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use super::client::OpenAiAdapter;
use crate::domain::voice::Voice;
use crate::ports::voice::VoiceRepository;

/// One entry of the server's `GET /voices` response.
#[derive(Debug, Deserialize)]
struct VoiceDto {
    name: String,
    #[serde(default)]
    has_ref_text: bool,
}

/// The `GET /voices` envelope.
#[derive(Debug, Deserialize)]
struct VoicesEnvelope {
    #[serde(default)]
    voices: Vec<VoiceDto>,
}

impl VoiceRepository for OpenAiAdapter {
    async fn add(&self, name: &str, audio: &[u8], ref_text: Option<&str>) -> Result<()> {
        let mut form = Form::new().text("name", name.to_owned());
        if let Some(text) = ref_text {
            form = form.text("ref_text", text.to_owned());
        }
        let part = Part::bytes(audio.to_vec())
            .file_name(format!("{name}.wav"))
            .mime_str("application/octet-stream")
            .context("building voice reference part")?;
        form = form.part("audio", part);
        self.send_ok(self.http.post(self.url("/voices")).multipart(form))
            .await?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Voice>> {
        let resp = self.send_ok(self.http.get(self.url("/voices"))).await?;
        let envelope: VoicesEnvelope = resp.json().await.context("parsing /voices response")?;
        envelope
            .voices
            .into_iter()
            .map(|v| Voice::new(&v.name, v.has_ref_text).map_err(Into::into))
            .collect()
    }

    async fn remove(&self, name: &str) -> Result<()> {
        self.send_ok(self.http.delete(self.url(&format!("/voices/{name}"))))
            .await?;
        Ok(())
    }
}
