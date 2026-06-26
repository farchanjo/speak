//! `Language` value object (T010).
//!
//! A normalized language hint (BCP-47-ish, e.g. `pt-BR`, `en`, `en-US`) used by
//! TTS (`language`), ASR (`--language`), and the realtime `--from`/`--to`
//! selectors. Pure: it trims, validates non-emptiness and an allowed character
//! set, and exposes English detection for the translate Strategy selection
//! (ADR-0004). It does not resolve locales or perform any I/O.

use crate::domain::errors::DomainError;

/// A validated, normalized language tag (e.g. `pt-BR`, `en`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    tag: String,
}

impl Language {
    /// Parse and validate a language tag; rejects empty / non-tag input.
    pub fn parse(input: &str) -> Result<Self, DomainError> {
        let tag = input.trim();
        if tag.is_empty() || !tag.chars().all(is_tag_char) {
            return Err(DomainError::InvalidLanguage(input.to_owned()));
        }
        Ok(Self {
            tag: tag.to_owned(),
        })
    }

    /// The normalized tag text as sent on the wire.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.tag
    }

    /// Whether the tag denotes English (primary subtag `en`, case-insensitive).
    ///
    /// Drives the realtime translate Strategy: an English target stays on the
    /// Whisper translate endpoint, while a non-English target needs chat-MT.
    #[must_use]
    pub fn is_english(&self) -> bool {
        let primary = self.tag.split(['-', '_']).next().unwrap_or(&self.tag);
        primary.eq_ignore_ascii_case("en")
    }
}

/// Whether `c` is allowed in a language tag (ASCII letters, digits, `-`, `_`).
fn is_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_region_tag() {
        let lang = Language::parse("  pt-BR ").unwrap();
        assert_eq!(lang.as_str(), "pt-BR");
        assert!(!lang.is_english());
    }

    #[test]
    fn detects_english_case_insensitively() {
        assert!(Language::parse("en").unwrap().is_english());
        assert!(Language::parse("EN").unwrap().is_english());
        assert!(Language::parse("en-US").unwrap().is_english());
        assert!(Language::parse("en_GB").unwrap().is_english());
    }

    #[test]
    fn rejects_empty_or_blank() {
        assert!(Language::parse("").is_err());
        assert!(Language::parse("   ").is_err());
    }

    #[test]
    fn rejects_embedded_spaces_and_punctuation() {
        assert!(Language::parse("pt BR").is_err());
        assert!(Language::parse("pt.br").is_err());
    }

    #[test]
    fn underscore_and_digits_are_accepted() {
        assert_eq!(Language::parse("es_419").unwrap().as_str(), "es_419");
    }
}
