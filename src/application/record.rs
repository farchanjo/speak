//! `record` use case (T043): capture the microphone to a WAV/FLAC file (FR-9).
//!
//! Orchestrates `AudioSource` (capture) -> `AudioDecoder` (resample to the
//! requested rate/channels when they differ from the device's) -> `AudioEncoder`
//! (mux WAV or FLAC). The muxed bytes are returned so the driving adapter writes
//! the `--output` file; no `objc2`/`ffmpeg` type crosses the boundary.

use anyhow::Result;

use crate::domain::capture_source::CaptureSource;
use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::AudioSource;
use crate::ports::codec::{AudioDecoder, AudioEncoder, RecordFormat};

/// Options for a `record` invocation.
#[derive(Debug, Clone)]
pub struct RecordOptions {
    /// Where the audio comes from: an input device or the host output
    /// (ADR-0015); carries the device and the single capture channel (ADR-0013).
    pub source: CaptureSource,
    /// Capture duration in seconds.
    pub secs: f64,
    /// Output container.
    pub format: RecordFormat,
    /// Target sample rate (`None` keeps the captured rate).
    pub sample_rate: Option<u32>,
    /// Target channel count (`None` keeps the captured channels).
    pub channels: Option<u16>,
}

/// The result of a `record` invocation.
#[derive(Debug, Clone)]
pub struct RecordOutcome {
    /// The muxed WAV/FLAC file bytes.
    pub bytes: Vec<u8>,
    /// Frames written.
    pub frames: usize,
    /// Recorded duration in seconds.
    pub secs: f64,
    /// The container that was written.
    pub format: RecordFormat,
}

/// The `record` use case over the source, codec, and encoder ports.
pub struct RecordUseCase<'a, Src, Dec, Enc> {
    source: &'a Src,
    decoder: &'a Dec,
    encoder: &'a Enc,
}

impl<'a, Src, Dec, Enc> RecordUseCase<'a, Src, Dec, Enc>
where
    Src: AudioSource,
    Dec: AudioDecoder,
    Enc: AudioEncoder,
{
    /// Wire the use case to its ports.
    #[must_use]
    pub fn new(source: &'a Src, decoder: &'a Dec, encoder: &'a Enc) -> Self {
        Self {
            source,
            decoder,
            encoder,
        }
    }

    /// Capture, conform, and encode according to `opts`.
    pub async fn execute(&self, opts: &RecordOptions) -> Result<RecordOutcome> {
        let captured = self.source.capture_for(&opts.source, opts.secs).await?;
        let captured = super::pick_input_channel(captured, opts.source.channel())?;
        let pcm = self.conform(captured, opts)?;
        let bytes = self.encoder.encode(&pcm, opts.format)?;
        Ok(RecordOutcome {
            bytes,
            frames: pcm.frames(),
            secs: pcm.duration_secs(),
            format: opts.format,
        })
    }

    /// Resample to the requested rate/channels only when they differ.
    fn conform(&self, pcm: PcmBuffer, opts: &RecordOptions) -> Result<PcmBuffer> {
        let rate = opts.sample_rate.unwrap_or_else(|| pcm.sample_rate());
        let channels = opts.channels.unwrap_or_else(|| pcm.channels());
        if rate == pcm.sample_rate() && channels == pcm.channels() {
            return Ok(pcm);
        }
        self.decoder.resample(&pcm, rate, channels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAudio, FakeCodec};

    fn opts(format: RecordFormat) -> RecordOptions {
        RecordOptions {
            source: CaptureSource::input(None, None),
            secs: 1.0,
            format,
            sample_rate: None,
            channels: None,
        }
    }

    #[tokio::test]
    async fn records_wav_at_capture_rate_without_resampling() {
        // FakeAudio captures 48 kHz stereo, 1 s; no target => no resample.
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let outcome = RecordUseCase::new(&audio, &codec, &codec)
            .execute(&opts(RecordFormat::Wav))
            .await
            .unwrap();
        assert_eq!(&outcome.bytes[0..4], b"RIFF");
        assert_eq!(outcome.frames, 48_000);
        assert!((outcome.secs - 1.0).abs() < 1e-6);
        assert_eq!(outcome.format, RecordFormat::Wav);
    }

    #[tokio::test]
    async fn records_selecting_one_input_channel() {
        // FakeAudio captures 48 kHz stereo; picking channel 0 yields a mono
        // buffer with the frame count preserved (ADR-0013), no resample needed.
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let req = RecordOptions {
            source: CaptureSource::input(None, Some(0)),
            ..opts(RecordFormat::Wav)
        };
        let outcome = RecordUseCase::new(&audio, &codec, &codec)
            .execute(&req)
            .await
            .unwrap();
        assert_eq!(&outcome.bytes[0..4], b"RIFF");
        assert_eq!(outcome.frames, 48_000);
    }

    #[tokio::test]
    async fn record_rejects_out_of_range_input_channel() {
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let req = RecordOptions {
            source: CaptureSource::input(None, Some(9)),
            ..opts(RecordFormat::Wav)
        };
        let err = RecordUseCase::new(&audio, &codec, &codec)
            .execute(&req)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("out of range"), "{err}");
    }

    #[tokio::test]
    async fn records_flac_resampling_to_target_rate_and_channels() {
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let req = RecordOptions {
            sample_rate: Some(16_000),
            channels: Some(1),
            ..opts(RecordFormat::Flac)
        };
        let outcome = RecordUseCase::new(&audio, &codec, &codec)
            .execute(&req)
            .await
            .unwrap();
        assert_eq!(&outcome.bytes[0..4], b"fLaC");
        // The resample path ran (fake keeps the frame count at the new rate).
        assert!(outcome.frames > 0);
    }
}
