//! Non-macOS backend for the `coreaudio` adapter.
//!
//! Native device I/O and HAL enumeration target CoreAudio / AVFAudio only. On
//! other platforms playback, capture and enumeration return a clear error
//! instead of panicking, so file-oriented commands (`say -o`, `transcribe`,
//! `translate`) still work.

use anyhow::{Result, bail};

use crate::domain::capture_source::CaptureSource;
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

/// Output capture is unsupported off macOS (see module docs).
pub fn capture_output(
    _device: Option<u32>,
    _channel: Option<u16>,
    _secs: f64,
) -> Result<PcmBuffer> {
    bail!(
        "host-output capture (native Core Audio tap) is only implemented on macOS 14.4+; \
         route the output to a virtual-loopback device and use `--source input -d <id>`"
    )
}

/// Device enumeration is unsupported off macOS (see module docs).
pub fn enumerate() -> Result<Vec<AudioDevice>> {
    bail!("audio device enumeration is only implemented on macOS (CoreAudio)")
}

/// TCC responsibility disclaim is macOS-only; a no-op elsewhere (ADR-0016).
pub fn reexec_disclaimed() -> Result<()> {
    Ok(())
}

/// Continuous streaming capture is macOS-only (ADR-0017).
pub(crate) fn start_capture_stream(
    _source: &CaptureSource,
    _params: super::SegmentParams,
) -> Result<tokio::sync::mpsc::Receiver<PcmBuffer>> {
    bail!("continuous capture is only implemented on macOS (CoreAudio)")
}
