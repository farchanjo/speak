//! Non-macOS backend for the `coreaudio` adapter.
//!
//! Native device I/O and HAL enumeration target CoreAudio / AVFAudio only. On
//! other platforms playback, capture and enumeration return a clear error
//! instead of panicking, so file-oriented commands (`say -o`, `transcribe`,
//! `translate`) still work.

use anyhow::{Result, bail};

use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::{AudioDevice, AudioDeviceId};

/// Playback is unsupported off macOS (see module docs).
pub fn play(_pcm: &PcmBuffer, _volume: f32) -> Result<()> {
    bail!("native audio playback is only implemented on macOS (CoreAudio); use -o FILE / --no-play")
}

/// Multi-output fan-out is unsupported off macOS (see module docs).
pub fn play_to(_pcm: &PcmBuffer, _devices: &[AudioDeviceId], _volume: f32) -> Result<()> {
    bail!("multi-output fan-out is only implemented on macOS (CoreAudio)")
}

/// Capture is unsupported off macOS (see module docs).
pub fn capture(_device: Option<AudioDeviceId>, _secs: f64) -> Result<PcmBuffer> {
    bail!("native microphone capture is only implemented on macOS (CoreAudio)")
}

/// Device enumeration is unsupported off macOS (see module docs).
pub fn enumerate() -> Result<Vec<AudioDevice>> {
    bail!("audio device enumeration is only implemented on macOS (CoreAudio)")
}
