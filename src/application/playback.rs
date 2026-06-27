//! Shared playback helper for the use cases that emit audio (`say`, `realtime`).
//!
//! Decoding compressed audio into a [`PcmBuffer`] and routing it to one or many
//! output devices is a recurring orchestration; centralising it here keeps the
//! `say` and `realtime` use cases free of duplication. It depends on the
//! [`AudioDecoder`] and [`AudioSink`] ports only — no framework type appears.

use anyhow::Result;

use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::{AudioDeviceId, AudioSink};
use crate::ports::codec::AudioDecoder;

/// Decoded-buffer statistics surfaced for status reporting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackStats {
    /// Number of audio frames decoded (samples per channel).
    pub frames: usize,
    /// Decoded duration in seconds.
    pub secs: f64,
}

/// Decode `bytes` and play the result, returning the decoded statistics.
pub async fn decode_and_play<D, K>(
    decoder: &D,
    sink: &K,
    bytes: &[u8],
    devices: &[AudioDeviceId],
    volume: f32,
) -> Result<PlaybackStats>
where
    D: AudioDecoder,
    K: AudioSink,
{
    let pcm = decoder.decode(bytes)?;
    let stats = PlaybackStats {
        frames: pcm.frames(),
        secs: pcm.duration_secs(),
    };
    play_pcm(sink, &pcm, devices, volume).await?;
    Ok(stats)
}

/// Route `pcm` to the default device, or fan it out to `devices` (FR-11).
pub async fn play_pcm<K>(
    sink: &K,
    pcm: &PcmBuffer,
    devices: &[AudioDeviceId],
    volume: f32,
) -> Result<()>
where
    K: AudioSink,
{
    if devices.is_empty() {
        sink.play(pcm, volume).await
    } else {
        sink.play_to(pcm, devices, volume).await
    }
}
