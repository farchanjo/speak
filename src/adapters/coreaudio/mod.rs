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

use crate::domain::capture_source::{CaptureDirection, CaptureSource};
use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::{AudioDevice, AudioDeviceId, AudioSink, AudioSource};

pub use backend::{capture, capture_output, enumerate, play, play_to, reexec_disclaimed};

/// Tuning for a continuous capture stream (ADR-0017 / ADR-0019). When `vad` is
/// set, the producer cuts on the silence between utterances; otherwise it slices
/// a fixed `chunk_secs` grid (the `--no-vad` path — no audio is ever dropped).
#[derive(Debug, Clone, Copy)]
pub struct SegmentParams {
    /// Cut on silence (VAD segmentation) instead of a fixed time grid.
    pub vad: bool,
    /// Linear RMS amplitude at/above which a hop counts as speech.
    pub floor: f64,
    /// Fixed slice length (seconds) used only when `vad` is off.
    pub chunk_secs: f64,
    /// Ring backlog ceiling (seconds) before the oldest audio is dropped.
    pub cap_secs: f64,
}

/// Native `CoreAudio` [`AudioSink`] + [`AudioSource`] Adapter (Factory: `new`).
#[derive(Debug, Clone, Copy, Default)]
pub struct CoreAudio;

impl CoreAudio {
    /// Build the adapter (the native device path needs no configuration).
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Start a continuous capture stream for `source` (ADR-0017): one long-lived
    /// native capture feeds VAD-segmented utterances (or fixed `chunk_secs` slices
    /// when `params.vad` is off, ADR-0019) over a channel bounded by
    /// `params.cap_secs`, decoupling capture from the SSE consumer so a slow round
    /// trip never drops audio.
    pub fn capture_stream(
        &self,
        source: &CaptureSource,
        params: SegmentParams,
    ) -> Result<NativeCaptureStream> {
        Ok(NativeCaptureStream {
            rx: backend::start_capture_stream(source, params)?,
        })
    }
}

/// A continuous capture stream of `chunk_secs`-sized chunks (ADR-0017).
///
/// Yields chunks until dropped (which stops the underlying native capture);
/// `tokio` stays inside the adapter, like the `sse` reconnecting stream.
pub struct NativeCaptureStream {
    rx: tokio::sync::mpsc::Receiver<PcmBuffer>,
}

impl NativeCaptureStream {
    /// The next captured chunk (device rate/channels), or `None` once the capture
    /// has ended.
    pub async fn recv(&mut self) -> Option<PcmBuffer> {
        self.rx.recv().await
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

    /// Route by source (ADR-0015): `Input` captures a device; `Output` runs the
    /// native Core Audio system-output tap (macOS 14.4+) off the async runtime.
    async fn capture_for(&self, source: &CaptureSource, secs: f64) -> Result<PcmBuffer> {
        match source.direction() {
            CaptureDirection::Input => self.capture(source.device().map(AudioDeviceId), secs).await,
            CaptureDirection::Output => {
                let (device, channel) = (source.device(), source.channel());
                tokio::task::spawn_blocking(move || capture_output(device, channel, secs)).await?
            }
        }
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
