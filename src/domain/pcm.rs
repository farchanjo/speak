//! `SampleFormat` and `PcmBuffer` value objects (T010).
//!
//! Pure, IO-free PCM representation. [`PcmBuffer`] holds interleaved 32-bit
//! float samples (the canonical in-memory playback representation) tagged with
//! their sample rate and channel count; [`SampleFormat`] names the on-the-wire
//! integer or float encoding the codec adapters convert to/from. No `ffmpeg`
//! types appear here — the libav adapter (ADR-0001) owns the real conversion;
//! this layer only models the data and derives frame/duration arithmetic.

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
}
