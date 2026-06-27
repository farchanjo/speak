//! Decode realtime SSE events into the typed [`RealtimeFrame`] (T036).
//!
//! Pure, network-free translation of an `(event-type, data-json)` pair — the two
//! halves of an `event: <TYPE>\ndata: <json>` SSE frame — into the domain-facing
//! [`RealtimeFrame`] the `RealtimeStream` port yields (ADR-0004). Keeping the
//! parsing here lets it be unit-tested without a server.

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;

use crate::ports::realtime::RealtimeFrame;

/// `transcript` / `translation` payload (`{text, seq?, lang?/to?}`); only `text`
/// is consumed (the sequence/language metadata is informational).
#[derive(Debug, Deserialize)]
struct TextData {
    #[serde(default)]
    text: String,
}

/// `audio` payload (`{audio_b64, format?, seq?}`).
#[derive(Debug, Deserialize)]
struct AudioData {
    audio_b64: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    seq: Option<u64>,
}

/// `error` payload (`{message}`).
#[derive(Debug, Deserialize)]
struct ErrorData {
    #[serde(default)]
    message: String,
}

/// Decode one SSE `(event_type, data)` pair into a [`RealtimeFrame`].
///
/// Unknown event types (heartbeats, comments, the default `message` type) yield
/// `Ok(None)` so the consumer skips them; `audio` frames base64-decode the
/// `audio_b64` field into raw bytes.
pub fn decode(event_type: &str, data: &str) -> Result<Option<RealtimeFrame>> {
    match event_type {
        "transcript" => Ok(Some(RealtimeFrame::Transcript {
            text: text_of(data)?,
        })),
        "translation" => Ok(Some(RealtimeFrame::Translation {
            text: text_of(data)?,
        })),
        "audio" => Ok(Some(decode_audio(data)?)),
        "done" => Ok(Some(RealtimeFrame::Done)),
        "error" => {
            let payload: ErrorData =
                serde_json::from_str(data).context("parsing SSE error frame")?;
            Ok(Some(RealtimeFrame::Error {
                message: payload.message,
            }))
        }
        _ => Ok(None),
    }
}

/// Extract the `text` field of a transcript/translation payload.
fn text_of(data: &str) -> Result<String> {
    let payload: TextData = serde_json::from_str(data).context("parsing SSE text frame")?;
    Ok(payload.text)
}

/// Base64-decode an `audio` payload into an [`RealtimeFrame::Audio`].
fn decode_audio(data: &str) -> Result<RealtimeFrame> {
    let payload: AudioData = serde_json::from_str(data).context("parsing SSE audio frame")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.audio_b64.as_bytes())
        .context("decoding base64 audio chunk")?;
    Ok(RealtimeFrame::Audio {
        data: bytes,
        format: payload.format,
        seq: payload.seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn decodes_transcript_and_translation_text() {
        assert_eq!(
            decode("transcript", r#"{"text":"bonjour","seq":0,"lang":"fr"}"#).unwrap(),
            Some(RealtimeFrame::Transcript {
                text: "bonjour".into()
            })
        );
        assert_eq!(
            decode("translation", r#"{"text":"hello","seq":0,"to":"en"}"#).unwrap(),
            Some(RealtimeFrame::Translation {
                text: "hello".into()
            })
        );
    }

    #[test]
    fn decodes_audio_from_base64() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"OGGS");
        let data = format!(r#"{{"audio_b64":"{b64}","format":"opus","seq":3}}"#);
        assert_eq!(
            decode("audio", &data).unwrap(),
            Some(RealtimeFrame::Audio {
                data: b"OGGS".to_vec(),
                format: Some("opus".into()),
                seq: Some(3),
            })
        );
    }

    #[test]
    fn decodes_done_and_error() {
        assert_eq!(decode("done", "{}").unwrap(), Some(RealtimeFrame::Done));
        assert_eq!(
            decode("error", r#"{"message":"boom"}"#).unwrap(),
            Some(RealtimeFrame::Error {
                message: "boom".into()
            })
        );
    }

    #[test]
    fn unknown_event_types_are_skipped() {
        assert_eq!(decode("message", "{}").unwrap(), None);
        assert_eq!(decode("ping", "").unwrap(), None);
    }

    #[test]
    fn rejects_malformed_audio_base64() {
        let err = decode("audio", r#"{"audio_b64":"!!!not-base64!!!"}"#).unwrap_err();
        assert!(err.to_string().contains("base64"), "got: {err}");
    }
}
