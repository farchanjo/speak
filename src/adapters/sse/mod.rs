//! `sse` driven adapter (T036): the [`RealtimeStream`] port over the server's
//! `POST /v1/realtime/translate` Server-Sent Events endpoint (ADR-0004).
//!
//! A captured audio chunk is POSTed as multipart (`file` + `to`/`translate`/
//! `voice`/`instruct`/`language`/`format`) over the shared warm `reqwest` pool;
//! the `text/event-stream` response is consumed with `eventsource-stream` and each
//! `event: <TYPE>\ndata: <json>` frame is decoded ([`frame`]) into a typed
//! [`RealtimeFrame`] (`transcript`/`translation`/`audio`/`done`/`error`).
//!
//! Selection is a **runtime** capability probe (the [`ServerProbe`] port), not a
//! compile-time feature: one prebuilt binary picks the SSE path when the endpoint
//! answers and falls back to the chunked ASR -> MT -> TTS loop otherwise. The
//! stream is wrapped by the bounded [`ReconnectingStream`] retry decorator (T046)
//! so a dropped connection re-establishes under the same `[retry]` policy.
//!
//! [`ServerProbe`]: crate::ports::probe::ServerProbe

mod frame;

use std::pin::Pin;

use anyhow::{Context, Result};
use eventsource_stream::{Event, EventStreamError, Eventsource};
use futures_util::StreamExt;
use futures_util::stream::Stream;
use reqwest::multipart::{Form, Part};

use crate::adapters::config::Config;
use crate::adapters::retry::{HttpStatusError, ReconnectingStream, StreamFactory};
use crate::domain::retry::RetryPolicy;
use crate::ports::realtime::{RealtimeFrame, RealtimeStream};

/// A captured chunk projected onto the realtime endpoint's multipart form fields.
///
/// Pure data (no framework type) so the driving adapter can build it from the
/// application's realtime options without touching `reqwest`.
#[derive(Debug, Clone)]
pub struct RealtimeRequest {
    /// The encoded audio chunk (`file`).
    pub audio: Vec<u8>,
    /// Advertised upload file name.
    pub filename: String,
    /// Target language (`to`); `None` leaves the server default.
    pub to: Option<String>,
    /// Whether to translate (`translate`); `false` re-voices the transcript.
    pub translate: bool,
    /// Saved/clone voice name (`voice`).
    pub voice: Option<String>,
    /// Voice-design canonical tags (`instruct`).
    pub instruct: Option<String>,
    /// Source-language hint (`language`).
    pub language: Option<String>,
    /// Output audio format (`format`).
    pub format: String,
}

impl RealtimeRequest {
    /// Build the multipart form for the realtime POST.
    fn to_form(&self) -> Result<Form> {
        let part = Part::bytes(self.audio.clone())
            .file_name(self.filename.clone())
            .mime_str("application/octet-stream")
            .context("building realtime chunk part")?;
        let mut form = Form::new()
            .part("file", part)
            .text("translate", if self.translate { "true" } else { "false" })
            .text("format", self.format.clone());
        if let Some(to) = &self.to {
            form = form.text("to", to.clone());
        }
        if let Some(voice) = &self.voice {
            form = form.text("voice", voice.clone());
        }
        if let Some(instruct) = &self.instruct {
            form = form.text("instruct", instruct.clone());
        }
        if let Some(language) = &self.language {
            form = form.text("language", language.clone());
        }
        Ok(form)
    }
}

/// The `sse` driven adapter: a warm pool bound to the realtime endpoint URL.
///
/// Construction is the **Factory** step. Each realtime turn opens a fresh stream
/// for one captured chunk via [`stream`](SseRealtimeClient::stream), already
/// wrapped in the bounded reconnect decorator.
pub struct SseRealtimeClient {
    http: reqwest::Client,
    url: String,
    api_key: Option<String>,
}

impl SseRealtimeClient {
    /// Build the client from resolved configuration (Factory).
    pub fn new(cfg: &Config) -> Result<Self> {
        let http = crate::adapters::http::build_http_client(&cfg.server)?;
        let base = cfg.server.host.trim_end_matches('/');
        Ok(Self {
            http,
            url: format!("{base}/v1/realtime/translate"),
            api_key: cfg.server.api_key.clone(),
        })
    }

    /// A reconnecting realtime stream for `request`, bounded by `policy` (T046).
    #[must_use]
    pub fn stream(
        &self,
        request: RealtimeRequest,
        policy: RetryPolicy,
        jitter_seed: Option<u64>,
    ) -> ReconnectingStream<SseStreamFactory, RetryPolicy> {
        let factory = SseStreamFactory {
            http: self.http.clone(),
            url: self.url.clone(),
            api_key: self.api_key.clone(),
            request,
        };
        ReconnectingStream::new(factory, policy, jitter_seed)
    }
}

/// Re-establishes the SSE stream on each (re)connect for the retry decorator.
pub struct SseStreamFactory {
    http: reqwest::Client,
    url: String,
    api_key: Option<String>,
    request: RealtimeRequest,
}

impl StreamFactory for SseStreamFactory {
    type Stream = SseRealtimeStream;

    async fn connect(&self) -> Result<Self::Stream> {
        let mut req = self.http.post(&self.url).multipart(self.request.to_form()?);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            // Tag the status so the reconnect classifier can recover 5xx/429 after
            // the error crosses the `anyhow` boundary (matching the openai adapter).
            let body = resp.text().await.unwrap_or_default();
            return Err(HttpStatusError::new(status.as_u16(), body).into());
        }
        let frames: Frames = Box::pin(resp.bytes_stream().eventsource());
        Ok(SseRealtimeStream { frames })
    }
}

