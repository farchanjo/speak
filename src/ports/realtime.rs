//! `RealtimeStream` driven port (T022).
//!
//! Consumes the realtime translate stream as a sequence of typed frames. The
//! sse adapter implements it over `POST /v1/realtime/translate` via
//! `eventsource-stream`, decoding `audio_b64` into bytes; the retry decorator
//! reconnects a dropped stream under the same bounded policy (ADR-0004). The
//! `RealtimeMode` Strategy that drives the loop lives in the domain
//! ([`crate::domain::realtime::RealtimeMode`]).

use anyhow::Result;

/// A decoded realtime frame (`{type, text?, audio_b64?, format?, seq?}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeFrame {
    /// Source-language transcript text.
    Transcript {
        /// The recognized text.
        text: String,
    },
    /// Target-language translation text.
    Translation {
        /// The translated text.
        text: String,
    },
    /// A chunk of synthesized audio.
    Audio {
        /// Decoded audio bytes (from `audio_b64`).
        data: Vec<u8>,
        /// Codec/format hint for the chunk.
        format: Option<String>,
        /// Monotonic sequence number when the server supplies one.
        seq: Option<u64>,
    },
    /// Terminal frame: the stream completed normally.
    Done,
    /// The server reported an error mid-stream.
    Error {
        /// The error message.
        message: String,
    },
}

/// Driven port: yield realtime frames until the stream is exhausted.
#[expect(
    async_fn_in_trait,
    reason = "driven port consumed by generic retry decorators, not as a trait object (ADR-0004)"
)]
pub trait RealtimeStream {
    /// Receive the next frame, or `None` once the stream ends.
    async fn recv(&mut self) -> Result<Option<RealtimeFrame>>;
}
