//! `AudioSink` and `AudioSource` driven ports (T021).
//!
//! Native device I/O abstracted away from `CoreAudio`. `AudioSink` plays a
//! [`PcmBuffer`] on the default device or fans it out to N devices (FR-11), and
//! enumerates output devices; `AudioSource` captures the microphone and
//! enumerates input devices (FR-8/FR-9/FR-10). The coreaudio adapter implements
//! both (ADR-0001 / ADR-0007). No `objc2` type crosses this boundary.

use anyhow::Result;

use crate::domain::pcm::PcmBuffer;

/// A `CoreAudio` device identifier (`AudioDeviceID`), surfaced by `speak devices`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioDeviceId(pub u32);

/// A discovered audio device descriptor (FR-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    /// The hardware device id used by `--output-device` / `[audio.*].device`.
    pub id: AudioDeviceId,
    /// Stable, human-portable device UID (the preferred selector for config).
    pub uid: String,
    /// Human-readable device name.
    pub name: String,
    /// Capture channel count (0 = not an input device).
    pub input_channels: u16,
    /// Playback channel count (0 = not an output device).
    pub output_channels: u16,
    /// Native nominal sample rate in Hz (0 = unknown).
    pub sample_rate: u32,
    /// Whether this is the system default input device.
    pub is_default_input: bool,
    /// Whether this is the system default output device.
    pub is_default_output: bool,
}

impl AudioDevice {
    /// Whether the device can capture audio.
    #[must_use]
    pub fn is_input(&self) -> bool {
        self.input_channels > 0
    }

    /// Whether the device can play audio.
    #[must_use]
    pub fn is_output(&self) -> bool {
        self.output_channels > 0
    }
}

/// Driven port: native audio playback (single device or multi-output fan-out).
#[expect(
    async_fn_in_trait,
    reason = "driven port consumed by use cases directly, not as a trait object (ADR-0003)"
)]
pub trait AudioSink {
    /// Play `pcm` on the default output device at `volume` (`0.0..=1.0`).
    async fn play(&self, pcm: &PcmBuffer, volume: f32) -> Result<()>;

    /// Fan `pcm` out to every device in `devices` simultaneously (FR-11).
    async fn play_to(&self, pcm: &PcmBuffer, devices: &[AudioDeviceId], volume: f32) -> Result<()>;

    /// Enumerate the available output devices.
    fn outputs(&self) -> Result<Vec<AudioDevice>>;
}

/// Driven port: native microphone capture.
#[expect(
    async_fn_in_trait,
    reason = "driven port consumed by use cases directly, not as a trait object (ADR-0003)"
)]
pub trait AudioSource {
    /// Capture `secs` seconds from `device` (or the default input when `None`).
    async fn capture(&self, device: Option<AudioDeviceId>, secs: f64) -> Result<PcmBuffer>;

    /// Enumerate the available input devices.
    fn inputs(&self) -> Result<Vec<AudioDevice>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_direction_predicates() {
        let mic = AudioDevice {
            id: AudioDeviceId(1),
            uid: "BuiltInMic".into(),
            name: "Mic".into(),
            input_channels: 1,
            output_channels: 0,
            sample_rate: 48_000,
            is_default_input: true,
            is_default_output: false,
        };
        let speakers = AudioDevice {
            id: AudioDeviceId(2),
            uid: "BuiltInSpeaker".into(),
            name: "Speakers".into(),
            input_channels: 0,
            output_channels: 2,
            sample_rate: 48_000,
            is_default_input: false,
            is_default_output: true,
        };
        assert!(mic.is_input() && !mic.is_output());
        assert!(speakers.is_output() && !speakers.is_input());
    }
}
