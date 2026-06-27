//! `ServerProbe` over the server's health + capability endpoints (T047 support).
//!
//! Completes the `openai` adapter's port coverage so the `check`/`health` use
//! case (T047) can be wired to a real adapter at the composition root: `health`
//! hits `GET /health`, `models` parses `GET /v1/models`, and `supports_realtime`
//! is the runtime capability probe of `POST /v1/realtime/translate` that selects
//! the SSE path versus the chunked fallback (ADR-0004). All calls ride the same
//! warm pool; the retry decorator wraps it transparently (T046).

use anyhow::{Context, Result};
use serde::Deserialize;

use super::client::OpenAiAdapter;
use crate::ports::probe::ServerProbe;

/// The `GET /v1/models` envelope (`{ "data": [{ "id": ... }] }`).
#[derive(Debug, Deserialize)]
struct ModelsEnvelope {
    #[serde(default)]
    data: Vec<ModelDto>,
}

/// One advertised model entry.
#[derive(Debug, Deserialize)]
struct ModelDto {
    id: String,
}

/// HTTP status the server returns when an endpoint is absent.
const NOT_FOUND: u16 = 404;

/// Parse a `/v1/models` body into the advertised model ids.
fn parse_models(bytes: &[u8]) -> Result<Vec<String>> {
    let envelope: ModelsEnvelope =
        serde_json::from_slice(bytes).context("parsing /v1/models response")?;
    Ok(envelope.data.into_iter().map(|m| m.id).collect())
}

impl ServerProbe for OpenAiAdapter {
    async fn health(&self) -> Result<bool> {
        let resp = self.auth(self.http.get(self.url("/health"))).send().await?;
        Ok(resp.status().is_success())
    }

    async fn models(&self) -> Result<Vec<String>> {
        let resp = self.send_ok(self.http.get(self.url("/v1/models"))).await?;
        let bytes = resp.bytes().await?;
        parse_models(&bytes)
    }

    async fn supports_realtime(&self) -> Result<bool> {
        // A reachable status other than 404 (e.g. 405 method-not-allowed for a
        // GET on a POST-only route) means the realtime endpoint exists.
        let resp = self
            .auth(self.http.get(self.url("/v1/realtime/translate")))
            .send()
            .await?;
        Ok(resp.status().as_u16() != NOT_FOUND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_models_collects_ids() {
        let body = br#"{"object":"list","data":[{"id":"tts-1"},{"id":"whisper-1"}]}"#;
        assert_eq!(parse_models(body).unwrap(), vec!["tts-1", "whisper-1"]);
    }

    #[test]
    fn parse_models_tolerates_missing_data() {
        assert!(parse_models(br#"{"object":"list"}"#).unwrap().is_empty());
    }

    #[test]
    fn parse_models_errors_on_garbage() {
        assert!(parse_models(b"not json").is_err());
    }
}
