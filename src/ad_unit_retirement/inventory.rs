use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{AdManagerError, fingerprint::stable_fingerprint};

use super::MAX_RETIREMENT_TARGETS;

pub(super) struct RetirementTarget {
    pub(super) network_code: String,
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
        let target = RetirementTarget {
            network_code: network_code.to_string(),
            resource_name: format!("networks/{network_code}/adUnits/{ad_unit_id}"),
            ad_unit_id: ad_unit_id.clone(),
        };
        if targets.insert(ad_unit_id.clone(), target).is_some() {
            return Err(AdManagerError::invalid(
                "ad_unit_ids",
                format!("duplicate canonical identifier `{ad_unit_id}`"),
            ));
        }
    }
    Ok(targets.into_values().collect())
}

fn validate_canonical_positive_id(
    field: &'static str,
    value: &str,
) -> Result<String, AdManagerError> {
    if value.is_empty()
        || value.len() > 19
        || !value.chars().all(|ch| ch.is_ascii_digit())
        || value.starts_with('0')
    {
        return Err(AdManagerError::invalid(
            field,
            "must use canonical positive signed-64-bit numeric identifiers without whitespace or leading zeroes",
        ));
    }
    let canonical = value
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value.to_string())
        .ok_or_else(|| {
            AdManagerError::invalid(
                field,
                "must use canonical positive signed-64-bit numeric identifiers without whitespace or leading zeroes",
            )
        })?;
    if canonical != value {
        return Err(AdManagerError::invalid(
            field,
            "must use canonical positive signed-64-bit numeric identifiers without whitespace or leading zeroes",
        ));
    }
    Ok(canonical)
}

pub(super) fn summarize_identity(target: &RetirementTarget, row: &Value) -> Value {
    let resource_name = row.get("name").and_then(Value::as_str).unwrap_or_default();
    let resolved_id = exact_ad_unit_resource_id(&target.network_code, resource_name);
    let identity_matches_request = resource_name == target.resource_name
        && resolved_id.as_deref() == Some(target.ad_unit_id.as_str());

    let mut shape_issues = Vec::new();
    let ad_unit_code = required_bounded_string(
        row.get("adUnitCode"),
        128,
        "ad_unit_code",
        &mut shape_issues,
    );
    let ad_unit_code_source_fingerprint = row
        .get("adUnitCode")
        .and_then(Value::as_str)
        .map(stable_fingerprint);
    let status = required_bounded_string(row.get("status"), 32, "status", &mut shape_issues);
    if status
        .as_str()
        .is_some_and(|value| !matches!(value, "ACTIVE" | "INACTIVE" | "ARCHIVED"))
    {
        shape_issues.push("status_unknown_or_unspecified");
    }
    let has_children = match row.get("hasChildren").and_then(Value::as_bool) {
        Some(value) => Some(value),
        None => {
            shape_issues.push("has_children_missing_or_invalid");
            None
        }
    };
    let update_time =
        required_bounded_string(row.get("updateTime"), 64, "update_time", &mut shape_issues);
    let sizes = compact_sizes(row.get("adUnitSizes"), &mut shape_issues);
    let parent_ad_unit_id = match row.get("parentAdUnit") {
        None | Some(Value::Null) => None,
        Some(Value::String(resource_name)) => {
            match exact_ad_unit_resource_id(&target.network_code, resource_name) {
                Some(id) => Some(id),
                None => {
                    shape_issues.push("parent_ad_unit_invalid_or_cross_network");
                    None
                }
            }
        }
        Some(_) => {
            shape_issues.push("parent_ad_unit_invalid_or_cross_network");
            None
        }
    };

    if !identity_matches_request {
        shape_issues.insert(0, "resource_identity_mismatch");
    }
    let shape_complete = shape_issues.is_empty();
    let current = json!({
        "ad_unit_id": resolved_id,
        "ad_unit_code": ad_unit_code,
        "ad_unit_code_source_fingerprint": ad_unit_code_source_fingerprint,
        "status": status,
        "sizes": sizes,
        "has_children": has_children,
        "parent_ad_unit_id": parent_ad_unit_id,
        "update_time": update_time,
    });
    let proof_state = if !identity_matches_request {
        "complete_blocked"
    } else if !shape_complete {
        "not_run"
    } else {
        "complete_clear"
    };
    json!({
        "ad_unit_id": target.ad_unit_id,
        "resource_name": target.resource_name,
        "proof_state": proof_state,
        "identity_matches_request": identity_matches_request,
        "shape_complete": shape_complete,
        "shape_issues": shape_issues,
        "provider_request_state": "completed",
        "identity_fingerprint": stable_fingerprint(&current.to_string()),
        "current": current,
    })
}

