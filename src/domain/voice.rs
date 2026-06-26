//! Voice value objects (T012): saved-voice entries, the clone selector, the
//! built-in standard voice, and the three-arm [`VoiceMode`].
//!
//! Pure: names are validated for non-emptiness and trimmed; no server lookups
//! happen here. [`VoiceMode`] is the domain Strategy selector the `say` /
//! `realtime` use cases resolve into a wire request — exactly one of design,
//! clone, or standard (FR-2 / ADR-0003).

use crate::domain::errors::DomainError;
use crate::domain::voice_design::VoiceDesign;

/// A saved voice as listed by the server's `GET /voices` (name + ref-text flag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    name: String,
    has_ref_text: bool,
}

impl Voice {
    /// Build a saved-voice entry, validating the (trimmed) name is non-empty.
    pub fn new(name: &str, has_ref_text: bool) -> Result<Self, DomainError> {
        Ok(Self {
            name: validate_name(name)?,
            has_ref_text,
        })
    }

    /// The saved voice name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether a reference transcript is stored alongside this voice.
    #[must_use]
    pub fn has_ref_text(&self) -> bool {
        self.has_ref_text
    }
}

/// Clone-mode selection: a saved voice name plus an optional reference transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceClone {
    name: String,
    ref_text: Option<String>,
}

impl VoiceClone {
    /// Build a clone selector; blank `ref_text` normalizes to `None`.
    pub fn new(name: &str, ref_text: Option<&str>) -> Result<Self, DomainError> {
        Ok(Self {
            name: validate_name(name)?,
            ref_text: ref_text
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned),
        })
    }

    /// The saved voice name to clone.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The optional reference transcript for the clone.
    #[must_use]
    pub fn ref_text(&self) -> Option<&str> {
        self.ref_text.as_deref()
    }
}

/// A named built-in voice (e.g. the `[tts].voice` default `alloy`), distinct
/// from a saved clone (ADR-0003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardVoice {
    name: String,
}

impl StandardVoice {
    /// Build a standard-voice selector, validating the (trimmed) name.
    pub fn new(name: &str) -> Result<Self, DomainError> {
        Ok(Self {
            name: validate_name(name)?,
        })
    }

    /// The built-in voice name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The three interchangeable voice strategies for a synthesis request (FR-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceMode {
    /// Voice design from canonical tags (`--instruct`).
    Design(VoiceDesign),
    /// Clone of a saved voice (`--voice` + optional `--ref-text`).
    Clone(VoiceClone),
    /// A named built-in voice / server default.
    Standard(StandardVoice),
}

/// Trim and reject an empty voice name.
fn validate_name(name: &str) -> Result<String, DomainError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(DomainError::InvalidVoiceName(name.to_owned()));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_validates_name() {
        let v = Voice::new("  narrator ", true).unwrap();
        assert_eq!(v.name(), "narrator");
        assert!(v.has_ref_text());
        assert!(Voice::new("   ", false).is_err());
    }

    #[test]
    fn clone_normalizes_blank_ref_text_to_none() {
        let c = VoiceClone::new("narrator", Some("   ")).unwrap();
        assert_eq!(c.ref_text(), None);
        let c2 = VoiceClone::new("narrator", Some(" the quick fox ")).unwrap();
        assert_eq!(c2.ref_text(), Some("the quick fox"));
    }

    #[test]
    fn clone_rejects_empty_name() {
        assert!(VoiceClone::new("", None).is_err());
    }

    #[test]
    fn standard_voice_carries_name() {
        assert_eq!(StandardVoice::new("alloy").unwrap().name(), "alloy");
    }

    #[test]
    fn voice_mode_arms_are_distinct() {
        let design = VoiceMode::Design(VoiceDesign::parse("whisper").unwrap());
        let clone = VoiceMode::Clone(VoiceClone::new("narrator", None).unwrap());
        let standard = VoiceMode::Standard(StandardVoice::new("alloy").unwrap());
        assert_ne!(design, clone);
        assert_ne!(clone, standard);
        assert_eq!(
            standard,
            VoiceMode::Standard(StandardVoice::new("alloy").unwrap())
        );
    }
}
