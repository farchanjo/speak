//! Non-macOS audio stub.
//!
//! Native device I/O currently targets CoreAudio / AVFAudio only. On other
//! platforms playback and capture return a clear error instead of panicking,
//! so file-oriented commands (`say -o`, `transcribe`, `translate`) still work.

use anyhow::{Result, bail};

use crate::domain::pcm::PcmBuffer;

/// Playback is unsupported off macOS (see module docs).
pub fn play(_pcm: &PcmBuffer, _volume: f32) -> Result<()> {
    bail!("native audio playback is only implemented on macOS (CoreAudio); use -o FILE / --no-play")
}

/// Capture is unsupported off macOS (see module docs).
pub fn capture_chunk(_device: u32, _secs: f64) -> Result<PcmBuffer> {
    bail!("native microphone capture is only implemented on macOS (CoreAudio)")
}
