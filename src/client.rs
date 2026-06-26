//! HTTP client for the OpenAI-compatible speech server (reqwest, async).
//!
//! Endpoints: `/health`, `/v1/audio/speech`, native `/tts`,
//! `/v1/audio/transcriptions`, `/v1/audio/translations`, and an optional
//! chat-completions endpoint for arbitrary-target translation.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};

use crate::config::Config;

/// Async speech client bound to a single server + optional bearer key.
pub struct SpeechClient {
    http: Client,
    base: String,
    api_key: Option<String>,
}

/// Parameters for a TTS request.
pub struct SpeakRequest<'a> {
    /// Text to synthesize.
    pub input: &'a str,
    /// Model id.
    pub model: &'a str,
    /// Voice id.
    pub voice: &'a str,
    /// OpenAI `response_format` (mp3/opus/aac/flac/wav/pcm).
    pub response_format: &'a str,
    /// Playback speed multiplier.
    pub speed: f32,
    /// Language hint.
    pub language: &'a str,
}

/// Audio bytes plus the server-reported `Content-Type`.
pub struct AudioReply {
    /// Raw encoded audio bytes.
    pub bytes: Vec<u8>,
    /// Codec hint from the `Content-Type` header.
    pub content_type: String,
}

impl SpeechClient {
    /// Build a client from resolved configuration.
    pub fn new(cfg: &Config) -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!("speak/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            base: cfg.host.trim_end_matches('/').to_owned(),
            api_key: cfg.api_key.clone(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    }

    /// `GET /health` -> parsed JSON.
    pub async fn health(&self) -> Result<Value> {
        let resp = self.auth(self.http.get(self.url("/health"))).send().await?;
        let resp = ensure_ok(resp).await?;
        resp.json().await.context("parsing /health JSON")
    }

    /// `POST /v1/audio/speech` -> encoded audio bytes.
    pub async fn speak(&self, req: &SpeakRequest<'_>) -> Result<AudioReply> {
        let body = json!({
            "model": req.model,
            "input": req.input,
            "voice": req.voice,
            "response_format": req.response_format,
            "speed": req.speed,
            "language": req.language,
        });
        let resp = self
            .auth(self.http.post(self.url("/v1/audio/speech")).json(&body))
            .send()
            .await?;
        audio_reply(resp).await
    }

    /// `POST /tts` (native) -> WAV bytes.
    pub async fn speak_native(&self, text: &str, language: &str, speed: f32) -> Result<AudioReply> {
        let body = json!({ "text": text, "language": language, "speed": speed });
        let resp = self
            .auth(self.http.post(self.url("/tts")).json(&body))
            .send()
            .await?;
        audio_reply(resp).await
    }

    /// `POST /v1/audio/transcriptions` (multipart) -> transcript text.
    pub async fn transcribe(
        &self,
        audio: Vec<u8>,
        filename: &str,
        model: &str,
        language: Option<&str>,
        format: &str,
    ) -> Result<String> {
        let mut form = audio_form(audio, filename, model, format)?;
        if let Some(lang) = language {
            form = form.text("language", lang.to_owned());
        }
        let resp = self
            .auth(self.http.post(self.url("/v1/audio/transcriptions")).multipart(form))
            .send()
            .await?;
        text_reply(resp, format).await
    }

    /// `POST /v1/audio/translations` (multipart) -> English text.
    pub async fn translate(
        &self,
        audio: Vec<u8>,
        filename: &str,
        model: &str,
        format: &str,
    ) -> Result<String> {
        let form = audio_form(audio, filename, model, format)?;
        let resp = self
            .auth(self.http.post(self.url("/v1/audio/translations")).multipart(form))
            .send()
            .await?;
        text_reply(resp, format).await
    }

    /// Optional chat-completions translation to an arbitrary target language.
    pub async fn chat_translate(
        &self,
        url: &str,
        model: &str,
        text: &str,
        target: &str,
    ) -> Result<String> {
        let body = json!({
            "model": model,
            "messages": [
                { "role": "system",
                  "content": format!("Translate the user message into {target}. Reply with only the translation.") },
                { "role": "user", "content": text },
            ],
        });
        let resp = self.auth(self.http.post(url).json(&body)).send().await?;
        let value: Value = ensure_ok(resp).await?.json().await?;
        value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("chat translation response missing choices[0].message.content"))
    }
}

fn audio_form(audio: Vec<u8>, filename: &str, model: &str, format: &str) -> Result<Form> {
    let part = Part::bytes(audio)
        .file_name(filename.to_owned())
        .mime_str("application/octet-stream")
        .context("building multipart audio part")?;
    Ok(Form::new()
        .text("model", model.to_owned())
        .text("response_format", format.to_owned())
        .part("file", part))
}

async fn audio_reply(resp: reqwest::Response) -> Result<AudioReply> {
    let resp = ensure_ok(resp).await?;
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let bytes = resp.bytes().await?.to_vec();
    if bytes.is_empty() {
        bail!("server returned empty audio body");
    }
    Ok(AudioReply { bytes, content_type })
}

async fn text_reply(resp: reqwest::Response, format: &str) -> Result<String> {
    let resp = ensure_ok(resp).await?;
    let body = resp.text().await?;
    if matches!(format, "json" | "verbose_json") {
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            if let Some(text) = value.get("text").and_then(Value::as_str) {
                return Ok(text.trim().to_owned());
            }
        }
    }
    Ok(body.trim().to_owned())
}

async fn ensure_ok(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    bail!("server returned {status}: {}", body.trim());
}
