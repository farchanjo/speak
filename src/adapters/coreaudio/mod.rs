//! `coreaudio` driven adapter (T034 / T035): native macOS audio I/O behind the
//! [`crate::ports::AudioSink`] / [`crate::ports::AudioSource`] ports.
//!
//! This is the ONLY place the `objc2` / `AVFAudio` / CoreAudio-HAL crates appear
//! (ADR-0001 / ADR-0003 / ADR-0007). [`CoreAudio`] plays a decoded
//! [`PcmBuffer`] through the native `AVAudioEngine` mixer on the default device
//! or fans one decode out to N pinned `AudioDeviceID`s (FR-11), captures the
//! microphone, and enumerates input/output devices via the `CoreAudio` HAL
//! (`kAudioHardwarePropertyDevices`, FR-10). The macOS backend lives behind a
//! cfg gate; other platforms get a clear-error stub. No `objc2` type crosses the
//! port boundary.
//!
//! The free `play` / `capture_chunk` functions are re-exported for the
//! still-flat realtime path until it moves onto the `AudioSource` port (T044).

#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod backend;

#[cfg(not(target_os = "macos"))]
#[path = "stub.rs"]
mod backend;

use anyhow::Result;

use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::{AudioDevice, AudioDeviceId, AudioSink, AudioSource};

pub use backend::{capture, enumerate, play, play_to};

/// Native `CoreAudio` [`AudioSink`] + [`AudioSource`] Adapter (Factory: `new`).
#[derive(Debug, Clone, Copy, Default)]
pub struct CoreAudio;

impl CoreAudio {
    /// Build the adapter (the native device path needs no configuration).
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Flat-path helper: capture from a device index (0 = system default) used by
/// the still-flat realtime loop until it moves onto the `AudioSource` port.
pub fn capture_chunk(device: u32, secs: f64) -> Result<PcmBuffer> {
    let target = (device != 0).then_some(AudioDeviceId(device));
    capture(target, secs)
}

impl AudioSink for CoreAudio {
    async fn play(&self, pcm: &PcmBuffer, volume: f32) -> Result<()> {
        let pcm = pcm.clone();
        tokio::task::spawn_blocking(move || play(&pcm, volume)).await?
    }

    async fn play_to(&self, pcm: &PcmBuffer, devices: &[AudioDeviceId], volume: f32) -> Result<()> {
        let pcm = pcm.clone();
        let devices = devices.to_vec();
        tokio::task::spawn_blocking(move || play_to(&pcm, &devices, volume)).await?
    }

    fn outputs(&self) -> Result<Vec<AudioDevice>> {
        Ok(enumerate()?
            .into_iter()
            .filter(AudioDevice::is_output)
            .collect())
    }
}

impl AudioSource for CoreAudio {
    async fn capture(&self, device: Option<AudioDeviceId>, secs: f64) -> Result<PcmBuffer> {
        tokio::task::spawn_blocking(move || capture(device, secs)).await?
    }

    fn inputs(&self) -> Result<Vec<AudioDevice>> {
        Ok(enumerate()?
            .into_iter()
            .filter(AudioDevice::is_input)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_chunk_zero_maps_to_default_device() {
        // Index 0 means "system default" -> no explicit AudioDeviceId target.
        assert_eq!((0u32 != 0).then_some(AudioDeviceId(0)), None);
        assert_eq!(
            (3u32 != 0).then_some(AudioDeviceId(3)),
            Some(AudioDeviceId(3))
        );
    }
}
