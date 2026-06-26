//! Voice-design value object (T011).
//!
//! A voice design is a comma-separated list of *canonical* tags accepted by the
//! server's `instruct` field. Free text is rejected — only the 23 EN tags below
//! compose a valid design. Matching is case-insensitive; the original (trimmed)
//! tag text is preserved on the wire so known-good title-cased tags such as
//! `"British Accent"` are sent verbatim.

use anyhow::{bail, Result};

/// The canonical voice-design tags accepted by the server `instruct` field.
pub const CANONICAL_TAGS: &[&str] = &[
    "male",
    "female",
    "child",
    "teenager",
    "young adult",
    "middle-aged",
    "elderly",
    "very low pitch",
    "low pitch",
    "moderate pitch",
    "high pitch",
    "very high pitch",
    "whisper",
    "american accent",
    "australian accent",
    "british accent",
    "canadian accent",
    "chinese accent",
    "indian accent",
    "japanese accent",
    "korean accent",
    "portuguese accent",
    "russian accent",
];

/// A validated comma-separated list of canonical voice-design tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceDesign {
    tags: Vec<String>,
}

impl VoiceDesign {
    /// Parse and validate a comma-separated tag list; rejects unknown / free text.
    pub fn parse(input: &str) -> Result<Self> {
        let mut tags = Vec::new();
        for raw in input.split(',') {
            let tag = raw.trim();
            if tag.is_empty() {
                continue;
            }
            if !is_canonical(tag) {
                bail!(
                    "invalid voice-design tag '{tag}'; instruct accepts only canonical tags \
                     (see `say --list-designs`)"
                );
            }
            tags.push(tag.to_owned());
        }
        if tags.is_empty() {
            bail!("voice design is empty; pass one or more canonical tags");
        }
        Ok(Self { tags })
    }

    /// The validated `instruct` string (original casing, comma-separated).
    #[must_use]
    pub fn instruct(&self) -> String {
        self.tags.join(", ")
    }
}

/// Whether `tag` (case-insensitive) is a canonical voice-design tag.
#[must_use]
pub fn is_canonical(tag: &str) -> bool {
    let lower = tag.to_ascii_lowercase();
    CANONICAL_TAGS.contains(&lower.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_title_case() {
        let design = VoiceDesign::parse("Female, Young Adult, British Accent").unwrap();
        assert_eq!(design.instruct(), "Female, Young Adult, British Accent");
    }

    #[test]
    fn rejects_free_text() {
        assert!(VoiceDesign::parse("sounds happy and warm").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(VoiceDesign::parse("  ,  ").is_err());
    }

    #[test]
    fn canonical_set_has_23_tags() {
        assert_eq!(CANONICAL_TAGS.len(), 23);
    }

    #[test]
    fn is_canonical_is_case_insensitive() {
        assert!(is_canonical("BRITISH ACCENT"));
        assert!(is_canonical("british accent"));
        assert!(is_canonical("British Accent"));
        assert!(!is_canonical("texan accent"));
    }

    #[test]
    fn trims_whitespace_and_drops_blank_segments() {
        let design = VoiceDesign::parse("  female ,, , british accent  ").unwrap();
        assert_eq!(design.instruct(), "female, british accent");
    }

    #[test]
    fn rejects_when_any_tag_is_free_text() {
        // One bad tag in an otherwise-valid list fails the whole design.
        let err = VoiceDesign::parse("female, sounds friendly").unwrap_err();
        assert!(err.to_string().contains("sounds friendly"), "{err}");
    }

    #[test]
    fn single_tag_is_accepted() {
        let design = VoiceDesign::parse("Whisper").unwrap();
        assert_eq!(design.instruct(), "Whisper");
    }
}
