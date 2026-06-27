//! Generation-params value object (T013).
//!
//! The validated pass-through knobs for the extended speech request, modelled as
//! a pure domain map ([`GenParams`] of [`GenValue`]) with zero framework types
//! (ADR-0003) — the adapters translate it to/from JSON at the boundary. `num_step`
//! is the only canonical step key; `steps` is a CLI alias that normalizes to it;
//! `num_steps` and any other key are rejected. This keeps the wire request in
//! lockstep with the server's documented gen-param surface.

use std::collections::BTreeMap;

use crate::domain::errors::DomainError;

/// Canonical generation-param keys (excludes the `steps` CLI alias).
pub const CANONICAL_KEYS: &[&str] = &[
    "num_step",
    "guidance_scale",
    "t_shift",
    "layer_penalty_factor",
    "position_temperature",
    "class_temperature",
    "denoise",
    "preprocess_prompt",
    "postprocess_output",
    "audio_chunk_duration",
    "audio_chunk_threshold",
];

/// A single generation-param value (the scalar arms the server accepts).
#[derive(Debug, Clone, PartialEq)]
pub enum GenValue {
    /// Integer scalar (e.g. `num_step`).
    Int(i64),
    /// Floating-point scalar (e.g. `guidance_scale`).
    Float(f64),
    /// Boolean toggle (e.g. `denoise`).
    Bool(bool),
    /// String scalar (free-form fallback).
    Str(String),
}

/// A validated, ordered set of generation-param overrides (pure domain map).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GenParams {
    entries: BTreeMap<String, GenValue>,
}

impl GenParams {
    /// An empty override set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether no overrides are set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The number of overrides set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The value for `key`, if set.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&GenValue> {
        self.entries.get(key)
    }

    /// Whether `key` is set.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Set (or overwrite) `key` to `value`.
    pub fn insert(&mut self, key: String, value: GenValue) {
        self.entries.insert(key, value);
    }

    /// Iterate the overrides in canonical (sorted) key order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &GenValue)> {
        self.entries.iter()
    }
}

impl IntoIterator for GenParams {
    type Item = (String, GenValue);
    type IntoIter = std::collections::btree_map::IntoIter<String, GenValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

/// Resolve a `--set` key to its canonical form, applying the `steps -> num_step`
/// alias and rejecting unknown keys (including `num_steps`).
pub fn canonical_key(key: &str) -> Result<&'static str, DomainError> {
    if key == "steps" {
        return Ok("num_step");
    }
    CANONICAL_KEYS
        .iter()
        .copied()
        .find(|k| *k == key)
        .ok_or_else(|| DomainError::UnknownGenParam(key.to_owned()))
}

/// Parse repeatable `--set key=value` overrides into a validated [`GenParams`].
pub fn parse_overrides(sets: &[String]) -> Result<GenParams, DomainError> {
    let mut params = GenParams::new();
    for entry in sets {
        let (key, raw) = entry
            .split_once('=')
            .ok_or_else(|| DomainError::MalformedOverride(entry.clone()))?;
        if raw.is_empty() {
            return Err(DomainError::EmptyGenParamValue(key.trim().to_owned()));
        }
        params.insert(
            canonical_key(key.trim())?.to_owned(),
            parse_scalar(raw.trim()),
        );
    }
    Ok(params)
}

/// Coerce a raw `--set` value to the tightest scalar (int, float, bool, str).
#[must_use]
pub fn parse_scalar(raw: &str) -> GenValue {
    if let Ok(i) = raw.parse::<i64>() {
        GenValue::Int(i)
    } else if let Ok(f) = raw.parse::<f64>() {
        GenValue::Float(f)
    } else if let Ok(b) = raw.parse::<bool>() {
        GenValue::Bool(b)
    } else {
        GenValue::Str(raw.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_aliases_to_num_step() {
        let map = parse_overrides(&["steps=32".to_owned()]).unwrap();
        assert_eq!(map.get("num_step"), Some(&GenValue::Int(32)));
        assert!(!map.contains_key("steps"));
    }

    #[test]
    fn rejects_num_steps() {
        assert!(canonical_key("num_steps").is_err());
        assert!(parse_overrides(&["num_steps=8".to_owned()]).is_err());
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(parse_overrides(&["wat=1".to_owned()]).is_err());
    }

    #[test]
    fn coerces_scalar_types() {
        assert_eq!(parse_scalar("7"), GenValue::Int(7));
        assert_eq!(parse_scalar("0.5"), GenValue::Float(0.5));
        assert_eq!(parse_scalar("true"), GenValue::Bool(true));
        assert_eq!(parse_scalar("hi"), GenValue::Str("hi".to_owned()));
    }

    #[test]
    fn requires_key_value() {
        assert!(parse_overrides(&["num_step".to_owned()]).is_err());
    }

    #[test]
    fn canonical_key_passes_known_keys_through() {
        assert_eq!(canonical_key("guidance_scale").unwrap(), "guidance_scale");
        assert_eq!(canonical_key("denoise").unwrap(), "denoise");
        assert_eq!(canonical_key("steps").unwrap(), "num_step");
    }

    #[test]
    fn canonical_set_has_expected_arity() {
        // The 11 documented gen-param keys (the `steps` alias is excluded).
        assert_eq!(CANONICAL_KEYS.len(), 11);
        assert!(CANONICAL_KEYS.contains(&"num_step"));
        assert!(!CANONICAL_KEYS.contains(&"steps"));
    }

    #[test]
    fn rejects_empty_value() {
        assert!(parse_overrides(&["num_step=".to_owned()]).is_err());
    }

    #[test]
    fn trims_key_and_value_whitespace() {
        let map = parse_overrides(&[" guidance_scale = 3 ".to_owned()]).unwrap();
        assert_eq!(map.get("guidance_scale"), Some(&GenValue::Int(3)));
    }

    #[test]
    fn later_override_wins_for_same_key() {
        let map = parse_overrides(&["num_step=8".to_owned(), "num_step=24".to_owned()]).unwrap();
        assert_eq!(map.get("num_step"), Some(&GenValue::Int(24)));
    }
}
