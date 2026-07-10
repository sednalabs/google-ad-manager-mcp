use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::AdManagerError;
use crate::fingerprint::stable_fingerprint;

use super::MAX_RETIREMENT_TARGETS;

pub(super) struct RetirementTarget {
    pub(super) ad_unit_id: String,
    pub(super) resource_name: String,
}

pub(super) fn validate_network_code(value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 20 || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(AdManagerError::invalid(
            "network_code",
            "must be a positive numeric identifier of at most 20 digits",
        ));
    }
    let canonical = trimmed
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value.to_string())
        .ok_or_else(|| {
            AdManagerError::invalid(
                "network_code",
                "must be a positive numeric identifier of at most 20 digits",
            )
        })?;
    if canonical != trimmed {
        return Err(AdManagerError::invalid(
            "network_code",
            "must use its canonical numeric form without leading zeroes",
        ));
    }
    Ok(canonical)
}

pub(super) fn validate_targets(
    network_code: &str,
    ad_unit_ids: &[String],
) -> Result<Vec<RetirementTarget>, AdManagerError> {
    let mut targets = BTreeMap::new();
    for ad_unit_id in validate_numeric_ids("ad_unit_ids", ad_unit_ids, MAX_RETIREMENT_TARGETS)? {
        targets.insert(
            ad_unit_id.clone(),
            RetirementTarget {
                resource_name: format!("networks/{network_code}/adUnits/{ad_unit_id}"),
                ad_unit_id,
            },
        );
    }

    if targets.is_empty() {
        return Err(AdManagerError::invalid(
            "ad_unit_ids",
            "must include at least one exact ad-unit id",
        ));
    }
    if targets.len() > MAX_RETIREMENT_TARGETS {
        return Err(AdManagerError::invalid(
            "ad_unit_ids",
            format!("must include at most {MAX_RETIREMENT_TARGETS} unique exact targets"),
        ));
    }
    Ok(targets.into_values().collect())
}

fn canonical_positive_id(value: &str, field: &'static str) -> Result<String, AdManagerError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value.to_string())
        .ok_or_else(|| AdManagerError::invalid(field, "must contain positive numeric identifiers"))
}

pub(super) fn validate_numeric_ids(
    field: &'static str,
    ids: &[String],
    limit: usize,
) -> Result<Vec<String>, AdManagerError> {
    if ids.len() > limit {
        return Err(AdManagerError::invalid(
            field,
            format!("must contain at most {limit} identifiers"),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut canonical = Vec::with_capacity(ids.len());
    for value in ids {
        let id = canonical_positive_id(value.trim(), field)?;
        if !seen.insert(id.clone()) {
            return Err(AdManagerError::invalid(
                field,
                format!("duplicate canonical identifier `{id}`"),
            ));
        }
        canonical.push(id);
    }
    Ok(canonical)
}

pub(super) fn summarize_identity(target: &RetirementTarget, row: &Value) -> Value {
    let resource_name = row.get("name").and_then(Value::as_str).unwrap_or_default();
    let resolved_id = numeric_id(resource_name);
    let current = json!({
        "ad_unit_id": resolved_id,
        "ad_unit_code": bounded_string(row.get("adUnitCode"), 128),
        "status": bounded_string(row.get("status"), 32),
        "sizes": compact_sizes(row),
        "has_children": row.get("hasChildren").and_then(Value::as_bool),
        "parent_ad_unit_id": row.get("parentAdUnit").and_then(Value::as_str).and_then(numeric_id),
        "update_time": bounded_string(row.get("updateTime"), 64),
    });
    let matches = resource_name == target.resource_name
        && resolved_id.as_deref() == Some(target.ad_unit_id.as_str());
    json!({
        "ad_unit_id": target.ad_unit_id,
        "resource_name": target.resource_name,
        "proof_state": if matches { "complete_clear" } else { "complete_blocked" },
        "identity_matches_request": matches,
        "identity_fingerprint": stable_fingerprint(&current.to_string()),
        "current": current,
    })
}

pub(super) fn blocked_identity(target: &RetirementTarget, err: AdManagerError) -> Value {
    let (state, reason) = match err {
        AdManagerError::UpstreamApi {
            status: 401 | 403, ..
        }
        | AdManagerError::WriteScopeRequired { .. } => (
            "blocked_permission",
            "the authenticated principal could not read this exact ad unit",
        ),
        AdManagerError::UpstreamApi { status: 404, .. } => (
            "complete_blocked",
            "the exact ad-unit resource was not found",
        ),
        _ => (
            "not_run",
            "the exact ad-unit identity read did not complete",
        ),
    };
    json!({
        "ad_unit_id": target.ad_unit_id,
        "resource_name": target.resource_name,
        "proof_state": state,
        "identity_matches_request": false,
        "reason": reason,
    })
}

pub(super) fn summarize_identities(identities: &[Value]) -> Value {
    let state = if identities
        .iter()
        .any(|value| proof_state(value) == "complete_blocked")
    {
        "complete_blocked"
    } else if identities
        .iter()
        .any(|value| proof_state(value) == "blocked_permission")
    {
        "blocked_permission"
    } else if identities
        .iter()
        .all(|value| proof_state(value) == "complete_clear")
    {
        "complete_clear"
    } else {
        "not_run"
    };
    json!({
        "proof_state": state,
        "target_count": identities.len(),
        "result_fingerprint": stable_fingerprint(&Value::Array(identities.to_vec()).to_string()),
        "targets": identities,
    })
}

pub(super) fn proof_state(value: &Value) -> &str {
    value
        .get("proof_state")
        .and_then(Value::as_str)
        .unwrap_or("not_run")
}

fn compact_sizes(row: &Value) -> Vec<String> {
    row.get("adUnitSizes")
        .or_else(|| row.get("sizes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let size = entry.get("size").unwrap_or(entry);
            let width = size.get("width").and_then(Value::as_u64)?;
            let height = size.get("height").and_then(Value::as_u64)?;
            Some(format!("{width}x{height}"))
        })
        .take(20)
        .collect()
}

pub(super) fn bounded_string(value: Option<&Value>, max_bytes: usize) -> Value {
    let Some(value) = value.and_then(Value::as_str) else {
        return Value::Null;
    };
    let bounded = value.chars().take(max_bytes).collect::<String>();
    Value::String(bounded)
}

pub(super) fn numeric_id(value: &str) -> Option<String> {
    let candidate = value.rsplit('/').next().unwrap_or(value).trim();
    candidate
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value.to_string())
}
