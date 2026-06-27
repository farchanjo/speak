//! `openai` driven adapter (T030-T032): one warm `async-openai` client speaking
//! the OpenAI-compatible speech server (ADR-0004).
//!
//! The adapter implements four driven ports over a single, shared keep-alive
//! pool ([`OpenAiAdapter`]):
//!
//! - [`crate::ports::Transcriber`] / [`crate::ports::Translator`] use the typed
//!   `async-openai` `audio().transcription()` / `audio().translation()` request
//!   groups (`create_raw`, so every `response_format` round-trips as bytes).
//! - [`crate::ports::Synthesizer`] sends the server's EXTENDED `/v1/audio/speech`
//!   request (voice-design `instruct`, `voice=clone`, `ref_text`, `language`,
//!   generation params) plus the native `/tts` endpoint. `async-openai` 0.41
//!   exposes no non-streaming speech "bring-your-own-types" method and discards
//!   the `X-RTF` / `X-Audio-Seconds` headers FR-1 needs, so the Synthesizer
//!   serializes a `speak`-owned body (built by a fluent **Builder**) and posts it
//!   over the adapter's tuned warm `reqwest` pool.
//! - [`crate::ports::VoiceRepository`] drives the server's non-OpenAI
//!   `POST/GET/DELETE /voices` surface over that shared client.
//!
//! Retry is NOT baked in here: a port-preserving decorator (T046) wraps each
//! port at the composition root (T054). This adapter is a pure Adapter.

mod client;
mod speech;
mod transcription;
mod translation;
mod voices;

pub use client::OpenAiAdapter;
pub use speech::SpeechBodyBuilder;
