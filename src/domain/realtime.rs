//! `RealtimeMode` value object (FR-8): the three realtime pipeline strategies.
//!
//! Pure selector resolved by the `realtime` use case (ADR-0004). `Translate`
//! runs ASR -> MT; `NoTranslate` re-voices the transcript (ASR -> TTS in the
//! chosen output voice); `Echo` plays the raw capture back, then re-voices it.

use crate::domain::errors::DomainError;

/// The realtime pipeline mode (`--translate` / `--no-translate` / `--echo`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeMode {
    /// ASR then machine translation (default).
    #[default]
    Translate,
    /// Passthrough re-voice: ASR then TTS, no translation.
    NoTranslate,
    /// Raw captured audio is played back, then re-voiced via TTS.
    Echo,
}

impl RealtimeMode {
    /// Parse a mode token (`translate|no-translate|echo`, case-insensitive).
    pub fn parse(input: &str) -> Result<Self, DomainError> {
        Ok(match input.trim().to_ascii_lowercase().as_str() {
            "translate" => Self::Translate,
            "no-translate" | "no_translate" | "notranslate" => Self::NoTranslate,
            "echo" => Self::Echo,
            _ => return Err(DomainError::InvalidRealtimeMode(input.to_owned())),
        })
    }

    /// The canonical flag token for this mode.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Translate => "translate",
            Self::NoTranslate => "no-translate",
            Self::Echo => "echo",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_translate() {
        assert_eq!(RealtimeMode::default(), RealtimeMode::Translate);
    }

    #[test]
    fn parses_aliases_case_insensitively() {
        assert_eq!(
            RealtimeMode::parse("Translate").unwrap(),
            RealtimeMode::Translate
        );
        assert_eq!(
            RealtimeMode::parse("no_translate").unwrap(),
            RealtimeMode::NoTranslate
        );
        assert_eq!(
            RealtimeMode::parse("NOTRANSLATE").unwrap(),
            RealtimeMode::NoTranslate
        );
        assert_eq!(RealtimeMode::parse("echo").unwrap(), RealtimeMode::Echo);
    }

    #[test]
    fn round_trips_canonical_token() {
        for mode in [
            RealtimeMode::Translate,
            RealtimeMode::NoTranslate,
            RealtimeMode::Echo,
        ] {
            assert_eq!(RealtimeMode::parse(mode.as_str()).unwrap(), mode);
        }
    }

    #[test]
    fn rejects_unknown_mode() {
        assert!(RealtimeMode::parse("loop").is_err());
    }
}
