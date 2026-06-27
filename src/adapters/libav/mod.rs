//! `libav` driven adapter (T033 / T038): the codec direction behind the
//! [`crate::ports::AudioDecoder`] / [`crate::ports::AudioEncoder`] ports.
//!
//! This is the ONLY place the `ffmpeg-the-third` (libav FFI) crate appears
//! (ADR-0001 / ADR-0003). [`LibavCodec`] decodes compressed server audio into a
//! pure [`crate::domain::pcm::PcmBuffer`], resamples between formats with
//! libswresample, and encodes captured PCM into a WAV or FLAC record container.
//! All work is in-process through a custom in-memory AVIO read/write callback —
//! no temp files, no child process. No `ffmpeg` type crosses the port boundary.
//!
//! The lower-level free functions (`decode`, `to_asr_mono16`, `wav_mono16`,
//! `rms_s16`, the canonical rate/channel constants and [`DecodeOptions`]) are
//! re-exported for the still-flat realtime path until it moves onto the ports
//! (T044/T055).

pub mod accel;
mod codec;
mod encode;

use anyhow::Result;

use crate::domain::pcm::PcmBuffer;
use crate::ports::codec::{AudioDecoder, AudioEncoder, RecordFormat};

pub use codec::{
    ASR_CHANNELS, ASR_RATE, DecodeOptions, PLAY_CHANNELS, PLAY_RATE, decode, resample_pcm, rms_s16,
    to_asr_mono16, wav_mono16, wav_pcm16,
};
pub use encode::{encode_flac, encode_wav};

/// libav-backed [`AudioDecoder`] + [`AudioEncoder`] Adapter (Factory: `new`).
#[derive(Debug, Clone, Default)]
pub struct LibavCodec {
    options: DecodeOptions,
}

impl LibavCodec {
    /// Build the adapter with the given libav decode tuning (threads/log level).
    #[must_use]
    pub fn new(options: DecodeOptions) -> Self {
        Self { options }
    }
}

impl AudioDecoder for LibavCodec {
    fn decode(&self, bytes: &[u8]) -> Result<PcmBuffer> {
        decode(bytes.to_vec(), &self.options)
    }

    fn resample(&self, pcm: &PcmBuffer, sample_rate: u32, channels: u16) -> Result<PcmBuffer> {
        resample_pcm(pcm, sample_rate, channels)
    }
}

impl AudioEncoder for LibavCodec {
    fn encode(&self, pcm: &PcmBuffer, format: RecordFormat) -> Result<Vec<u8>> {
        match format {
            RecordFormat::Wav => Ok(encode_wav(pcm)),
            RecordFormat::Flac => encode_flac(pcm),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_default_uses_all_cores_error_level() {
        let adapter = LibavCodec::default();
        assert_eq!(adapter.options.threads, 0);
        assert_eq!(adapter.options.log_level, "error");
    }

    #[test]
    fn encoder_wav_round_trips_through_decoder() {
        let pcm = PcmBuffer::new(vec![0.0f32; 16_000], 16_000, 1);
        let bytes = LibavCodec::default()
            .encode(&pcm, RecordFormat::Wav)
            .unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        let back = LibavCodec::default().decode(&bytes).unwrap();
        assert!(back.duration_secs() > 0.9);
    }

    #[test]
    fn encoder_flac_produces_flac_magic() {
        let pcm = PcmBuffer::new(vec![0.0f32; 8_000], 16_000, 1);
        let bytes = LibavCodec::default()
            .encode(&pcm, RecordFormat::Flac)
            .unwrap();
        assert_eq!(&bytes[0..4], b"fLaC");
    }
}
