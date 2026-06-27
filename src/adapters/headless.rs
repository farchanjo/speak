//! `headless` audio adapter (T053): a no-op [`AudioSink`]/[`AudioSource`] for the
//! background daemon.
//!
//! The persistent daemon (ADR-0005) routes the network speech ports through the
//! shared application [`SpeakFacade`](crate::application::SpeakFacade) but has no
//! foreground audio device — playback and capture belong to the CLI in the
//! foreground. This adapter satisfies the audio role's port bounds without
//! touching hardware: `say` runs with `play = false` on the daemon (synthesize
//! only), so [`play`](AudioSink::play) is never reached; capture is rejected
//! because `record`/`realtime` are local-only commands and are never forwarded.

use anyhow::{Result, bail};

use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::{AudioDevice, AudioDeviceId, AudioSink, AudioSource};

/// A device-less audio adapter for the daemon: drops playback, rejects capture.
#[derive(Debug, Default, Clone, Copy)]
pub struct HeadlessAudio;

impl HeadlessAudio {
    /// Construct the headless audio adapter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl AudioSink for HeadlessAudio {
    async fn play(&self, _pcm: &PcmBuffer, _volume: f32) -> Result<()> {
        // The daemon never plays: `say` runs synthesize-only (play = false).
        Ok(())
    }

    async fn play_to(
        &self,
        _pcm: &PcmBuffer,
        _devices: &[AudioDeviceId],
        _volume: f32,
    ) -> Result<()> {
        Ok(())
    }

    fn outputs(&self) -> Result<Vec<AudioDevice>> {
        Ok(Vec::new())
    }
}

impl AudioSource for HeadlessAudio {
    async fn capture(&self, _device: Option<AudioDeviceId>, _secs: f64) -> Result<PcmBuffer> {
        bail!("the daemon is headless: record/realtime capture runs in the foreground CLI")
    }

    fn inputs(&self) -> Result<Vec<AudioDevice>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn play_is_a_silent_no_op() {
        let audio = HeadlessAudio::new();
        let pcm = PcmBuffer::new(vec![0.0; 8], 48_000, 1);
        assert!(audio.play(&pcm, 1.0).await.is_ok());
        assert!(audio.play_to(&pcm, &[AudioDeviceId(3)], 0.5).await.is_ok());
    }

    #[tokio::test]
    async fn capture_is_rejected() {
        assert!(HeadlessAudio::new().capture(None, 1.0).await.is_err());
    }

    #[test]
    fn enumerations_are_empty() {
        let audio = HeadlessAudio::new();
        assert!(audio.outputs().unwrap().is_empty());
        assert!(audio.inputs().unwrap().is_empty());
    }
}
