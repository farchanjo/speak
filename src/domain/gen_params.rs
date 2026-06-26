//! Generation-params value object (T013).
//!
//! The validated pass-through knobs for the extended speech request. `num_step`
//! is the only canonical step key; `steps` is a CLI alias that normalizes to it;
//! `num_steps` and any other key are rejected. This keeps the wire request in
//! lockstep with the server's documented gen-param surface.

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value};

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

/// Resolve a `--set` key to its canonical form, applying the `steps -> num_step`
/// alias and rejecting unknown keys (including `num_steps`).
pub fn canonical_key(key: &str) -> Result<&'static str> {
    if key == "steps" {
        return Ok("num_step");
    }
    CANONICAL_KEYS
        .iter()
        .copied()
        .find(|k| *k == key)
        .ok_or_else(|| {
            anyhow!(
                "unknown generation param '{key}'; valid keys: {}, steps",
                CANONICAL_KEYS.join(", ")
            )
        })
}

/// Parse repeatable `--set key=value` overrides into a validated JSON map.
pub fn parse_overrides(sets: &[String]) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    for entry in sets {
        let (key, raw) = entry
            .split_once('=')
            .ok_or_else(|| anyhow!("--set expects key=value, got '{entry}'"))?;
        if raw.is_empty() {
            bail!("--set value is empty for key '{key}'");
        }
        map.insert(
            canonical_key(key.trim())?.to_owned(),
            parse_scalar(raw.trim()),
        );
    }
    Ok(map)
}

/// Coerce a raw `--set` value to the tightest JSON scalar (int, float, bool, str).
#[must_use]
pub fn parse_scalar(raw: &str) -> Value {
    if let Ok(i) = raw.parse::<i64>() {
        Value::from(i)
    } else if let Ok(f) = raw.parse::<f64>() {
        Value::from(f)
    } else if let Ok(b) = raw.parse::<bool>() {
        Value::from(b)
    } else {
        Value::from(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_aliases_to_num_step() {
        let map = parse_overrides(&["steps=32".to_owned()]).unwrap();
        assert_eq!(map.get("num_step"), Some(&Value::from(32)));
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
        assert_eq!(parse_scalar("7"), Value::from(7));
        assert_eq!(parse_scalar("0.5"), Value::from(0.5));
        assert_eq!(parse_scalar("true"), Value::from(true));
        assert_eq!(parse_scalar("hi"), Value::from("hi"));
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
        assert_eq!(map.get("guidance_scale"), Some(&Value::from(3)));
    }

    #[test]
    fn later_override_wins_for_same_key() {
        let map = parse_overrides(&["num_step=8".to_owned(), "num_step=24".to_owned()]).unwrap();
        assert_eq!(map.get("num_step"), Some(&Value::from(24)));
    }
}
