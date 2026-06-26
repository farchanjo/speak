//! `AudioFormat` value object (T014): the `response_format` / `--format`
//! vocabulary.
//!
//! Names the container/codec the server encodes synthesized audio into and the
//! client decodes for playback or writes with `-o` (FR-1). Pure: parse + render
//! only — the libav adapter (ADR-0001) performs the actual decode/encode.

use crate::domain::errors::DomainError;

/// A supported audio response format (`mp3|opus|aac|flac|wav|pcm`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// MPEG-1 Audio Layer III (default).
    #[default]
    Mp3,
    /// Opus in an Ogg container.
    Opus,
    /// Advanced Audio Coding.
    Aac,
    /// Free Lossless Audio Codec.
    Flac,
    /// RIFF/WAVE PCM.
    Wav,
    /// Raw little-endian PCM samples.
    Pcm,
}

impl AudioFormat {
    /// Parse the OpenAI `response_format` token (case-insensitive).
    pub fn parse(input: &str) -> Result<Self, DomainError> {
        Ok(match input.trim().to_ascii_lowercase().as_str() {
            "mp3" => Self::Mp3,
            "opus" => Self::Opus,
            "aac" => Self::Aac,
            "flac" => Self::Flac,
            "wav" => Self::Wav,
            "pcm" => Self::Pcm,
            _ => return Err(DomainError::UnknownAudioFormat(input.to_owned())),
        })
    }

    /// The canonical lowercase wire token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Aac => "aac",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Pcm => "pcm",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_mp3() {
        assert_eq!(AudioFormat::default(), AudioFormat::Mp3);
    }

    #[test]
    fn parses_every_token_case_insensitively() {
        for token in ["mp3", "OPUS", "Aac", "flac", "WAV", "pcm"] {
            let fmt = AudioFormat::parse(token).unwrap();
            assert_eq!(fmt.as_str(), token.to_ascii_lowercase());
        }
    }

    #[test]
    fn round_trips_token() {
        assert_eq!(
            AudioFormat::parse(AudioFormat::Flac.as_str()).unwrap(),
            AudioFormat::Flac
        );
    }

    #[test]
    fn rejects_unknown_token() {
        let err = AudioFormat::parse("ogg").unwrap_err();
        assert_eq!(err, DomainError::UnknownAudioFormat("ogg".into()));
    }
}
