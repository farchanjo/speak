//! HTTP core for the OpenAI-compatible speech server (reqwest, async).
//!
//! Everything goes through a small generic proxy (`proxy` / `proxy_multipart`)
//! that returns a [`ProxyReply`]. The same proxy is used directly by the CLI
//! and, over a Unix socket, by the [`crate::daemon`] — so a request takes the
//! identical shape whether it runs in-process or through the warm daemon.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
        if matches!(format, "json" | "verbose_json") {
            if let Ok(value) = serde_json::from_str::<Value>(&body) {
                if let Some(text) = value.get("text").and_then(Value::as_str) {
                    return Ok(text.trim().to_owned());
                }
            }
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

impl SpeechClient {
    /// Build a client from resolved configuration.
    pub fn new(cfg: &Config) -> Result<Self> {
        let s = &cfg.server;
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
        Ok(Self {
            http: builder.build().context("building HTTP client")?,
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
