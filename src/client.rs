//! HTTP core for the OpenAI-compatible speech server (reqwest, async).
//!
//! Everything goes through a small generic proxy (`proxy` / `proxy_multipart`)
//! that returns a [`ProxyReply`]. The same proxy is used directly by the CLI
//! and, over a Unix socket, by the [`crate::daemon`] — so a request takes the
//! identical shape whether it runs in-process or through the warm daemon.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;
use crate::domain::retry::{ErrorKind, RetryPolicy};

/// HTTP core bound to a single server + optional bearer key, with retry.
pub struct SpeechClient {
    http: Client,
    base: String,
    api_key: Option<String>,
    policy: RetryPolicy,
    jitter_seed: Option<u64>,
}

/// Parameters for a TTS request. `voice` => clone a saved voice; `instruct`
/// => voice design; neither => auto. `extra` carries generation params.
pub struct SpeakRequest<'a> {
    /// Text to synthesize.
    pub input: &'a str,
    /// Model id.
    pub model: &'a str,
    /// Saved voice name (omit for design/auto).
    pub voice: Option<&'a str>,
    /// OpenAI `response_format`.
    pub response_format: &'a str,
    /// Speed multiplier.
    pub speed: f32,
    /// Language hint.
    pub language: &'a str,
    /// Voice-design tags.
    pub instruct: Option<&'a str>,
    /// Reference transcript for cloning.
    pub ref_text: Option<&'a str>,
    /// Target duration hint (seconds).
    pub duration: Option<f32>,
    /// Pass-through generation params.
    pub extra: serde_json::Map<String, Value>,
}

impl SpeakRequest<'_> {
    /// Build the JSON request body.
    #[must_use]
    pub fn to_body(&self) -> Value {
        let mut body = serde_json::Map::new();
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

fn insert_opt(body: &mut serde_json::Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        body.insert(key.to_owned(), json!(v));
    }
}

/// Audio bytes plus server metadata headers.
pub struct AudioReply {
    /// Encoded audio bytes.
    pub bytes: Vec<u8>,
    /// Codec hint from `Content-Type`.
    pub content_type: String,
    /// `X-RTF` real-time factor.
    pub rtf: Option<String>,
    /// `X-Audio-Seconds` synthesised duration.
    pub audio_seconds: Option<String>,
}

/// A saved voice entry from `GET /voices`.
#[derive(Debug, Deserialize)]
pub struct VoiceInfo {
    /// Voice name.
    pub name: String,
    /// Whether a reference transcript is stored.
    #[serde(default)]
    pub has_ref_text: bool,
}

/// A multipart field (text part).
pub type Field = (String, String);

/// Generic proxied HTTP reply (status + headers + raw body).
#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyReply {
    /// HTTP status code.
    pub status: u16,
    /// `Content-Type` header.
    pub content_type: String,
    /// `X-RTF` header.
    pub rtf: Option<String>,
    /// `X-Audio-Seconds` header.
    pub audio_seconds: Option<String>,
    /// Raw response body.
    pub body: Vec<u8>,
}

impl ProxyReply {
    fn ensure_ok(self) -> Result<Self> {
        if (200..300).contains(&self.status) {
            return Ok(self);
        }
        bail!(
            "server returned {}: {}",
            self.status,
            String::from_utf8_lossy(&self.body).trim()
        );
    }

    /// Interpret the reply as encoded audio.
    pub fn into_audio(self) -> Result<AudioReply> {
        let reply = self.ensure_ok()?;
        if reply.body.is_empty() {
            bail!("server returned empty audio body");
        }
        Ok(AudioReply {
            bytes: reply.body,
            content_type: reply.content_type,
            rtf: reply.rtf,
            audio_seconds: reply.audio_seconds,
        })
    }

