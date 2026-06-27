//! Shared live-capture step for the streaming pipelines (ADR-0014 / ADR-0015).
//!
//! Capture one chunk from a [`CaptureSource`] Strategy, pick a single channel
//! (ADR-0013), resample to the ASR rate/mono, gate silence (VAD), and mux a WAV
//! for the realtime SSE endpoint. Kept framework-free so both the realtime
//! re-voicing pipeline and the transcript-only streaming transcribe reuse it.

use anyhow::Result;

use crate::domain::capture_source::CaptureSource;
use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::AudioSource;
use crate::ports::codec::{AudioDecoder, AudioEncoder, RecordFormat};

/// Whisper's required ASR sample rate (Hz) — a fixed protocol constant.
pub(crate) const ASR_RATE: u32 = 16_000;
/// Whisper expects mono audio.
pub(crate) const ASR_CHANNELS: u16 = 1;

/// Capture one chunk from `source`, gate silence, and encode it to WAV.
///
/// Returns `Ok(None)` when the VAD gate (`vad` + `silence_floor`) treats the
/// chunk as silence; otherwise the channel-picked raw capture (for echo
/// playback) alongside the WAV bytes ready to POST. The captured buffer is
/// reduced to one channel before the mono downmix when `source.channel()` is
/// set (ADR-0013).
pub(crate) async fn capture_gated<A, C>(
    audio: &A,
    codec: &C,
    source: &CaptureSource,
    chunk_secs: f64,
    vad: bool,
    silence_floor: f64,
) -> Result<Option<(PcmBuffer, Vec<u8>)>>
where
    A: AudioSource,
    C: AudioDecoder + AudioEncoder,
{
    let captured = audio.capture_for(source, chunk_secs).await?;
    let captured = super::pick_input_channel(captured, source.channel())?;
    let mono = codec.resample(&captured, ASR_RATE, ASR_CHANNELS)?;
    if vad && rms(&mono) < silence_floor {
        return Ok(None);
    }
    let wav = codec.encode(&mono, RecordFormat::Wav)?;
    Ok(Some((captured, wav)))
}

/// Encode one already-captured chunk for the SSE endpoint (ADR-0017): pick a
/// single channel (ADR-0013), resample to mono 16 kHz, VAD-gate, and mux WAV.
/// `Ok(None)` when the chunk is gated as silence. Used by the continuous
/// streaming pipeline, where capture is decoupled from this encode step.
pub(crate) fn encode_chunk<C>(
    codec: &C,
    raw: PcmBuffer,
    channel: Option<u16>,
    vad: bool,
    silence_floor: f64,
) -> Result<Option<Vec<u8>>>
where
    C: AudioDecoder + AudioEncoder,
{
    let picked = super::pick_input_channel(raw, channel)?;
    let mono = codec.resample(&picked, ASR_RATE, ASR_CHANNELS)?;
    if vad && rms(&mono) < silence_floor {
        return Ok(None);
    }
    Ok(Some(codec.encode(&mono, RecordFormat::Wav)?))
}

/// Linear RMS amplitude of an interleaved float buffer (silence-gate input).
pub(crate) fn rms(pcm: &PcmBuffer) -> f64 {
    let samples = pcm.samples();
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    (sum / samples.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAudio, FakeCodec};

    #[tokio::test]
    async fn capture_gated_input_yields_raw_and_wav() {
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let src = CaptureSource::input(None, None);
        let (_, wav) = capture_gated(&audio, &codec, &src, 1.0, false, 0.1)
            .await
            .unwrap()
            .expect("speech passes when the gate is off");
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[test]
    fn encode_chunk_encodes_a_captured_buffer() {
        let codec = FakeCodec;
        let raw = PcmBuffer::new(vec![0.5; 9_600], 48_000, 2);
        let wav = encode_chunk(&codec, raw, None, false, 0.1)
            .unwrap()
            .expect("speech passes when the gate is off");
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[test]
    fn encode_chunk_gates_silence() {
        let codec = FakeCodec;
        let raw = PcmBuffer::new(vec![0.0; 9_600], 48_000, 2);
        let gated = encode_chunk(&codec, raw, None, true, 0.1).unwrap();
        assert!(gated.is_none(), "silence is gated");
    }

    #[tokio::test]
    async fn output_source_without_native_tap_errors_with_fallback_hint() {
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let src = CaptureSource::output(None, None);
        let err = capture_gated(&audio, &codec, &src, 1.0, false, 0.1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("BlackHole"), "{err}");
    }
}
