use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{AdManagerError, fingerprint::stable_fingerprint};

use super::MAX_RETIREMENT_TARGETS;

pub(super) struct RetirementTarget {
    pub(super) ad_unit_id: String,
    pub(super) resource_name: String,
}

pub(super) fn validate_targets(
    network_code: &str,
    ad_unit_ids: &[String],
) -> Result<Vec<RetirementTarget>, AdManagerError> {
    validate_canonical_positive_id("network_code", network_code)?;
    if ad_unit_ids.is_empty() {
        return Err(AdManagerError::invalid(
            "ad_unit_ids",
            "must include at least one exact ad-unit id",
        ));
    }
    if ad_unit_ids.len() > MAX_RETIREMENT_TARGETS {
        return Err(AdManagerError::invalid(
            "ad_unit_ids",
            format!("must include at most {MAX_RETIREMENT_TARGETS} exact targets"),
        ));
    }

    let mut targets = BTreeMap::new();
    for raw_id in ad_unit_ids {
        let ad_unit_id = validate_canonical_positive_id("ad_unit_ids", raw_id)?;
        if targets.contains_key(&ad_unit_id) {
            return Err(AdManagerError::invalid(
                "ad_unit_ids",
                format!("duplicate canonical identifier `{ad_unit_id}`"),
            ));
        }
        targets.insert(
            ad_unit_id.clone(),
            RetirementTarget {
                resource_name: format!("networks/{network_code}/adUnits/{ad_unit_id}"),
                ad_unit_id,
            },
        );
    }
    Ok(targets.into_values().collect())
}

fn validate_canonical_positive_id(
    field: &'static str,
    value: &str,
) -> Result<String, AdManagerError> {
    if value.is_empty()
        || value.len() > 20
        || !value.chars().all(|ch| ch.is_ascii_digit())
        || value.starts_with('0')
    {
        return Err(AdManagerError::invalid(
            field,
            "must use canonical positive numeric identifiers of at most 20 digits without whitespace or leading zeroes",
        ));
    }
    let canonical = value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value.to_string())
        .ok_or_else(|| {
            AdManagerError::invalid(
                field,
                "must use canonical positive numeric identifiers of at most 20 digits without whitespace or leading zeroes",
            )
        })?;
    if canonical != value {
        return Err(AdManagerError::invalid(
            field,
            "must use canonical positive numeric identifiers of at most 20 digits without whitespace or leading zeroes",
        ));
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
    let identity_matches_request = resource_name == target.resource_name
        && resolved_id.as_deref() == Some(target.ad_unit_id.as_str());
    json!({
        "ad_unit_id": target.ad_unit_id,
        "resource_name": target.resource_name,
        "proof_state": if identity_matches_request { "complete_clear" } else { "complete_blocked" },
        "identity_matches_request": identity_matches_request,
        "identity_fingerprint": stable_fingerprint(&current.to_string()),
        "current": current,
    })
}

pub(super) fn blocked_identity(target: &RetirementTarget, err: AdManagerError) -> Value {
    let (proof_state, reason) = match err {
        AdManagerError::AuthBootstrap(_)
        | AdManagerError::UpstreamApi {
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
        "proof_state": proof_state,
        "identity_matches_request": false,
        "reason": reason,
    })
}

pub(super) fn summarize_identities(identities: &[Value]) -> Value {
    let proof_state = if identities
        .iter()
        .any(|value| state(value) == "complete_blocked")
    {
        "complete_blocked"
    } else if identities
        .iter()
        .any(|value| state(value) == "blocked_permission")
    {
        "blocked_permission"
    } else if identities
        .iter()
        .all(|value| state(value) == "complete_clear")
    {
        "complete_clear"
    } else {
        "not_run"
    };
    json!({
        "proof_state": proof_state,
        "target_count": identities.len(),
        "result_fingerprint": stable_fingerprint(&Value::Array(identities.to_vec()).to_string()),
        "targets": identities,
    })
}

fn state(value: &Value) -> &str {
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
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(20)
        .collect()
}

fn bounded_string(value: Option<&Value>, max_chars: usize) -> Value {
    let Some(value) = value.and_then(Value::as_str) else {
        return Value::Null;
    };
    Value::String(value.chars().take(max_chars).collect())
}

fn numeric_id(value: &str) -> Option<String> {
    let candidate = value.rsplit('/').next().unwrap_or(value);
    validate_canonical_positive_id("resource_name", candidate).ok()
}