    /// Interpret the reply as transcript/translation text for `format`.
    pub fn into_text(self, format: &str) -> Result<String> {
        let reply = self.ensure_ok()?;
        let body = String::from_utf8_lossy(&reply.body).into_owned();
        if matches!(format, "json" | "verbose_json")
            && let Ok(value) = serde_json::from_str::<Value>(&body)
            && let Some(text) = value.get("text").and_then(Value::as_str)
        {
            return Ok(text.trim().to_owned());
        }
        Ok(body.trim().to_owned())
    }

    /// Interpret the reply as JSON.
    pub fn into_json(self) -> Result<Value> {
        let reply = self.ensure_ok()?;
        serde_json::from_slice(&reply.body).context("parsing JSON response")
    }

    /// Interpret the reply as trimmed UTF-8 text.
    pub fn into_string(self) -> Result<String> {
        let reply = self.ensure_ok()?;
        Ok(String::from_utf8_lossy(&reply.body).trim().to_owned())
    }
}

/// Build one warm, pooled [`reqwest::Client`] from the `[server]` tuning knobs.
///
/// Shared by the flat [`SpeechClient`] and the `openai` adapter so both reuse an
/// identically-configured keep-alive pool (ADR-0004). The adapter passes a clone
/// of this client to `async-openai` *and* keeps one for the extended speech /
/// `/tts` / voice-CRUD requests the typed API cannot express, so a single warm
/// connection pool backs every call the adapter makes.
pub fn build_http_client(s: &crate::config::Server) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(s.user_agent.clone())
        .pool_max_idle_per_host(s.pool_max_idle)
        .pool_idle_timeout(Duration::from_secs(s.pool_idle_timeout))
        .tcp_keepalive(Duration::from_secs(s.tcp_keepalive))
        .tcp_nodelay(true)
        .connect_timeout(Duration::from_secs(s.connect_timeout))
        .timeout(Duration::from_secs(s.timeout));
    if s.http2 {
        builder = builder.http2_prior_knowledge();
    }
    builder.build().context("building HTTP client")
}

impl SpeechClient {
    /// Build a client from resolved configuration.
    pub fn new(cfg: &Config) -> Result<Self> {
        let s = &cfg.server;
        Ok(Self {
            http: build_http_client(s)?,
            base: s.host.trim_end_matches('/').to_owned(),
            api_key: s.api_key.clone(),
            policy: cfg.retry.policy,
            jitter_seed: cfg.retry.jitter_seed,
        })
    }

    fn url(&self, endpoint: &str) -> String {
        if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.to_owned()
        } else {
            format!("{}{endpoint}", self.base)
        }
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    }

    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        loop {
            let Some(clone) = builder.try_clone() else {
                return builder.send().await.map_err(Into::into);
            };
            match clone.send().await {
                Ok(resp) => match retryable_status(resp.status()) {
                    Some(kind) if self.policy.should_retry(attempt, kind) => {
                        self.backoff(attempt).await;
                        attempt += 1;
                    }
                    _ => return Ok(resp),
                },
                Err(e) if self.policy.should_retry(attempt, classify(&e)) => {
                    self.backoff(attempt).await;
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Sleep for the policy-computed delay before retry `attempt`.
    async fn backoff(&self, attempt: u32) {
        let delay = self.policy.delay_for(attempt, self.seed(attempt));
        tokio::time::sleep(delay).await;
    }

    /// Jitter entropy in `[0.0, 1.0)`: deterministic when a seed is configured,
    /// else derived from the OS clock so the pure policy stays testable.
    fn seed(&self, attempt: u32) -> f64 {
        match self.jitter_seed {
            Some(seed) => deterministic_seed(seed, attempt),
            None => os_seed(),
        }
    }

    /// Proxy a JSON (or bodyless) request to `endpoint` and collect the reply.
    pub async fn proxy(
        &self,
        method: &str,
        endpoint: &str,
        json_body: Option<Value>,
    ) -> Result<ProxyReply> {
        let verb = Method::from_bytes(method.as_bytes()).context("invalid HTTP method")?;
        let mut req = self.http.request(verb, self.url(endpoint));
        if let Some(body) = &json_body {
            req = req.json(body);
        }
        let resp = self.send(self.auth(req)).await?;
        collect(resp).await
    }

    /// Proxy a multipart upload (text fields + optional named file) to `endpoint`.
    pub async fn proxy_multipart(
        &self,
        endpoint: &str,
        fields: &[Field],
        file: Option<(Vec<u8>, String)>,
        file_part: &str,
    ) -> Result<ProxyReply> {
        let mut form = Form::new();
        for (name, value) in fields {
            form = form.text(name.clone(), value.clone());
        }
        if let Some((bytes, filename)) = file {
            let part = Part::bytes(bytes)
                .file_name(filename)
                .mime_str("application/octet-stream")
                .context("building multipart file part")?;
            form = form.part(file_part.to_owned(), part);
        }
        let resp = self
            .send(self.auth(self.http.post(self.url(endpoint)).multipart(form)))
            .await?;
        collect(resp).await
    }
}

/// Map a transport-level reqwest error to a retry classification (connect /
/// timeout are retryable; everything else is terminal).
fn classify(e: &reqwest::Error) -> ErrorKind {
    if e.is_connect() {
        ErrorKind::Connect
    } else if e.is_timeout() {
        ErrorKind::Timeout
    } else {
        ErrorKind::Other
    }
}

/// Classify an HTTP status for retry: 429 and any 5xx are retryable.
fn retryable_status(status: reqwest::StatusCode) -> Option<ErrorKind> {
    if status.as_u16() == 429 {
        Some(ErrorKind::TooMany429)
    } else if status.is_server_error() {
        Some(ErrorKind::Server5xx)
    } else {
        None
    }
}

/// OS-clock-derived jitter entropy in `[0.0, 1.0)`.
fn os_seed() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos) / 1_000_000_000.0
}

