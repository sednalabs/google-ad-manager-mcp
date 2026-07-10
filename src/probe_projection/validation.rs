use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::ProbeKind;

pub(super) fn string_counts(
    rows: &[Value],
    field: &str,
) -> Result<BTreeMap<String, usize>, String> {
    let mut counts = BTreeMap::new();
    for row in rows {
        let label = text(object(row, "classified row")?, field, "classified row")?;
        if label.len() > 64 || label.chars().any(char::is_control) {
            return Err(format!("classification {field} was not a bounded label"));
        }
        *counts.entry(label.into()).or_default() += 1;
    }
    Ok(counts)
}

pub(super) fn status_counts(
    rows: &[Value],
    field: &str,
) -> Result<BTreeMap<String, usize>, String> {
    let mut counts = BTreeMap::new();
    for row in rows {
        let row = object(row, "status row")?;
        let label = match row.get(field) {
            None | Some(Value::Null) => "unknown",
            Some(Value::String(value))
                if value.len() <= 64 && !value.chars().any(char::is_control) =>
            {
                value
            }
            _ => return Err(format!("status {field} was not a bounded label")),
        };
        *counts.entry(label.into()).or_default() += 1;
    }
    Ok(counts)
}

pub(super) fn bool_counts(
    rows: &[Value],
    field: &str,
) -> Result<BTreeMap<&'static str, usize>, String> {
    let mut counts = BTreeMap::from([("false", 0_usize), ("true", 0), ("unknown", 0)]);
    for row in rows {
        let key = match row.get(field) {
            Some(Value::Bool(true)) => "true",
            Some(Value::Bool(false)) => "false",
            None | Some(Value::Null) => "unknown",
            _ => return Err(format!("boolean classification {field} was invalid")),
        };
        *counts.get_mut(key).expect("known bool key") += 1;
    }
    Ok(counts)
}

pub(super) fn target_identity_summary(rows: &[Value]) -> Result<Value, String> {
    let mut ids = BTreeSet::new();
    let mut missing = 0_usize;
    let mut duplicates = 0_usize;
    for row in rows {
        let row = object(row, "ad unit identity")?;
        match row.get("ad_unit_id") {
            Some(Value::String(id)) if canonical_id(id) => {
                if !ids.insert(id.clone()) {
                    duplicates = duplicates.saturating_add(1);
                }
            }
            None | Some(Value::Null) => missing = missing.saturating_add(1),
            _ => return Err("ad unit identity contained a noncanonical id".into()),
        }
    }
    let canonical_count = ids.len();
    Ok(json!({
        "canonical_ad_unit_ids": ids,
        "canonical_ad_unit_id_count": canonical_count,
        "missing_ad_unit_id_count": missing,
        "duplicate_ad_unit_id_count": duplicates,
    }))
}

pub(super) fn canonical_target_ids(rows: &[Value]) -> Result<BTreeSet<String>, String> {
    let mut ids = BTreeSet::new();
    for row in rows {
        let row = object(row, "ad unit identity")?;
        match row.get("ad_unit_id") {
            Some(Value::String(id)) if canonical_id(id) => {
                if !ids.insert(id.clone()) {
                    return Err("ad unit identities contained a duplicate canonical id".into());
                }
            }
            None | Some(Value::Null) => {}
            _ => return Err("ad unit identity contained a noncanonical id".into()),
        }
    }
    Ok(ids)
}

pub(super) fn canonical_id_set(values: &[Value], name: &str) -> Result<BTreeSet<String>, String> {
    let mut ids = BTreeSet::new();
    for value in values {
        let id = value
            .as_str()
            .filter(|id| canonical_id(id))
            .ok_or_else(|| format!("{name} contained a noncanonical id"))?;
        if !ids.insert(id.to_string()) {
            return Err(format!("{name} contained a duplicate id"));
        }
    }
    Ok(ids)
}

pub(super) fn require_target_member(
    value: &Value,
    target_ids: &BTreeSet<String>,
    name: &str,
) -> Result<(), String> {
    let id = value
        .as_str()
        .filter(|id| canonical_id(id))
        .ok_or_else(|| format!("{name} was not a canonical target id"))?;
    if !target_ids.contains(id) {
        return Err(format!("{name} was outside the probe target scope"));
    }
    Ok(())
}

