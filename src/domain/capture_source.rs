//! `CaptureSource` value object (ADR-0015): which side of the audio device a
//! live/record capture reads from, plus the optional device and channel.
//!
//! A pure **Strategy selector** with zero IO. `Input` captures an audio input
//! device (microphone / line-in, the existing path); `Output` captures the
//! host's playback (system / sound-card output) via the native tap. `device`
//! is the raw `CoreAudio` `AudioDeviceID` (`None` = the default for the
//! direction); `channel` is a single 0-based capture channel (`None` = downmix
//! all channels, ADR-0013). No framework type appears here.

/// Which side of the device the capture reads from (ADR-0015).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureDirection {
    /// An audio input device (microphone / line-in).
    Input,
    /// The host's output (system / sound-card playback), captured via the tap.
    Output,
}

impl CaptureDirection {
    /// The canonical wire/config token (`"input"` / `"output"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }

    /// Parse a config/flag token; unknown values are rejected.
    pub fn parse(token: &str) -> Result<Self, crate::domain::errors::DomainError> {
        match token.trim().to_ascii_lowercase().as_str() {
            "input" | "in" | "mic" => Ok(Self::Input),
            "output" | "out" | "system" => Ok(Self::Output),
            other => Err(crate::domain::errors::DomainError::InvalidCaptureSource(
                other.to_owned(),
            )),
        }
    }
}

/// The capture-source Strategy selector for a live/record capture (ADR-0015).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureSource {
    direction: CaptureDirection,
    device: Option<u32>,
    channel: Option<u16>,
}

impl CaptureSource {
    /// Build a source from its parts.
    #[must_use]
    pub fn new(direction: CaptureDirection, device: Option<u32>, channel: Option<u16>) -> Self {
        Self {
            direction,
            device,
            channel,
        }
    }

    /// An input-device source (microphone / line-in).
    #[must_use]
    pub fn input(device: Option<u32>, channel: Option<u16>) -> Self {
        Self::new(CaptureDirection::Input, device, channel)
    }

    /// An output-capture source (system / sound-card playback).
    #[must_use]
    pub fn output(device: Option<u32>, channel: Option<u16>) -> Self {
        Self::new(CaptureDirection::Output, device, channel)
    }

    /// Which side of the device this source reads from.
    #[must_use]
    pub fn direction(&self) -> CaptureDirection {
        self.direction
    }

    /// The `AudioDeviceID` to capture (`None` = default for the direction).
    #[must_use]
    pub fn device(&self) -> Option<u32> {
        self.device
    }

    /// The single 0-based capture channel (`None` = downmix all, ADR-0013).
    #[must_use]
    pub fn channel(&self) -> Option<u16> {
        self.channel
    }

    /// Whether this source captures the host output (needs the native tap).
    #[must_use]
    pub fn is_output(&self) -> bool {
        matches!(self.direction, CaptureDirection::Output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_is_the_default_direction_shape() {
        let s = CaptureSource::input(Some(3), Some(1));
        assert_eq!(s.direction(), CaptureDirection::Input);
        assert_eq!(s.device(), Some(3));
        assert_eq!(s.channel(), Some(1));
        assert!(!s.is_output());
    }

    #[test]
    fn output_source_flags_the_native_tap() {
        let s = CaptureSource::output(None, None);
        assert!(s.is_output());
        assert_eq!(s.device(), None);
        assert_eq!(s.channel(), None);
        assert_eq!(s.direction().as_str(), "output");
    }

    #[test]
    fn direction_parses_known_tokens_and_rejects_others() {
        assert_eq!(
            CaptureDirection::parse("input").unwrap(),
            CaptureDirection::Input
        );
        assert_eq!(
            CaptureDirection::parse(" OUTPUT ").unwrap(),
            CaptureDirection::Output
        );
        assert_eq!(
            CaptureDirection::parse("mic").unwrap(),
            CaptureDirection::Input
        );
        assert!(CaptureDirection::parse("loopback").is_err());
    }
}
