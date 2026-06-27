//! `chatmt` driven adapter (T039): the arbitrary-target [`Translator`] **Strategy**
//! over a non-OpenAI chat-completions endpoint (ADR-0004).
//!
//! The default [`Translator`] strategy (the `openai` adapter) only emits English
//! via Whisper translate. For a non-English `--to` target this adapter transcribes
//! the chunk with an injected [`Transcriber`], then translates the source text by
//! POSTing to `{translate_url}/v1/chat/completions` with the system prompt
//! `"Translate into <to>. Output only the translation."` and the configured
//! `[http].translate_model` (FR-8). The composition root selects this strategy
//! only when `[http].translate_url` is set; otherwise it degrades to the source
//! transcript. Retry is layered by the port-preserving decorator (T046), so this
//! adapter stays a pure Adapter.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::adapters::retry::HttpStatusError;
use crate::config::Config;
use crate::domain::language::Language;
use crate::ports::transcriber::{TranscribeRequest, Transcriber};
use crate::ports::translator::Translator;

/// Default chat model when `[http].translate_model` is unset.
const DEFAULT_MODEL: &str = "qwen2.5-14b-instruct";
/// Low temperature for faithful, near-deterministic translation.
const TEMPERATURE: f32 = 0.3;
/// Generous-but-bounded completion budget for a translated chunk.
const MAX_TOKENS: u32 = 512;

/// The chat-MT translator: an injected ASR transcriber plus the chat endpoint.
pub struct ChatMtTranslator<T> {
    transcriber: T,
    http: reqwest::Client,
    url: String,
    model: String,
    api_key: Option<String>,
}

impl<T> ChatMtTranslator<T> {
    /// Build the strategy when `[http].translate_url` is configured (Factory).
    ///
    /// Returns `Ok(None)` when no chat-MT endpoint is set so the composition root
    /// can degrade to the source transcript without a strategy.
    pub fn new(transcriber: T, cfg: &Config) -> Result<Option<Self>> {
        let Some(base) = cfg.http.translate_url.clone() else {
            return Ok(None);
        };
        let http = crate::client::build_http_client(&cfg.server)?;
        let model = cfg
            .http
            .translate_model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        Ok(Some(Self {
            transcriber,
            http,
            url: format!("{}/v1/chat/completions", base.trim_end_matches('/')),
            model,
            api_key: cfg.server.api_key.clone(),
        }))
    }

    /// POST the chat-completions translation request and return the reply text.
    async fn chat_translate(&self, text: &str, target: &Language) -> Result<String> {
        let mut req = self
            .http
            .post(&self.url)
            .json(&chat_body(&self.model, text, target));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(HttpStatusError::new(status.as_u16(), body).into());
        }
        let parsed: ChatResponse = resp.json().await.context("parsing chat-MT response")?;
        Ok(parsed.content().unwrap_or_default())
    }
}

impl<T: Transcriber> Translator for ChatMtTranslator<T> {
    async fn translate(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String> {
        let source = self
            .transcriber
            .transcribe(&TranscribeRequest {
                audio,
                filename,
                language: None,
                format: "json",
            })
            .await?;
        if source.trim().is_empty() {
            return Ok(source);
        }
        self.chat_translate(&source, target).await
    }
}

/// Build the OpenAI-compatible chat-completions body for a translation request.
fn chat_body(model: &str, text: &str, target: &Language) -> Value {
    json!({
        "model": model,
        "temperature": TEMPERATURE,
        "max_tokens": MAX_TOKENS,
        "messages": [
            {
                "role": "system",
                "content": format!("Translate into {}. Output only the translation.", target.as_str()),
            },
            { "role": "user", "content": text },
        ],
    })
}

/// The chat-completions response envelope (`{choices:[{message:{content}}]}`).
#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

/// One completion choice.
#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
}

/// The assistant message carrying the translated text.
#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: String,
}

impl ChatResponse {
    /// The first choice's trimmed content, if any.
    fn content(&self) -> Option<String> {
        self.choices
            .first()
            .map(|c| c.message.content.trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Language {
        Language::parse("fr").unwrap()
    }

    #[test]
    fn chat_body_carries_system_prompt_target_and_user_text() {
        let body = chat_body("qwen2.5-14b-instruct", "good morning", &target());
        assert_eq!(body["model"], json!("qwen2.5-14b-instruct"));
        assert_eq!(body["temperature"], json!(TEMPERATURE));
        assert_eq!(body["max_tokens"], json!(MAX_TOKENS));
        assert_eq!(
            body["messages"][0]["content"],
            json!("Translate into fr. Output only the translation.")
        );
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(body["messages"][1]["content"], json!("good morning"));
        assert_eq!(body["messages"][1]["role"], json!("user"));
    }

    #[test]
    fn parses_first_choice_content_trimmed() {
        let resp: ChatResponse =
            serde_json::from_str(r#"{"choices":[{"message":{"content":"  bonjour \n"}}]}"#)
                .unwrap();
        assert_eq!(resp.content().as_deref(), Some("bonjour"));
    }

    #[test]
    fn empty_choices_yields_no_content() {
        let resp: ChatResponse = serde_json::from_str(r#"{"choices":[]}"#).unwrap();
        assert!(resp.content().is_none());
    }
}