pub(super) fn verify_no_mutation(
    kind: ProbeKind,
    full: &Value,
    meta: &Value,
) -> Result<(), String> {
    if meta.get("mutation_performed").and_then(Value::as_bool) != Some(false) {
        return Err("probe metadata did not prove mutation_performed=false".into());
    }
    reject_mutation_claim(full)?;
    if kind == ProbeKind::AdUnitDependency
        && full.get("mutation_performed").and_then(Value::as_bool) != Some(false)
    {
        return Err("dependency result did not prohibit mutation".into());
    }
    Ok(())
}

fn reject_mutation_claim(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            if map
                .get("mutation_performed")
                .is_some_and(|value| value.as_bool() != Some(false))
            {
                return Err("probe contained a non-false mutation claim".into());
            }
            map.values().try_for_each(reject_mutation_claim)
        }
        Value::Array(values) => values.iter().try_for_each(reject_mutation_claim),
        _ => Ok(()),
    }
}

pub(super) fn exact_keys(
    source: &Map<String, Value>,
    keys: &[&str],
    name: &str,
) -> Result<(), String> {
    if source.len() != keys.len() || source.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(format!(
            "{name} fields did not match the projection contract"
        ));
    }
    Ok(())
}

pub(super) fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{name} was not an object"))
}

pub(super) fn as_object_mut<'a>(
    value: &'a mut Value,
    name: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    value
        .as_object_mut()
        .ok_or_else(|| format!("{name} was not an object"))
}

pub(super) fn get<'a>(
    source: &'a Map<String, Value>,
    field: &str,
    name: &str,
) -> Result<&'a Value, String> {
    source
        .get(field)
        .ok_or_else(|| format!("{name} omitted {field}"))
}

pub(super) fn text<'a>(
    source: &'a Map<String, Value>,
    field: &str,
    name: &str,
) -> Result<&'a str, String> {
    get(source, field, name)?
        .as_str()
        .ok_or_else(|| format!("{name}.{field} was not text"))
}

pub(super) fn array<'a>(
    source: &'a Map<String, Value>,
    field: &str,
    name: &str,
) -> Result<&'a Vec<Value>, String> {
    get(source, field, name)?
        .as_array()
        .ok_or_else(|| format!("{name}.{field} was not an array"))
}

pub(super) fn false_field(
    source: &Map<String, Value>,
    field: &str,
    name: &str,
) -> Result<(), String> {
    if get(source, field, name)?.as_bool() != Some(false) {
        return Err(format!("{name}.{field} was not false"));
    }
    Ok(())
}

pub(super) fn flag(source: &Map<String, Value>, field: &str, name: &str) -> Result<bool, String> {
    get(source, field, name)?
        .as_bool()
        .ok_or_else(|| format!("{name}.{field} was not boolean"))
}

pub(super) fn count(source: &Map<String, Value>, field: &str, name: &str) -> Result<usize, String> {
    let value = get(source, field, name)?
        .as_u64()
        .ok_or_else(|| format!("{name}.{field} was not an unsigned count"))?;
    usize::try_from(value).map_err(|_| format!("{name}.{field} exceeded usize"))
}

pub(super) fn false_if_present(
    source: &Map<String, Value>,
    field: &str,
    name: &str,
) -> Result<(), String> {
    if source.get(field).is_some() {
        false_field(source, field, name)?;
    }
    Ok(())
}

pub(super) fn validate_truncation_pairs(
    source: &Map<String, Value>,
    pairs: &[(&str, &str)],
    name: &str,
) -> Result<(), String> {
    for (value_field, truncated_field) in pairs {
        match (source.get(*value_field), source.get(*truncated_field)) {
            (Some(_), Some(Value::Bool(_))) | (None, None) => {}
            _ => {
                return Err(format!(
                    "{name}.{value_field} and {truncated_field} did not form an explicit truncation pair"
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn canonical_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && !value.starts_with('0')
        && value.parse::<u64>().is_ok_and(|value| value > 0)
}

pub(super) fn utf8_prefix(value: &str, max: usize) -> &str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
