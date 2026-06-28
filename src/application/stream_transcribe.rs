//! `transcribe --stream` use case (ADR-0014 / ADR-0017): transcript-only drive.
//!
//! The ASR-only sibling of [`RealtimeUseCase`](super::realtime::RealtimeUseCase).
//! Capture is decoupled (a continuous native stream owned by the driving
//! adapter, ADR-0017); this use case keeps the **pure** steps: encode one
//! captured chunk for the SSE endpoint, and drive the [`RealtimeStream`] port
//! **transcript-only** — `transcript` frames become text events; `audio` and
//! `translation` frames are ignored **without decoding or playing** (the key
//! difference from the re-voicing realtime pipeline); `done` ends the chunk and
//! `error` is surfaced.

use anyhow::Result;

use crate::application::capture::encode_chunk;
use crate::application::realtime::FrameKind;
use crate::domain::capture_source::CaptureSource;
use crate::domain::pcm::PcmBuffer;
use crate::ports::codec::{AudioDecoder, AudioEncoder};
use crate::ports::realtime::{RealtimeFrame, RealtimeStream};

/// Per-chunk options for a streaming-transcribe invocation.
#[derive(Debug, Clone)]
pub struct StreamTranscribeOptions {
    /// Where the live audio comes from (input device or host output).
    pub source: CaptureSource,
    /// Capture chunk length in seconds.
    pub chunk_secs: f64,
    /// Whether the silence (VAD) gate is enabled.
    pub vad: bool,
    /// Linear RMS floor below which a chunk is treated as silence.
    pub silence_floor: f64,
}

/// How a transcript-only stream ended (the driving adapter loops or stops).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscribeStreamEnd {
    /// The server sent a terminal `done` frame.
    Done,
    /// The stream was exhausted without a terminal frame.
    Exhausted,
    /// The server reported an error mid-stream.
    Failed {
        /// The error message.
        message: String,
    },
}

/// The streaming-transcribe use case over the codec port role.
pub struct StreamTranscribeUseCase<'a, Codec> {
    codec: &'a Codec,
}

impl<'a, Codec> StreamTranscribeUseCase<'a, Codec>
where
    Codec: AudioDecoder + AudioEncoder,
{
    /// Wire the use case to its codec role.
    #[must_use]
    pub fn new(codec: &'a Codec) -> Self {
        Self { codec }
    }

    /// Encode one captured segment to WAV. The continuous producer is the single
    /// VAD authority now (ADR-0019: it cuts on silence and drops noise), so this
    /// no longer re-gates — a second whole-segment RMS gate would wrongly drop a
    /// soft-but-real utterance ("nem chega nada"). `Ok(None)` only on an empty pick.
    pub fn encode(
        &self,
        raw: PcmBuffer,
        opts: &StreamTranscribeOptions,
    ) -> Result<Option<Vec<u8>>> {
        encode_chunk(self.codec, raw, opts.source.channel(), false, 0.0)
    }

    /// Drive an SSE stream text-only: surface each `transcript`/`translation`
    /// frame to `on_text` with its [`FrameKind`] (the caller prints the kind it
    /// wants — `transcribe --stream` the transcript, `translate --stream` the
    /// translation), ignore re-voiced `audio` frames without playback, and end on
    /// `done`/`error`.
    pub async fn drive<St, F>(&self, stream: &mut St, mut on_text: F) -> Result<TranscribeStreamEnd>
    where
        St: RealtimeStream,
        F: FnMut(FrameKind, &str),
    {
        while let Some(frame) = stream.recv().await? {
            match frame {
                RealtimeFrame::Transcript { text } => on_text(FrameKind::Transcript, &text),
                RealtimeFrame::Translation { text } => on_text(FrameKind::Translation, &text),
                RealtimeFrame::Done => return Ok(TranscribeStreamEnd::Done),
                RealtimeFrame::Error { message } => {
                    return Ok(TranscribeStreamEnd::Failed { message });
                }
                // Re-voiced audio frames are not part of a text stream.
                RealtimeFrame::Audio { .. } => {}
            }
        }
        Ok(TranscribeStreamEnd::Exhausted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeCodec, FakeStream};

    fn opts() -> StreamTranscribeOptions {
        StreamTranscribeOptions {
            source: CaptureSource::input(None, None),
            chunk_secs: 5.0,
            vad: false,
            silence_floor: 0.1,
        }
    }

    #[test]
    fn encode_returns_wav_for_a_captured_chunk() {
        let codec = FakeCodec;
        let raw = PcmBuffer::new(vec![0.5; 9_600], 48_000, 2);
        let wav = StreamTranscribeUseCase::new(&codec)
            .encode(raw, &opts())
            .unwrap()
            .unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[tokio::test]
    async fn drive_surfaces_transcript_and_translation_ignoring_audio() {
        let codec = FakeCodec;
        let uc = StreamTranscribeUseCase::new(&codec);
        let mut stream = FakeStream::new(vec![
            RealtimeFrame::Transcript {
                text: "bom dia".into(),
            },
            RealtimeFrame::Audio {
                data: b"REVOICED".to_vec(),
                format: Some("mp3".into()),
                seq: Some(0),
            },
            RealtimeFrame::Translation {
                text: "good morning".into(),
            },
            RealtimeFrame::Done,
            RealtimeFrame::Transcript {
                text: "unreached".into(),
            },
        ]);

        let mut out = Vec::new();
        let end = uc
            .drive(&mut stream, |kind, t| out.push((kind, t.to_owned())))
            .await
            .unwrap();

        assert_eq!(end, TranscribeStreamEnd::Done);
        assert_eq!(
            out,
            vec![
                (FrameKind::Transcript, "bom dia".to_owned()),
                (FrameKind::Translation, "good morning".to_owned()),
            ]
        );
    }

    #[tokio::test]
    async fn drive_reports_a_server_error_frame() {
        let codec = FakeCodec;
        let uc = StreamTranscribeUseCase::new(&codec);
        let mut stream = FakeStream::new(vec![
            RealtimeFrame::Error {
                message: "backend down".into(),
            },
            RealtimeFrame::Done,
        ]);

        let mut out = Vec::new();
        let end = uc
            .drive(&mut stream, |kind, t| out.push((kind, t.to_owned())))
            .await
            .unwrap();
        assert_eq!(
            end,
            TranscribeStreamEnd::Failed {
                message: "backend down".into()
            }
        );
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn drive_completes_on_natural_exhaustion() {
        let codec = FakeCodec;
        let uc = StreamTranscribeUseCase::new(&codec);
        let mut stream = FakeStream::new(vec![RealtimeFrame::Transcript { text: "hi".into() }]);
        let mut out = Vec::new();
        let end = uc
            .drive(&mut stream, |kind, t| out.push((kind, t.to_owned())))
            .await
            .unwrap();
        assert_eq!(end, TranscribeStreamEnd::Exhausted);
        assert_eq!(out, vec![(FrameKind::Transcript, "hi".to_owned())]);
    }
}
