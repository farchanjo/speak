//! Domain error type (T014).
//!
//! The pure failure vocabulary raised by the domain value objects and the
//! [`crate::domain::speech_spec::SpeechSpec`] aggregate. It implements
//! [`std::error::Error`] so callers that use `anyhow` get an automatic
//! conversion, while the domain itself stays free of any `anyhow`/framework
//! dependency (ADR-0003: the domain is pure, zero I/O).

use std::fmt::{self, Display};

/// A domain-level validation failure (no I/O, no framework types).
#[derive(Debug, Clone, PartialEq)]
pub enum DomainError {
    /// Synthesis input text was empty after trimming.
    EmptyInput,
    /// A mandatory aggregate field was not supplied to the builder.
    MissingField(&'static str),
    /// A language tag was empty or carried disallowed characters.
    InvalidLanguage(String),
    /// A voice / clone / standard-voice name was empty.
    InvalidVoiceName(String),
    /// A `response_format` token was not one of the supported formats.
    UnknownAudioFormat(String),
    /// A realtime mode token was not `translate`/`no-translate`/`echo`.
    InvalidRealtimeMode(String),
    /// Speed multiplier was non-finite or non-positive.
    InvalidSpeed(f32),
}

impl Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => f.write_str("synthesis input is empty"),
            Self::MissingField(name) => write!(f, "missing required field '{name}'"),
            Self::InvalidLanguage(v) => write!(f, "invalid language tag '{v}'"),
            Self::InvalidVoiceName(v) => write!(f, "invalid voice name '{v}'"),
            Self::UnknownAudioFormat(v) => {
                write!(
                    f,
                    "unknown audio format '{v}'; expected mp3|opus|aac|flac|wav|pcm"
                )
            }
            Self::InvalidRealtimeMode(v) => {
                write!(
                    f,
                    "invalid realtime mode '{v}'; expected translate|no-translate|echo"
                )
            }
            Self::InvalidSpeed(v) => {
                write!(f, "invalid speed {v}; expected a positive finite value")
            }
        }
    }
}

impl std::error::Error for DomainError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_informative() {
        assert_eq!(
            DomainError::EmptyInput.to_string(),
            "synthesis input is empty"
        );
        assert!(
            DomainError::MissingField("voice")
                .to_string()
                .contains("voice")
        );
        assert!(
            DomainError::UnknownAudioFormat("ogg".into())
                .to_string()
                .contains("ogg")
        );
        assert!(
            DomainError::InvalidRealtimeMode("loop".into())
                .to_string()
                .contains("loop")
        );
    }

    #[test]
    fn is_a_std_error() {
        // Confirms the `?`-into-`anyhow` bridge the domain relies on.
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&DomainError::EmptyInput);
        let any: anyhow::Error = DomainError::InvalidSpeed(0.0).into();
        assert!(any.to_string().contains("speed"));
    }

    #[test]
    fn equality_holds_for_same_variant() {
        assert_eq!(
            DomainError::InvalidLanguage("--".into()),
            DomainError::InvalidLanguage("--".into())
        );
        assert_ne!(DomainError::EmptyInput, DomainError::MissingField("voice"));
    }
}
