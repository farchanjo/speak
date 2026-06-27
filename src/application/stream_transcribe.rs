//! `transcribe --stream` use case (ADR-0014): live transcript-only streaming.
//!
//! The ASR-only sibling of [`RealtimeUseCase`](super::realtime::RealtimeUseCase).
//! It captures one chunk from the shared [`CaptureSource`] step and drives the
//! [`RealtimeStream`] port **transcript-only**: `transcript` frames become text
//! events; `audio` and `translation` frames are ignored **without decoding or
//! playing** (the key difference from the re-voicing realtime pipeline); `done`
//! ends the chunk and `error` is surfaced. It depends on no `Synthesizer`,
//! `Translator`, or `AudioSink` — keeping the surface minimal (ADR-0003).

use anyhow::Result;

use crate::application::capture::capture_gated_wav;
use crate::domain::capture_source::CaptureSource;
use crate::ports::audio::AudioSource;
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

/// The streaming-transcribe use case over the audio + codec port roles.
pub struct StreamTranscribeUseCase<'a, Audio, Codec> {
    audio: &'a Audio,
    codec: &'a Codec,
}

impl<'a, Audio, Codec> StreamTranscribeUseCase<'a, Audio, Codec>
where
    Audio: AudioSource,
    Codec: AudioDecoder + AudioEncoder,
{
    /// Wire the use case to its port roles.
    #[must_use]
    pub fn new(audio: &'a Audio, codec: &'a Codec) -> Self {
        Self { audio, codec }
    }

    /// Capture one chunk encoded as WAV, or `Ok(None)` when gated as silence.
    pub async fn capture(&self, opts: &StreamTranscribeOptions) -> Result<Option<Vec<u8>>> {
        capture_gated_wav(
            self.audio,
            self.codec,
            &opts.source,
            opts.chunk_secs,
            opts.vad,
            opts.silence_floor,
        )
        .await
    }

    /// Drive an SSE stream transcript-only: surface `transcript` text, ignore
    /// `audio`/`translation` frames without playback, end on `done`/`error`.
    pub async fn drive<St, F>(
        &self,
        stream: &mut St,
        mut on_transcript: F,
    ) -> Result<TranscribeStreamEnd>
    where
        St: RealtimeStream,
        F: FnMut(&str),
    {
        while let Some(frame) = stream.recv().await? {
            match frame {
                RealtimeFrame::Transcript { text } => on_transcript(&text),
                RealtimeFrame::Done => return Ok(TranscribeStreamEnd::Done),
                RealtimeFrame::Error { message } => {
                    return Ok(TranscribeStreamEnd::Failed { message });
                }
                // Re-voiced audio and translation frames are not part of a
                // transcript-only stream; ignore them without playback.
                RealtimeFrame::Audio { .. } | RealtimeFrame::Translation { .. } => {}
            }
        }
        Ok(TranscribeStreamEnd::Exhausted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAudio, FakeCodec, FakeStream};

    fn opts() -> StreamTranscribeOptions {
        StreamTranscribeOptions {
            source: CaptureSource::input(None, None),
            chunk_secs: 5.0,
            vad: false,
            silence_floor: 0.1,
        }
    }

    #[tokio::test]
    async fn capture_returns_wav_for_a_live_chunk() {
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let wav = StreamTranscribeUseCase::new(&audio, &codec)
            .capture(&opts())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[tokio::test]
    async fn drive_surfaces_transcripts_and_ignores_audio_and_translation() {
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let uc = StreamTranscribeUseCase::new(&audio, &codec);
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
            RealtimeFrame::Transcript {
                text: "tudo bem".into(),
            },
            RealtimeFrame::Done,
            RealtimeFrame::Transcript {
                text: "unreached".into(),
            },
        ]);

        let mut lines = Vec::new();
        let end = uc
            .drive(&mut stream, |t| lines.push(t.to_owned()))
            .await
            .unwrap();

        assert_eq!(end, TranscribeStreamEnd::Done);
        assert_eq!(lines, vec!["bom dia".to_owned(), "tudo bem".to_owned()]);
        // No AudioSink is wired: audio frames cannot be played by construction,
        // and the translation frame is dropped from a transcript-only stream.
        assert!(audio.plays.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn drive_reports_a_server_error_frame() {
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let uc = StreamTranscribeUseCase::new(&audio, &codec);
        let mut stream = FakeStream::new(vec![
            RealtimeFrame::Error {
                message: "backend down".into(),
            },
            RealtimeFrame::Done,
        ]);

        let mut lines = Vec::new();
        let end = uc
            .drive(&mut stream, |t| lines.push(t.to_owned()))
            .await
            .unwrap();
        assert_eq!(
            end,
            TranscribeStreamEnd::Failed {
                message: "backend down".into()
            }
        );
        assert!(lines.is_empty());
    }

    #[tokio::test]
    async fn drive_completes_on_natural_exhaustion() {
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let uc = StreamTranscribeUseCase::new(&audio, &codec);
        let mut stream = FakeStream::new(vec![RealtimeFrame::Transcript { text: "hi".into() }]);
        let mut lines = Vec::new();
        let end = uc
            .drive(&mut stream, |t| lines.push(t.to_owned()))
            .await
            .unwrap();
        assert_eq!(end, TranscribeStreamEnd::Exhausted);
        assert_eq!(lines, vec!["hi".to_owned()]);
    }
}
