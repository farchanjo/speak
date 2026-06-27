//! Shared chunk-gating step for the streaming pipelines (ADR-0014 / ADR-0017).
//!
//! Capture is decoupled (a continuous native stream owned by the driving
//! adapter, ADR-0017); this module is the **pure** post-capture step: pick a
//! single channel (ADR-0013), resample to the ASR rate/mono, gate silence
//! (VAD), and mux a WAV for the realtime SSE endpoint. Framework-free, so both
//! the realtime re-voicing pipeline and the transcript-only streaming transcribe
//! reuse it.

use anyhow::Result;

use crate::domain::pcm::PcmBuffer;
use crate::ports::codec::{AudioDecoder, AudioEncoder, RecordFormat};

/// Whisper's required ASR sample rate (Hz) — a fixed protocol constant.
pub(crate) const ASR_RATE: u32 = 16_000;
/// Whisper expects mono audio.
pub(crate) const ASR_CHANNELS: u16 = 1;

/// Gate + encode one captured chunk: pick a single channel (ADR-0013), resample
/// to mono 16 kHz, VAD-gate, and mux WAV. Returns the channel-picked capture
/// (for echo playback) alongside the WAV bytes, or `Ok(None)` when gated as
/// silence.
pub(crate) fn gate_chunk<C>(
    codec: &C,
    raw: PcmBuffer,
    channel: Option<u16>,
    vad: bool,
    silence_floor: f64,
) -> Result<Option<(PcmBuffer, Vec<u8>)>>
where
    C: AudioDecoder + AudioEncoder,
{
    let picked = super::pick_input_channel(raw, channel)?;
    let mono = codec.resample(&picked, ASR_RATE, ASR_CHANNELS)?;
    if vad && rms(&mono) < silence_floor {
        return Ok(None);
    }
    let wav = codec.encode(&mono, RecordFormat::Wav)?;
    Ok(Some((picked, wav)))
}

/// Like [`gate_chunk`] but yielding only the WAV bytes (no echo capture).
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
    Ok(gate_chunk(codec, raw, channel, vad, silence_floor)?.map(|(_, wav)| wav))
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
    use crate::application::fakes::FakeCodec;

    #[test]
    fn gate_chunk_yields_picked_capture_and_wav() {
        let codec = FakeCodec;
        let raw = PcmBuffer::new(vec![0.5; 9_600], 48_000, 2);
        let (picked, wav) = gate_chunk(&codec, raw, None, false, 0.1)
            .unwrap()
            .expect("speech passes when the gate is off");
        assert!(!picked.is_empty());
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
}
