//! `SpeechSpec` aggregate (T014): the validated, transport-agnostic description
//! of one synthesis request — input text, voice mode, output format, language,
//! speed, and pass-through generation params.
//!
//! Pure: assembled through a fluent **Builder** (GoF Builder, ADR-0003) that
//! enforces the invariants (non-empty input; positive, finite speed; a chosen
//! voice mode and language) and yields an immutable aggregate. No wire/serde
//! request shape lives here — the openai adapter translates it (ADR-0004). The
//! `gen_params` map is the already-validated output of
//! [`crate::domain::gen_params::parse_overrides`].

use serde_json::{Map, Value};

use crate::domain::audio_format::AudioFormat;
use crate::domain::errors::DomainError;
use crate::domain::language::Language;
use crate::domain::voice::VoiceMode;

/// A validated synthesis request aggregate.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechSpec {
    input: String,
    voice: VoiceMode,
    format: AudioFormat,
    language: Language,
    speed: f32,
    gen_params: Map<String, Value>,
}

impl SpeechSpec {
    /// Start building a spec for `input` text.
    #[must_use]
    pub fn builder(input: &str) -> SpeechSpecBuilder {
        SpeechSpecBuilder::new(input)
    }

    /// The text to synthesize.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The resolved voice strategy.
    #[must_use]
    pub fn voice(&self) -> &VoiceMode {
        &self.voice
    }

    /// The output audio format.
    #[must_use]
    pub fn format(&self) -> AudioFormat {
        self.format
    }

    /// The language hint.
    #[must_use]
    pub fn language(&self) -> &Language {
        &self.language
    }

    /// The speed multiplier (validated positive and finite).
    #[must_use]
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// The validated pass-through generation params.
    #[must_use]
    pub fn gen_params(&self) -> &Map<String, Value> {
        &self.gen_params
    }
}

/// Fluent Builder for [`SpeechSpec`] (GoF Builder, ADR-0003).
pub struct SpeechSpecBuilder {
    input: String,
    voice: Option<VoiceMode>,
    format: AudioFormat,
    language: Option<Language>,
    speed: f32,
    gen_params: Map<String, Value>,
}

impl SpeechSpecBuilder {
    /// Seed a builder with the input text and neutral defaults.
    fn new(input: &str) -> Self {
        Self {
            input: input.to_owned(),
            voice: None,
            format: AudioFormat::default(),
            language: None,
            speed: 1.0,
            gen_params: Map::new(),
        }
    }

    /// Set the voice strategy (required).
    #[must_use]
    pub fn voice(mut self, mode: VoiceMode) -> Self {
        self.voice = Some(mode);
        self
    }

    /// Set the output format (defaults to `mp3`).
    #[must_use]
    pub fn format(mut self, format: AudioFormat) -> Self {
        self.format = format;
        self
    }

    /// Set the language hint (required).
    #[must_use]
    pub fn language(mut self, language: Language) -> Self {
        self.language = Some(language);
        self
    }

    /// Set the speed multiplier (defaults to `1.0`).
    #[must_use]
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    /// Set the validated generation-param overrides.
    #[must_use]
    pub fn gen_params(mut self, params: Map<String, Value>) -> Self {
        self.gen_params = params;
        self
    }

    /// Validate the invariants and produce the immutable aggregate.
    pub fn build(self) -> Result<SpeechSpec, DomainError> {
        let input = self.input.trim();
        if input.is_empty() {
            return Err(DomainError::EmptyInput);
        }
        if !self.speed.is_finite() || self.speed <= 0.0 {
            return Err(DomainError::InvalidSpeed(self.speed));
        }
        Ok(SpeechSpec {
            input: input.to_owned(),
            voice: self.voice.ok_or(DomainError::MissingField("voice"))?,
            format: self.format,
            language: self.language.ok_or(DomainError::MissingField("language"))?,
            speed: self.speed,
            gen_params: self.gen_params,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::voice::{StandardVoice, VoiceMode};
    use serde_json::json;

    fn standard() -> VoiceMode {
        VoiceMode::Standard(StandardVoice::new("alloy").unwrap())
    }

    fn lang() -> Language {
        Language::parse("pt-BR").unwrap()
    }

    #[test]
    fn builds_with_defaults() {
        let spec = SpeechSpec::builder("olá")
            .voice(standard())
            .language(lang())
            .build()
            .unwrap();
        assert_eq!(spec.input(), "olá");
        assert_eq!(spec.format(), AudioFormat::Mp3);
        assert!((spec.speed() - 1.0).abs() < f32::EPSILON);
        assert!(spec.gen_params().is_empty());
        assert_eq!(spec.language().as_str(), "pt-BR");
    }

    #[test]
    fn trims_input_and_rejects_empty() {
        let spec = SpeechSpec::builder("  hi  ")
            .voice(standard())
            .language(lang())
            .build()
            .unwrap();
        assert_eq!(spec.input(), "hi");
        let err = SpeechSpec::builder("   ")
            .voice(standard())
            .language(lang())
            .build()
            .unwrap_err();
        assert_eq!(err, DomainError::EmptyInput);
    }

    #[test]
    fn rejects_non_positive_or_nan_speed() {
        let base = || SpeechSpec::builder("hi").voice(standard()).language(lang());
        assert!(matches!(
            base().speed(0.0).build(),
            Err(DomainError::InvalidSpeed(_))
        ));
        assert!(matches!(
            base().speed(-1.0).build(),
            Err(DomainError::InvalidSpeed(_))
        ));
        assert!(matches!(
            base().speed(f32::NAN).build(),
            Err(DomainError::InvalidSpeed(_))
        ));
    }

    #[test]
    fn requires_voice_and_language() {
        assert_eq!(
            SpeechSpec::builder("hi")
                .language(lang())
                .build()
                .unwrap_err(),
            DomainError::MissingField("voice")
        );
        assert_eq!(
            SpeechSpec::builder("hi")
                .voice(standard())
                .build()
                .unwrap_err(),
            DomainError::MissingField("language")
        );
    }

    #[test]
    fn carries_format_speed_and_gen_params() {
        let mut params = Map::new();
        params.insert("num_step".into(), json!(24));
        let spec = SpeechSpec::builder("hi")
            .voice(standard())
            .language(lang())
            .format(AudioFormat::Flac)
            .speed(1.5)
            .gen_params(params)
            .build()
            .unwrap();
        assert_eq!(spec.format(), AudioFormat::Flac);
        assert!((spec.speed() - 1.5).abs() < f32::EPSILON);
        assert_eq!(spec.gen_params().get("num_step"), Some(&json!(24)));
    }
}
