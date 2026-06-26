//! `AudioDecoder` and `AudioEncoder` driven ports (T021).
//!
//! The codec direction abstracted away from libav. `AudioDecoder` decodes
//! compressed server audio into a [`PcmBuffer`] and resamples between formats
//! (48 kHz stereo for playback, 16 kHz mono for ASR); `AudioEncoder` writes a
//! [`PcmBuffer`] into a record container (WAV/FLAC) for `speak record`
//! (FR-9 / ADR-0001). Both are synchronous CPU transforms; the libav adapter
//! implements them. No `ffmpeg` type crosses this boundary.

use anyhow::Result;

use crate::domain::pcm::PcmBuffer;

/// Container the `record` use case writes captured audio into (FR-9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordFormat {
    /// Hand-muxed RIFF/WAVE PCM (no encoder).
    Wav,
    /// Free Lossless Audio Codec via the libavcodec FLAC encoder.
    Flac,
}

/// Driven port: decode and resample server audio into PCM.
pub trait AudioDecoder {
    /// Decode compressed `bytes` into canonical playback PCM.
    fn decode(&self, bytes: &[u8]) -> Result<PcmBuffer>;

    /// Resample `pcm` to `sample_rate` / `channels` (e.g. 16 kHz mono for ASR).
    fn resample(&self, pcm: &PcmBuffer, sample_rate: u32, channels: u16) -> Result<PcmBuffer>;
}

/// Driven port: encode PCM into a record container.
pub trait AudioEncoder {
    /// Encode `pcm` into `format`, returning the muxed file bytes.
    fn encode(&self, pcm: &PcmBuffer, format: RecordFormat) -> Result<Vec<u8>>;
}