pub(super) fn blocked_identity(
    target: &RetirementTarget,
    err: AdManagerError,
    request_attempted: bool,
) -> Value {
    let (proof_state, reason) = match err {
        AdManagerError::AuthBootstrap(_) => (
            "blocked_auth",
            "credentials were not ready, so no provider request was sent",
        ),
        AdManagerError::UpstreamApi {
            status: 401 | 403, ..
        }
        | AdManagerError::WriteScopeRequired { .. } => (
            "blocked_permission",
            "the configured principal could not read this exact ad unit",
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
        "shape_complete": false,
        "shape_issues": [],
        "provider_request_state": if request_attempted { "attempted_no_complete_response" } else { "not_sent" },
        "reason": reason,
    })
}

pub(super) fn summarize_identities(identities: &[Value]) -> Value {
    let has_complete_blocked = identities
        .iter()
        .any(|value| state(value) == "complete_blocked");
    let has_incomplete = identities
        .iter()
        .any(|value| !matches!(state(value), "complete_clear" | "complete_blocked"));
    let proof_state = if has_complete_blocked && has_incomplete {
        "partial_blocked"
    } else if has_complete_blocked {
        "complete_blocked"
    } else if identities
        .iter()
        .any(|value| state(value) == "blocked_auth")
    {
        "blocked_auth"
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

fn required_bounded_string(
    value: Option<&Value>,
    max_chars: usize,
    field: &'static str,
    shape_issues: &mut Vec<&'static str>,
) -> Value {
    let Some(value) = value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        shape_issues.push(match field {
            "ad_unit_code" => "ad_unit_code_missing_or_invalid",
            "status" => "status_missing_or_invalid",
            "update_time" => "update_time_missing_or_invalid",
            _ => "required_string_missing_or_invalid",
        });
        return Value::Null;
    };
    let bounded = value.chars().take(max_chars).collect::<String>();
    if bounded.chars().count() != value.chars().count() {
        shape_issues.push(match field {
            "status" => "status_oversized",
            "update_time" => "update_time_oversized",
            _ => "bounded_string_truncated",
        });
    }
    Value::String(bounded)
}

fn compact_sizes(value: Option<&Value>, shape_issues: &mut Vec<&'static str>) -> Value {
    let Some(entries) = value.and_then(Value::as_array) else {
        shape_issues.push("ad_unit_sizes_missing_or_invalid");
        return json!({
            "source_count": 0,
            "retained_count": 0,
            "truncated": false,
            "source_fingerprint": null,
            "items": []
        });
    };

    let mut items = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(entry) = entry.as_object() else {
            shape_issues.push("ad_unit_size_entry_invalid");
            continue;
        };
        let size = entry.get("size").and_then(Value::as_object);
        let width = size
            .and_then(|size| size.get("width"))
            .and_then(Value::as_u64);
        let height = size
            .and_then(|size| size.get("height"))
            .and_then(Value::as_u64);
        if width.is_none() || height.is_none() {
            shape_issues.push("ad_unit_size_dimensions_invalid");
        }
        let environment_type = match entry.get("environmentType").and_then(Value::as_str) {
            Some(value) if matches!(value, "BROWSER" | "VIDEO_PLAYER") => Some(value),
            Some(_) => {
                shape_issues.push("ad_unit_size_environment_invalid");
                None
            }
            None => {
                shape_issues.push("ad_unit_size_environment_missing");
                None
            }
        };
        let companion_count = match entry.get("companions") {
            None | Some(Value::Null) => 0,
            Some(Value::Array(companions)) => {
                if companions.iter().any(|companion| {
                    let Some(companion) = companion.as_object() else {
                        return true;
                    };
                    companion.get("width").and_then(Value::as_u64).is_none()
                        || companion.get("height").and_then(Value::as_u64).is_none()
                }) {
                    shape_issues.push("ad_unit_size_companions_invalid");
                }
                companions.len()
            }
            Some(_) => {
                shape_issues.push("ad_unit_size_companions_invalid");
                0
            }
        };
        if environment_type != Some("VIDEO_PLAYER") && companion_count > 0 {
            shape_issues.push("ad_unit_size_companions_require_video_player");
        }
        if index < 20 {
            items.push(json!({
                "width": width,
                "height": height,
                "environment_type": environment_type,
                "companion_count": companion_count,
            }));
        }
    }
    json!({
        "source_count": entries.len(),
        "retained_count": items.len(),
        "truncated": entries.len() > 20,
        "source_fingerprint": stable_fingerprint(&Value::Array(entries.clone()).to_string()),
        "items": items,
    })
}

fn exact_ad_unit_resource_id(network_code: &str, resource_name: &str) -> Option<String> {
    let prefix = format!("networks/{network_code}/adUnits/");
    let candidate = resource_name.strip_prefix(&prefix)?;
    if candidate.contains('/') {
        return None;
    }
    validate_canonical_positive_id("resource_name", candidate).ok()
}
