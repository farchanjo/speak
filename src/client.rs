//! HTTP client for the OpenAI-compatible speech server (reqwest, async).
//!
//! Endpoints: `/health`, `/v1/audio/speech`, native `/tts`,
//! `/v1/audio/transcriptions`, `/v1/audio/translations`, and an optional
//! chat-completions endpoint for arbitrary-target translation.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Map, Value};

use crate::config::Config;

/// Async speech client bound to a single server + optional bearer key.
pub struct SpeechClient {
    http: Client,
    base: String,
    api_key: Option<String>,
}

/// Parameters for a TTS request. Optional fields select the server's voice
/// modes: `voice` => clone a saved voice; `instruct` => voice design; neither
/// => auto. `extra` carries pass-through generation params.
pub struct SpeakRequest<'a> {
    /// Text to synthesize.
    pub input: &'a str,
    /// Model id.
    pub model: &'a str,
    /// Saved voice name for cloning (omit for design/auto).
    pub voice: Option<&'a str>,
    /// OpenAI `response_format` (mp3/opus/aac/flac/wav/pcm).
    pub response_format: &'a str,
    /// Playback speed multiplier.
    pub speed: f32,
    /// Language hint.
    pub language: &'a str,
    /// Voice-design tags (comma-separated canonical tags).
    pub instruct: Option<&'a str>,
    /// Reference transcript for cloning.
    pub ref_text: Option<&'a str>,
    /// Target duration hint in seconds.
    pub duration: Option<f32>,
    /// Pass-through generation params (validated by the caller).
    pub extra: Map<String, Value>,
}

impl SpeakRequest<'_> {
    fn to_body(&self) -> Value {
        let mut body = Map::new();
        body.insert("input".into(), json!(self.input));
        body.insert("model".into(), json!(self.model));
        body.insert("response_format".into(), json!(self.response_format));
        body.insert("speed".into(), json!(self.speed));
        body.insert("language".into(), json!(self.language));
        insert_opt(&mut body, "voice", self.voice);
        insert_opt(&mut body, "instruct", self.instruct);
        insert_opt(&mut body, "ref_text", self.ref_text);
        if let Some(d) = self.duration {
            body.insert("duration".into(), json!(d));
        }
        for (k, v) in &self.extra {
            body.insert(k.clone(), v.clone());
        }
        Value::Object(body)
    }
}

fn insert_opt(body: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        body.insert(key.to_owned(), json!(v));
    }
}

/// Audio bytes plus server-reported metadata headers.
pub struct AudioReply {
    /// Raw encoded audio bytes.
    pub bytes: Vec<u8>,
    /// Codec hint from the `Content-Type` header.
    pub content_type: String,
    /// `X-RTF` (real-time factor), when present.
    pub rtf: Option<String>,
    /// `X-Audio-Seconds` (synthesised duration), when present.
    pub audio_seconds: Option<String>,
}

/// A saved voice entry from `GET /voices`.
#[derive(Debug, serde::Deserialize)]
pub struct VoiceInfo {
    /// Voice name.
    pub name: String,
    /// Whether a reference transcript is stored.
    #[serde(default)]
    pub has_ref_text: bool,
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
        let resp = self
            .auth(self.http.post(self.url("/v1/audio/speech")).json(&req.to_body()))
            .send()
            .await?;
        audio_reply(resp).await
    }

    /// `GET /voices` -> saved voices for cloning.
    pub async fn list_voices(&self) -> Result<Vec<VoiceInfo>> {
        let resp = self.auth(self.http.get(self.url("/voices"))).send().await?;
        let value: Value = ensure_ok(resp).await?.json().await?;
        let voices = value.get("voices").cloned().unwrap_or(Value::Null);
        serde_json::from_value(voices).context("parsing /voices response")
    }

    /// `POST /voices` (multipart) -> save a voice for cloning.
    pub async fn add_voice(
        &self,
        name: &str,
        audio: Vec<u8>,
        filename: &str,
        ref_text: Option<&str>,
    ) -> Result<String> {
        let part = Part::bytes(audio)
            .file_name(filename.to_owned())
            .mime_str("application/octet-stream")
            .context("building voice audio part")?;
        let mut form = Form::new().text("name", name.to_owned()).part("audio", part);
        if let Some(text) = ref_text {
            form = form.text("ref_text", text.to_owned());
        }
        let resp = self
            .auth(self.http.post(self.url("/voices")).multipart(form))
            .send()
            .await?;
        Ok(ensure_ok(resp).await?.text().await?)
    }

    /// `DELETE /voices/{name}` -> remove a saved voice.
    pub async fn delete_voice(&self, name: &str) -> Result<String> {
        let resp = self
            .auth(self.http.delete(self.url(&format!("/voices/{name}"))))
            .send()
            .await?;
        Ok(ensure_ok(resp).await?.text().await?)
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
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned)
    };
    let content_type = header("content-type").unwrap_or_else(|| "application/octet-stream".to_owned());
    let rtf = header("x-rtf");
    let audio_seconds = header("x-audio-seconds");
    let bytes = resp.bytes().await?.to_vec();
    if bytes.is_empty() {
        bail!("server returned empty audio body");
    }
    Ok(AudioReply { bytes, content_type, rtf, audio_seconds })
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
