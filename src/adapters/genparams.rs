//! Boundary mapping between the pure domain [`GenParams`] value object and the
//! `serde_json` wire representation (ADR-0003).
//!
//! The domain stays serde-free; the adapters own the JSON translation. The
//! `openai` adapter projects a spec's gen-params onto the extended speech body
//! with [`to_json`], and the `daemon` wire DTO round-trips them through
//! [`to_json`] / [`from_json`].

use serde_json::{Map, Number, Value};

use crate::domain::gen_params::{GenParams, GenValue};

/// Project the domain gen-params onto a JSON object (canonical key order).
#[must_use]
pub fn to_json(params: &GenParams) -> Map<String, Value> {
    params
        .iter()
        .map(|(key, value)| (key.clone(), value_to_json(value)))
        .collect()
}

/// Rebuild the domain gen-params from a JSON object received over the wire,
/// keeping only the scalar arms the value object models.
#[must_use]
pub fn from_json(map: Map<String, Value>) -> GenParams {
    let mut params = GenParams::new();
    for (key, value) in map {
        if let Some(value) = json_to_value(&value) {
            params.insert(key, value);
        }
    }
    params
}

fn value_to_json(value: &GenValue) -> Value {
    match value {
        GenValue::Int(i) => Value::from(*i),
        GenValue::Float(f) => Value::from(*f),
        GenValue::Bool(b) => Value::from(*b),
        GenValue::Str(s) => Value::from(s.clone()),
    }
}

fn json_to_value(value: &Value) -> Option<GenValue> {
    match value {
        Value::Bool(b) => Some(GenValue::Bool(*b)),
        Value::Number(n) => number_to_value(n),
        Value::String(s) => Some(GenValue::Str(s.clone())),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn number_to_value(n: &Number) -> Option<GenValue> {
    n.as_i64()
        .map(GenValue::Int)
        .or_else(|| n.as_f64().map(GenValue::Float))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GenParams {
        let mut p = GenParams::new();
        p.insert("num_step".to_owned(), GenValue::Int(24));
        p.insert("guidance_scale".to_owned(), GenValue::Float(3.0));
        p.insert("denoise".to_owned(), GenValue::Bool(true));
        p.insert(
            "preprocess_prompt".to_owned(),
            GenValue::Str("warm".to_owned()),
        );
        p
    }

    #[test]
    fn to_json_maps_each_scalar_arm() {
        let json = to_json(&sample());
        assert_eq!(json["num_step"], Value::from(24));
        assert_eq!(json["guidance_scale"], Value::from(3.0));
        assert_eq!(json["denoise"], Value::from(true));
        assert_eq!(json["preprocess_prompt"], Value::from("warm"));
    }

    #[test]
    fn round_trips_through_json() {
        let params = sample();
        let back = from_json(to_json(&params));
        assert_eq!(back, params);
    }

    #[test]
    fn from_json_drops_unsupported_arms() {
        let mut map = Map::new();
        map.insert("num_step".to_owned(), Value::from(8));
        map.insert("nested".to_owned(), Value::Array(vec![Value::from(1)]));
        let params = from_json(map);
        assert_eq!(params.get("num_step"), Some(&GenValue::Int(8)));
        assert!(!params.contains_key("nested"));
    }
}
