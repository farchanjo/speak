//! `SampleFormat` and `PcmBuffer` value objects (T010).
//!
//! Pure, IO-free PCM representation. [`PcmBuffer`] holds interleaved 32-bit
//! float samples (the canonical in-memory playback representation) tagged with
//! their sample rate and channel count; [`SampleFormat`] names the on-the-wire
//! integer or float encoding the codec adapters convert to/from. No codec /
//! audio-framework types appear here — the libav adapter (ADR-0001) owns the
//! real conversion; this layer only models the data and derives frame/duration
//! arithmetic.

/// On-the-wire PCM sample encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Signed 16-bit little-endian integer (ASR upload, WAV `record`).
    S16,
    /// 32-bit little-endian float (canonical playback mix format).
    F32,
}

impl SampleFormat {
    /// Bytes occupied by one sample in this format.
    #[must_use]
    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::S16 => 2,
            Self::F32 => 4,
        }
    }
}

/// Interleaved 32-bit float PCM with its sample rate and channel count.
#[derive(Debug, Clone, PartialEq)]
pub struct PcmBuffer {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
}

impl PcmBuffer {
    /// Wrap interleaved float samples with their rate and channel count.
    #[must_use]
    pub fn new(samples: Vec<f32>, sample_rate: u32, channels: u16) -> Self {
        Self {
            samples,
            sample_rate,
            channels,
        }
    }

    /// Interleaved float samples (`channels` values per frame).
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Sample rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channel count.
    #[must_use]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Number of audio frames (samples per channel; guards a zero channel count).
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// Duration in seconds (guards a zero sample rate).
    #[must_use]
    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate.max(1))
    }

    /// Whether the buffer carries no samples.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Extract a single 0-based `channel` into a mono buffer (same sample rate).
    ///
    /// Returns `None` when the buffer is zero-channel or `channel` is out of
    /// range. Picks one input channel from a multi-channel capture device (e.g. a
    /// mic on input 1 of a 16-in interface) instead of averaging every channel
    /// into the ASR/record mono downmix, which would attenuate the lone live
    /// channel by the channel count (ADR-0013).
    #[must_use]
    pub fn select_channel(&self, channel: u16) -> Option<Self> {
        let channels = usize::from(self.channels);
        let ch = usize::from(channel);
        if channels == 0 || ch >= channels {
            return None;
        }
        let mono = self.samples[ch..]
            .iter()
            .step_by(channels)
            .copied()
            .collect();
        Some(Self::new(mono, self.sample_rate, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_format_widths() {
        assert_eq!(SampleFormat::S16.bytes_per_sample(), 2);
        assert_eq!(SampleFormat::F32.bytes_per_sample(), 4);
    }

    #[test]
    fn frames_and_duration_for_stereo_second() {
        let pcm = PcmBuffer::new(vec![0.0; 96_000], 48_000, 2);
        assert_eq!(pcm.frames(), 48_000);
        assert!((pcm.duration_secs() - 1.0).abs() < f64::EPSILON);
        assert!(!pcm.is_empty());
    }

    #[test]
    fn accessors_round_trip() {
        let pcm = PcmBuffer::new(vec![0.1, -0.1], 16_000, 1);
        assert_eq!(pcm.samples(), &[0.1, -0.1]);
        assert_eq!(pcm.sample_rate(), 16_000);
        assert_eq!(pcm.channels(), 1);
    }

    #[test]
    fn duration_guards_zero_rate_and_channels() {
        let pcm = PcmBuffer::new(vec![0.0; 4], 0, 0);
        assert!(pcm.duration_secs().is_finite());
        assert_eq!(pcm.frames(), 4);
    }

    #[test]
    fn empty_buffer_reports_empty() {
        assert!(PcmBuffer::new(Vec::new(), 48_000, 2).is_empty());
    }

    #[test]
    fn select_channel_extracts_one_interleaved_channel() {
        // 3-channel, 2 frames: [c0,c1,c2, c0,c1,c2].
        let pcm = PcmBuffer::new(vec![0.0, 1.0, 2.0, 0.5, 1.5, 2.5], 48_000, 3);
        let ch1 = pcm.select_channel(1).unwrap();
        assert_eq!(ch1.channels(), 1);
        assert_eq!(ch1.sample_rate(), 48_000);
        assert_eq!(ch1.samples(), &[1.0, 1.5]);
        assert_eq!(pcm.select_channel(0).unwrap().samples(), &[0.0, 0.5]);
    }

    #[test]
    fn select_channel_rejects_out_of_range() {
        let pcm = PcmBuffer::new(vec![0.0, 1.0], 48_000, 2);
        assert!(pcm.select_channel(2).is_none());
        assert!(
            PcmBuffer::new(vec![0.0], 48_000, 0)
                .select_channel(0)
                .is_none()
        );
    }
}