/// The boxed event stream behind one realtime connection (`Unpin` for `next`).
type Frames = Pin<Box<dyn Stream<Item = Result<Event, EventStreamError<reqwest::Error>>> + Send>>;

/// A live SSE realtime stream yielding typed [`RealtimeFrame`]s until exhausted.
pub struct SseRealtimeStream {
    frames: Frames,
}

impl RealtimeStream for SseRealtimeStream {
    async fn recv(&mut self) -> Result<Option<RealtimeFrame>> {
        loop {
            match self.frames.next().await {
                None => return Ok(None),
                Some(Ok(event)) => {
                    if let Some(frame) = frame::decode(&event.event, &event.data)? {
                        return Ok(Some(frame));
                    }
                    // Heartbeat / unknown event type: keep pulling.
                }
                Some(Err(err)) => return Err(transport_error(err)),
            }
        }
    }
}

/// Surface a stream error so the reconnect classifier sees the transport drop.
///
/// The `Transport` arm is unwrapped to the inner `reqwest::Error` so
/// [`classify`](crate::adapters::retry::classify) can recognise connect/timeout
/// failures; UTF-8 / parse errors stay non-retryable.
fn transport_error(err: EventStreamError<reqwest::Error>) -> anyhow::Error {
    match err {
        EventStreamError::Transport(inner) => anyhow::Error::new(inner),
        other => anyhow::Error::new(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `SseRealtimeStream` over an in-memory SSE byte stream (no network),
    /// pinning the error type to `reqwest::Error` to match the production alias.
    fn stream_over(raw: &'static str) -> SseRealtimeStream {
        stream_over_chunks(vec![raw.as_bytes()])
    }

    /// Build a `SseRealtimeStream` over several byte chunks so a single frame can
    /// straddle chunk boundaries — mirroring `reqwest::bytes_stream` delivering a
    /// frame in pieces, which the eventsource decoder must buffer and reassemble.
    fn stream_over_chunks(chunks: Vec<&'static [u8]>) -> SseRealtimeStream {
        let items: Vec<_> = chunks
            .into_iter()
            .map(Ok::<&[u8], reqwest::Error>)
            .collect();
        SseRealtimeStream {
            frames: Box::pin(futures_util::stream::iter(items).eventsource()),
        }
    }

    #[tokio::test]
    async fn recv_decodes_frames_and_skips_unknown_event_types() {
        // "T0dHUw==" is base64("OGGS"); the `ping` event must be skipped.
        let raw = concat!(
            "event: transcript\ndata: {\"text\":\"hi\",\"seq\":0,\"lang\":\"fr\"}\n\n",
            "event: ping\ndata: {}\n\n",
            "event: audio\ndata: {\"audio_b64\":\"T0dHUw==\",\"format\":\"mp3\",\"seq\":0}\n\n",
            "event: done\ndata: {}\n\n",
        );
        let mut stream = stream_over(raw);
        assert_eq!(
            stream.recv().await.unwrap(),
            Some(RealtimeFrame::Transcript { text: "hi".into() })
        );
        assert_eq!(
            stream.recv().await.unwrap(),
            Some(RealtimeFrame::Audio {
                data: b"OGGS".to_vec(),
                format: Some("mp3".into()),
                seq: Some(0),
            })
        );
        assert_eq!(stream.recv().await.unwrap(), Some(RealtimeFrame::Done));
        assert_eq!(stream.recv().await.unwrap(), None, "stream is exhausted");
    }

    #[tokio::test]
    async fn recv_reassembles_a_frame_split_across_byte_chunks() {
        // The `transcript` frame's data line arrives in two byte chunks; the
        // eventsource decoder must buffer until the blank-line terminator lands.
        let mut stream = stream_over_chunks(vec![
            b"event: transcript\ndata: {\"te",
            b"xt\":\"split frame\",\"seq\":0}\n\n",
            b"event: done\ndata: {}\n\n",
        ]);
        assert_eq!(
            stream.recv().await.unwrap(),
            Some(RealtimeFrame::Transcript {
                text: "split frame".into(),
            })
        );
        assert_eq!(stream.recv().await.unwrap(), Some(RealtimeFrame::Done));
        assert_eq!(stream.recv().await.unwrap(), None, "stream is exhausted");
    }

    #[tokio::test]
    async fn recv_surfaces_translation_then_error_frames() {
        // A non-English target streams a `translation` frame; a mid-stream
        // `error` frame is surfaced as `RealtimeFrame::Error` (not a transport
        // drop) so the use case can terminate cleanly rather than reconnect.
        let raw = concat!(
            "event: translation\ndata: {\"text\":\"bonjour\",\"to\":\"fr\"}\n\n",
            "event: error\ndata: {\"message\":\"backend down\"}\n\n",
        );
        let mut stream = stream_over(raw);
        assert_eq!(
            stream.recv().await.unwrap(),
            Some(RealtimeFrame::Translation {
                text: "bonjour".into(),
            })
        );
        assert_eq!(
            stream.recv().await.unwrap(),
            Some(RealtimeFrame::Error {
                message: "backend down".into(),
            })
        );
        assert_eq!(stream.recv().await.unwrap(), None, "stream is exhausted");
    }
}