/// Reproducible jitter entropy in `[0.0, 1.0)` from a fixed seed + attempt.
fn deterministic_seed(seed: u64, attempt: u32) -> f64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (seed, attempt).hash(&mut hasher);
    (hasher.finish() % 1_000_000) as f64 / 1_000_000.0
}

async fn collect(resp: reqwest::Response) -> Result<ProxyReply> {
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned)
    };
    let status = resp.status().as_u16();
    let content_type =
        header("content-type").unwrap_or_else(|| "application/octet-stream".to_owned());
    let rtf = header("x-rtf");
    let audio_seconds = header("x-audio-seconds");
    let body = resp.bytes().await?.to_vec();
    Ok(ProxyReply {
        status,
        content_type,
        rtf,
        audio_seconds,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_request<'a>(extra: serde_json::Map<String, Value>) -> SpeakRequest<'a> {
        SpeakRequest {
            input: "hello",
            model: "tts-1",
            voice: None,
            response_format: "mp3",
            speed: 1.0,
            language: "pt-BR",
            instruct: None,
            ref_text: None,
            duration: None,
            extra,
        }
    }

    fn reply(status: u16, content_type: &str, body: &[u8]) -> ProxyReply {
        ProxyReply {
            status,
            content_type: content_type.to_owned(),
            rtf: None,
            audio_seconds: None,
            body: body.to_vec(),
        }
    }

    #[test]
    fn body_always_carries_core_fields() {
        let body = base_request(serde_json::Map::new()).to_body();
        assert_eq!(body["input"], json!("hello"));
        assert_eq!(body["model"], json!("tts-1"));
        assert_eq!(body["response_format"], json!("mp3"));
        assert_eq!(body["language"], json!("pt-BR"));
        assert_eq!(body["speed"], json!(1.0));
    }

    #[test]
    fn body_omits_unset_optionals() {
        let body = base_request(serde_json::Map::new()).to_body();
        let obj = body.as_object().unwrap();
        assert!(!obj.contains_key("voice"));
        assert!(!obj.contains_key("instruct"));
        assert!(!obj.contains_key("ref_text"));
        assert!(!obj.contains_key("duration"));
    }

    #[test]
    fn body_design_mode_carries_instruct() {
        let mut req = base_request(serde_json::Map::new());
        req.instruct = Some("Female, British Accent");
        let body = req.to_body();
        assert_eq!(body["instruct"], json!("Female, British Accent"));
        assert!(!body.as_object().unwrap().contains_key("voice"));
    }

    #[test]
    fn body_clone_mode_carries_voice_and_ref_text() {
        let mut req = base_request(serde_json::Map::new());
        req.voice = Some("narrator");
        req.ref_text = Some("the quick brown fox");
        req.duration = Some(3.5);
        let body = req.to_body();
        assert_eq!(body["voice"], json!("narrator"));
        assert_eq!(body["ref_text"], json!("the quick brown fox"));
        assert_eq!(body["duration"], json!(3.5));
    }

    #[test]
    fn body_passes_through_gen_params() {
        // The extended _byot surface the typed CreateSpeechRequest cannot express.
        let mut extra = serde_json::Map::new();
        extra.insert("num_step".into(), json!(24));
        extra.insert("guidance_scale".into(), json!(3.0));
        let body = base_request(extra).to_body();
        assert_eq!(body["num_step"], json!(24));
        assert_eq!(body["guidance_scale"], json!(3.0));
    }

    #[test]
    fn into_text_extracts_json_text_field() {
        let r = reply(200, "application/json", br#"{"text":"  bonjour  "}"#);
        assert_eq!(r.into_text("json").unwrap(), "bonjour");
    }

    #[test]
    fn into_text_passes_plain_format_through_trimmed() {
        let r = reply(200, "text/plain", b"  plain transcript \n");
        assert_eq!(r.into_text("text").unwrap(), "plain transcript");
    }

    #[test]
    fn into_audio_rejects_empty_body() {
        let r = reply(200, "audio/mpeg", b"");
        assert!(r.into_audio().is_err());
    }

    #[test]
    fn into_audio_returns_bytes_and_headers() {
        let mut r = reply(200, "audio/mpeg", b"\x00\x01\x02");
        r.rtf = Some("0.12".into());
        r.audio_seconds = Some("1.5".into());
        let audio = r.into_audio().unwrap();
        assert_eq!(audio.bytes, vec![0, 1, 2]);
        assert_eq!(audio.content_type, "audio/mpeg");
        assert_eq!(audio.rtf.as_deref(), Some("0.12"));
        assert_eq!(audio.audio_seconds.as_deref(), Some("1.5"));
    }

    #[test]
    fn non_2xx_status_is_an_error() {
        let r = reply(503, "text/plain", b"upstream busy");
        let err = r.into_string().unwrap_err().to_string();
        assert!(err.contains("503"), "{err}");
        assert!(err.contains("upstream busy"), "{err}");
    }

    #[test]
    fn into_json_parses_object() {
        let r = reply(200, "application/json", br#"{"status":"ok"}"#);
        let v = r.into_json().unwrap();
        assert_eq!(v["status"], json!("ok"));
    }

    #[test]
    fn retryable_status_classifies_5xx_and_429() {
        use reqwest::StatusCode;
        assert_eq!(
            retryable_status(StatusCode::from_u16(429).unwrap()),
            Some(ErrorKind::TooMany429)
        );
        assert_eq!(
            retryable_status(StatusCode::from_u16(503).unwrap()),
            Some(ErrorKind::Server5xx)
        );
        assert!(retryable_status(StatusCode::from_u16(404).unwrap()).is_none());
        assert!(retryable_status(StatusCode::from_u16(200).unwrap()).is_none());
    }

    #[test]
    fn seeds_stay_in_unit_interval() {
        for attempt in 0..8u32 {
            let s = deterministic_seed(42, attempt);
            assert!(
                (0.0..1.0).contains(&s),
                "deterministic seed {s} out of range"
            );
        }
        let o = os_seed();
        assert!((0.0..1.0).contains(&o), "os seed {o} out of range");
    }

    #[test]
    fn deterministic_seed_is_reproducible() {
        assert_eq!(deterministic_seed(7, 2), deterministic_seed(7, 2));
    }
}
