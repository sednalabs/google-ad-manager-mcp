use serde_json::{Map, Value, json};

use crate::contract;
use crate::fingerprint::stable_fingerprint;

use super::validation::utf8_prefix;

#[derive(Clone, Copy)]
pub(super) enum Class {
    Array,
    Map,
    RawSoap,
    Reason,
    Transport,
}

impl Class {
    const fn name(self) -> &'static str {
        match self {
            Self::Array => "expanded_array",
            Self::Map => "expanded_map",
            Self::RawSoap => "raw_soap",
            Self::Reason => "reason_detail",
            Self::Transport => "transport_detail",
        }
    }
}

#[derive(Default)]
pub(super) struct Ledger(pub(super) Vec<Value>);

impl Ledger {
    pub(super) fn omit(&mut self, path: &str, value: &Value, class: Class) -> Result<(), String> {
        let (unit, count) = match value {
            Value::Array(values) => ("items", values.len()),
            Value::Object(values) => ("entries", values.len()),
            Value::String(value) => ("utf8_bytes", value.len()),
            _ => ("values", 1),
        };
        if matches!(class, Class::Array) && !value.is_array()
            || matches!(class, Class::Map) && !value.is_object()
            || matches!(class, Class::RawSoap) && !value.is_string()
        {
            return Err(format!("typed omission at {path} had the wrong value kind"));
        }
        let mut row = json!({
            "path": path, "class": class.name(), "unit": unit,
            "source_count": count, "retained_count": 0, "omitted_count": count,
            "derived_witness_count": 0,
            "source_value_fingerprint": stable_fingerprint(&value.to_string()),
        });
        if let Some(witness) = witness(value, class) {
            row["witness"] = witness;
            row["derived_witness_count"] = json!(1);
        }
        self.0.push(row);
        Ok(())
    }

    pub(super) fn aggregate(
        &mut self,
        path: &str,
        source: &Value,
        class: Class,
        unit: &str,
        count: usize,
    ) {
        self.0.push(json!({
            "path": path, "class": class.name(), "unit": unit,
            "source_count": count, "retained_count": 0, "omitted_count": count,
            "derived_witness_count": 0,
            "source_value_fingerprint": stable_fingerprint(&source.to_string()),
        }));
    }
}

pub(super) fn select(
    source: &Map<String, Value>,
    path: &str,
    keep: &[&str],
    omit: &[(&str, Class)],
    ledger: &mut Ledger,
) -> Result<Map<String, Value>, String> {
    for key in source.keys() {
        if !keep.contains(&key.as_str()) && !omit.iter().any(|(field, _)| *field == key.as_str()) {
            return Err(format!("{path} contained unprojected field {key}"));
        }
    }
    let mut result = Map::new();
    for key in keep {
        if let Some(value) = source.get(*key) {
            result.insert((*key).into(), value.clone());
        }
    }
    for (key, class) in omit {
        if let Some(value) = source.get(*key) {
            ledger.omit(&format!("{path}/{key}"), value, *class)?;
        }
    }
    Ok(result)
}

fn witness(value: &Value, class: Class) -> Option<Value> {
    match (class, value) {
        (Class::Array | Class::Reason, Value::Array(values)) => values
            .iter()
            .enumerate()
            .min_by_key(|(_, value)| witness_rank(value))
            .map(|(index, value)| {
                json!({
                    "source_index": index,
                    "value": compact_witness_value(value),
                    "source_value_fingerprint": stable_fingerprint(&value.to_string()),
                })
            }),
        (Class::Map, Value::Object(values)) => values.iter().next().map(|(key, value)| {
            json!({
                "source_key": utf8_prefix(key, 64),
                "value": compact_witness_value(value),
                "source_value_fingerprint": stable_fingerprint(&value.to_string()),
            })
        }),
        _ => None,
    }
}

fn witness_rank(value: &Value) -> u8 {
    let priority = [
        (
            "decision",
            [
                "attention_required",
                "dependencies_found",
                "targeted_exposed",
            ],
        ),
        (
            "proof_state",
            ["blocked", "sample_only", "sample_or_shape_incomplete"],
        ),
        (
            "classification",
            ["targeted_exposed", "exact_target", "placement_target"],
        ),
    ];
    priority
        .iter()
        .position(|(field, values)| {
            value
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|candidate| values.contains(&candidate))
        })
        .map_or(3, |rank| rank as u8)
}

fn compact_witness_value(value: &Value) -> Value {
    const FIELDS: &[&str] = &[
        "ad_unit_id",
        "ad_unit_code",
        "line_item_id",
        "placement_id",
        "yield_group_id",
        "requested_ad_unit_id",
        "decision",
        "proof_state",
        "classification",
        "status",
    ];
    match value {
        Value::String(value) => {
            let redacted = contract::redact_secret_text(value);
            json!(utf8_prefix(&redacted, 80))
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .take(2)
                .filter(|value| value.is_string() || value.is_number() || value.is_boolean())
                .map(compact_witness_value)
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            FIELDS
                .iter()
                .filter_map(|field| {
                    values
                        .get(*field)
                        .map(|value| ((*field).to_string(), compact_witness_value(value)))
                })
                .take(6)
                .collect(),
        ),
        Value::Bool(_) | Value::Number(_) | Value::Null => value.clone(),
    }
}
