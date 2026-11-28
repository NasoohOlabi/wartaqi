use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use chrono::{DateTime, NaiveDate};
use regex::Regex;
use serde_json::Value;

pub(super) fn regex(pattern: &str) -> Option<Arc<Regex>> {
    static PATTERNS: LazyLock<Mutex<HashMap<String, Arc<Regex>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut patterns = PATTERNS.lock().ok()?;
    if let Some(pattern) = patterns.get(pattern) {
        return Some(pattern.clone());
    }
    if patterns.len() >= 128 {
        patterns.clear();
    }
    let compiled = Arc::new(Regex::new(pattern).ok()?);
    patterns.insert(pattern.into(), compiled.clone());
    Some(compiled)
}

pub(super) fn date(value: &str) -> bool {
    value.len() == 10
        && NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .is_ok_and(|date| date.format("%Y-%m-%d").to_string() == value)
}

pub(super) fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

const KEYWORDS: &[&str] = &[
    "$schema",
    "additionalProperties",
    "anyOf",
    "const",
    "default",
    "description",
    "enum",
    "format",
    "items",
    "maxItems",
    "maxLength",
    "maximum",
    "minItems",
    "minLength",
    "minimum",
    "oneOf",
    "pattern",
    "properties",
    "required",
    "type",
];

// Wartaq publishes this subset of draft-07. Reject any other schema keyword or
// malformed constraint, and reject properties the catalog did not describe.
pub(super) fn supported(schema: &Value, depth: usize) -> bool {
    let Some(fields) = schema.as_object() else {
        return false;
    };
    depth <= 16
        && !fields.is_empty()
        && fields.iter().all(|(key, value)| {
            if !KEYWORDS.contains(&key.as_str()) {
                return false;
            }
            match key.as_str() {
                "properties" => value
                    .as_object()
                    .is_some_and(|map| map.values().all(|value| supported(value, depth + 1))),
                "anyOf" | "oneOf" => value.as_array().is_some_and(|items| {
                    !items.is_empty() && items.iter().all(|value| supported(value, depth + 1))
                }),
                "items" => supported(value, depth + 1),
                "additionalProperties" => value.is_boolean(),
                "format" => matches!(value.as_str(), Some("uuid" | "date" | "date-time")),
                "type" => {
                    value.as_str().is_some_and(known_type)
                        || value.as_array().is_some_and(|items| {
                            !items.is_empty()
                                && items
                                    .iter()
                                    .all(|value| value.as_str().is_some_and(known_type))
                        })
                }
                "required" => value
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string)),
                "enum" => value.as_array().is_some_and(|items| !items.is_empty()),
                "minItems" | "maxItems" | "minLength" | "maxLength" => value.as_u64().is_some(),
                "minimum" | "maximum" => value.is_number(),
                "pattern" => value
                    .as_str()
                    .is_some_and(|pattern| pattern.len() <= 512 && regex(pattern).is_some()),
                _ => true,
            }
        })
}

fn known_type(kind: &str) -> bool {
    matches!(
        kind,
        "array" | "boolean" | "integer" | "null" | "number" | "object" | "string"
    )
}

fn has_type(value: &Value, kind: &str) -> bool {
    match kind {
        "array" => value.is_array(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_f64().is_some_and(|number| number.fract() == 0.0),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "object" => value.is_object(),
        "string" => value.is_string(),
        _ => false,
    }
}

pub(super) fn accepts(schema: &Value, value: &Value) -> bool {
    let Some(schema) = schema.as_object() else {
        return false;
    };
    let fits = |number: f64, min: &str, max: &str| {
        schema
            .get(min)
            .and_then(Value::as_f64)
            .is_none_or(|min| number >= min)
            && schema
                .get(max)
                .and_then(Value::as_f64)
                .is_none_or(|max| number <= max)
    };
    if let Some(kind) = schema.get("type")
        && !kind.as_str().is_some_and(|kind| has_type(value, kind))
        && !kind.as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item.as_str().is_some_and(|kind| has_type(value, kind)))
        })
    {
        return false;
    }
    if schema.get("const").is_some_and(|item| item != value)
        || schema
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.contains(value))
        || schema
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.iter().any(|item| accepts(item, value)))
        || schema
            .get("oneOf")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .filter(|item| accepts(item, value))
                    .take(2)
                    .count()
                    != 1
            })
    {
        return false;
    }
    match value {
        Value::Object(items) => {
            let has_required =
                schema
                    .get("required")
                    .and_then(Value::as_array)
                    .is_none_or(|required| {
                        required
                            .iter()
                            .all(|name| name.as_str().is_some_and(|name| items.contains_key(name)))
                    });
            let properties = schema.get("properties").and_then(Value::as_object);
            has_required
                && match properties {
                    Some(properties) => items.iter().all(|(key, item)| {
                        properties
                            .get(key)
                            .is_some_and(|property| accepts(property, item))
                    }),
                    None if schema.contains_key("anyOf") || schema.contains_key("oneOf") => true,
                    None => items.is_empty(),
                }
        }
        Value::Array(items) => {
            fits(items.len() as f64, "minItems", "maxItems")
                && schema
                    .get("items")
                    .is_none_or(|schema| items.iter().all(|item| accepts(schema, item)))
        }
        Value::String(value) => {
            let format_ok = match schema.get("format").and_then(Value::as_str) {
                Some("uuid") => uuid(value),
                Some("date") => date(value),
                Some("date-time") => DateTime::parse_from_rfc3339(value).is_ok(),
                Some(_) => false,
                None => true,
            };
            format_ok
                && fits(value.chars().count() as f64, "minLength", "maxLength")
                && schema
                    .get("pattern")
                    .and_then(Value::as_str)
                    .is_none_or(|pattern| {
                        regex(pattern).is_some_and(|pattern| pattern.is_match(value))
                    })
        }
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|number| fits(number, "minimum", "maximum")),
        Value::Bool(_) | Value::Null => true,
    }
}
