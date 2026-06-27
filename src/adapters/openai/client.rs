//! The shared `openai` adapter transport (T030): one warm `async-openai` client
//! plus the same `reqwest` pool for the requests the typed API cannot express.

use anyhow::Result;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use reqwest::RequestBuilder;

use crate::config::Config;

/// The `openai` driven adapter: a warm transport implementing the
/// `Synthesizer`, `Transcriber`, `Translator`, and `VoiceRepository` ports.
///
/// Construction is the **Factory** step. It builds one tuned keep-alive
/// `reqwest` pool ([`crate::adapters::http::build_http_client`]) for the raw
/// extended-speech / `/tts` / voice-CRUD calls the typed API cannot express,
/// and lets `async-openai` build its own client for the typed transcription /
/// translation calls. (`async-openai` 0.41 links a different `reqwest` major
/// version than this crate, so a single `reqwest::Client` instance cannot back
/// both; unifying the pool is a composition-root concern, T054.)
pub struct OpenAiAdapter {
    /// Typed client for the standard `/v1/audio/*` endpoints.
    pub(super) openai: Client<OpenAIConfig>,
    /// Tuned warm pool for the non-typed (extended speech / `/tts` / voices) calls.
    pub(super) http: reqwest::Client,
    /// Server base URL, trailing slash trimmed, WITHOUT the `/v1` suffix.
    pub(super) base: String,
    /// Bearer key, sent only when configured.
    pub(super) api_key: Option<String>,
    /// TTS model id for the speech request (`[tts].model`).
    pub(super) tts_model: String,
    /// ASR model id for transcription/translation (`[asr].model`).
    pub(super) asr_model: String,
    /// Route `synthesize` through the native `/tts` endpoint instead of speech.
    pub(super) native: bool,
}

impl OpenAiAdapter {
    /// Build the adapter from resolved configuration (Factory).
    pub fn new(cfg: &Config) -> Result<Self> {
        let http = crate::adapters::http::build_http_client(&cfg.server)?;
        let base = cfg.server.host.trim_end_matches('/').to_owned();
        let config = OpenAIConfig::default()
            .with_api_base(format!("{base}/v1"))
            .with_api_key(cfg.server.api_key.clone().unwrap_or_default());
        Ok(Self {
            openai: Client::with_config(config),
            http,
            base,
            api_key: cfg.server.api_key.clone(),
            tts_model: cfg.tts.model.clone(),
            asr_model: cfg.asr.model.clone(),
            native: cfg.tts.native,
        })
    }

    /// Override the native-`/tts` routing for a per-call `say --native` (Builder).
    #[must_use]
    pub fn with_native(mut self, native: bool) -> Self {
        self.native = native;
        self
    }

    /// Compose the absolute URL for a server `endpoint` path.
    pub(super) fn url(&self, endpoint: &str) -> String {
        format!("{}{endpoint}", self.base)
    }

    /// Attach bearer auth when a key is configured (matching the flat client).
    pub(super) fn auth(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.api_key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    }

    /// Send `req` (with auth) and fail on any non-2xx status, surfacing the body.
    ///
    /// A non-2xx response is returned as a typed
    /// [`HttpStatusError`](crate::adapters::retry::HttpStatusError) so the retry
    /// decorator can classify `5xx`/`429` after the error crosses the `anyhow`
    /// boundary; the `Display` text is unchanged.
    pub(super) async fn send_ok(&self, req: RequestBuilder) -> Result<reqwest::Response> {
        let resp = self.auth(req).send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        Err(crate::adapters::retry::HttpStatusError::new(status.as_u16(), body).into())
    }
}

/// Read a response header as an owned string when present and valid UTF-8.
pub(super) fn header(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned)
}

/// Decode an ASR/translation response body into trimmed text.
///
/// `json`/`verbose_json` carry the transcript in a `text` field; the subtitle
/// and plain formats are returned verbatim (trimmed).
pub(super) fn decode_text(bytes: &[u8], format: &str) -> String {
    let body = String::from_utf8_lossy(bytes);
    if matches!(format, "json" | "verbose_json")
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&body)
        && let Some(text) = value.get("text").and_then(serde_json::Value::as_str)
    {
        return text.trim().to_owned();
    }
    body.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_text_extracts_json_text_field() {
        assert_eq!(decode_text(br#"{"text":"  hi  "}"#, "json"), "hi");
        assert_eq!(
            decode_text(r#"{"text":"olá"}"#.as_bytes(), "verbose_json"),
            "olá"
        );
    }

    #[test]
    fn decode_text_passes_plain_formats_through_trimmed() {
        assert_eq!(decode_text(b"  plain text \n", "text"), "plain text");
        assert_eq!(decode_text(b"1\n00:00:01\n", "srt"), "1\n00:00:01");
    }

    #[test]
    fn decode_text_falls_back_when_json_lacks_text() {
        assert_eq!(decode_text(br#"{"other":1}"#, "json"), r#"{"other":1}"#);
    }
}
