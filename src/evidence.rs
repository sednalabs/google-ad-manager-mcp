//! Neutral evidence-receipt and bounded-result contracts for read-only probes.

use std::collections::BTreeSet;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use mcp_toolkit::rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::{AdManagerError, contract, fingerprint::stable_fingerprint};

pub(crate) const EVIDENCE_PRODUCER_CONTRACT_VERSION: &str = "gam-evidence-producer-v3";
pub(crate) const EVIDENCE_PROVENANCE: &str = "caller_supplied_unverified";
pub(crate) const EVIDENCE_OPERATOR_ACTION: &str = "Preserve the exact producer result and target scope. This caller-supplied receipt does not authorize a GAM mutation.";
pub(crate) const MAX_EVIDENCE_TARGETS: usize = 10;
pub(crate) const MAX_CONTRACT_ENVELOPE_BYTES: usize = 8 * 1024;
pub(crate) const MAX_RMCP_TRANSPORT_BYTES: usize = 20 * 1024;
pub(crate) const DEFAULT_EVIDENCE_TTL_SECONDS: u64 = 3_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceSource {
    DependencyProbe,
    DeliveryReport,
    ExchangeProtectionReview,
    SiteContract,
    Telemetry,
}

impl<'de> Deserialize<'de> for EvidenceSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "dependency_probe" => Ok(Self::DependencyProbe),
            "delivery_report" => Ok(Self::DeliveryReport),
            "exchange_protection_review" => Ok(Self::ExchangeProtectionReview),
            "site_contract" => Ok(Self::SiteContract),
            "telemetry" => Ok(Self::Telemetry),
            _ => Err(serde::de::Error::custom(
                "unsupported retirement evidence source",
            )),
        }
    }
}

impl EvidenceSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DependencyProbe => "dependency_probe",
            Self::DeliveryReport => "delivery_report",
            Self::ExchangeProtectionReview => "exchange_protection_review",
            Self::SiteContract => "site_contract",
            Self::Telemetry => "telemetry",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceState {
    CompleteClear,
    CompleteBlocked,
    PartialBlocked,
    PartialCapped,
    BlockedPermission,
    BlockedRead,
    UnsupportedSurface,
    ManualUiProofRequired,
    NotRun,
}

impl<'de> Deserialize<'de> for EvidenceState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "complete_clear" => Ok(Self::CompleteClear),
            "complete_blocked" => Ok(Self::CompleteBlocked),
            "partial_blocked" => Ok(Self::PartialBlocked),
            "partial_capped" => Ok(Self::PartialCapped),
            "blocked_permission" => Ok(Self::BlockedPermission),
            "blocked_read" => Ok(Self::BlockedRead),
            "unsupported_surface" => Ok(Self::UnsupportedSurface),
            "manual_ui_proof_required" => Ok(Self::ManualUiProofRequired),
            "not_run" => Ok(Self::NotRun),
            _ => Err(serde::de::Error::custom(
                "unsupported retirement evidence state",
            )),
        }
    }
}

impl EvidenceState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CompleteClear => "complete_clear",
            Self::CompleteBlocked => "complete_blocked",
            Self::PartialBlocked => "partial_blocked",
            Self::PartialCapped => "partial_capped",
            Self::BlockedPermission => "blocked_permission",
            Self::BlockedRead => "blocked_read",
            Self::UnsupportedSurface => "unsupported_surface",
            Self::ManualUiProofRequired => "manual_ui_proof_required",
            Self::NotRun => "not_run",
        }
    }
}

pub(crate) fn evidence_receipt_template(
    network_code: &str,
    source: EvidenceSource,
    state: EvidenceState,
    result_hash: &str,
    target_ad_unit_ids: Vec<String>,
) -> Result<Value, AdManagerError> {
    let network_code = validate_canonical_numeric_id("network_code", network_code)?;
    let target_ad_unit_ids = validate_target_ids(&target_ad_unit_ids)?;
    if !valid_result_hash(result_hash) {
        return Err(AdManagerError::invalid(
            "result_hash",
            "must be the exact 16-character lowercase hexadecimal producer fingerprint",
        ));
    }

    Ok(json!({
        "network_code": network_code,
        "source": source.as_str(),
        "source_version": EVIDENCE_PRODUCER_CONTRACT_VERSION,
        "state": state.as_str(),
        "result_hash": result_hash,
        "observed_at_unix_seconds": current_unix_seconds()?,
        "ttl_seconds": DEFAULT_EVIDENCE_TTL_SECONDS,
        "target_ad_unit_ids": target_ad_unit_ids,
        "provenance": EVIDENCE_PROVENANCE,
        "window_start_unix_seconds": null,
        "window_end_unix_seconds": null,
        "manual_ui_proof_included": false,
        "operator_action": EVIDENCE_OPERATOR_ACTION
    }))
}

pub(crate) fn evidence_receipt_target_ids(
    response: &Value,
    network_code: &str,
) -> Option<Vec<String>> {
    validate_canonical_numeric_id("network_code", network_code).ok()?;
    if response
        .get("target_resolution_issues")
        .and_then(Value::as_array)
        .is_some_and(|issues| !issues.is_empty())
    {
        return None;
    }
    let rows = response.get("ad_units")?.as_array()?;
    if rows.is_empty()
        || rows.len() > MAX_EVIDENCE_TARGETS
        || rows.iter().any(|row| !exact_target_row(row, network_code))
    {
        return None;
    }
    let target_ids = rows
        .iter()
        .filter_map(|row| row.get("ad_unit_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    (target_ids.len() == rows.len()).then_some(target_ids)
}

pub(crate) fn exact_ad_unit_id_from_resource_name(
    network_code: &str,
    resource_name: &str,
) -> Option<String> {
    exact_resource_id_from_name(network_code, "adUnits", resource_name)
}

pub(crate) fn exact_resource_id_from_name(
    network_code: &str,
    resource_collection: &str,
    resource_name: &str,
) -> Option<String> {
    validate_canonical_numeric_id("network_code", network_code).ok()?;
    let prefix = format!("networks/{network_code}/{resource_collection}/");
    let raw_id = resource_name.strip_prefix(&prefix)?;
    if raw_id.is_empty() || raw_id.contains('/') {
        return None;
    }
    let canonical_id = validate_canonical_numeric_id("resource_id", raw_id).ok()?;
    (canonical_id == raw_id).then(|| canonical_id.to_string())
}

pub(crate) fn dependency_evidence_state(decision: &str, response: &Value) -> EvidenceState {
    match decision {
        "dependencies_found" => {
            if dependency_receipt_proof_incomplete(response) {
                EvidenceState::PartialBlocked
            } else {
                EvidenceState::CompleteBlocked
            }
        }
        "no_dependencies_observed" => EvidenceState::CompleteClear,
        "incomplete_no_dependencies_observed" | "missing_or_ambiguous_targets" => {
            EvidenceState::PartialCapped
        }
        "blocked" => blocked_evidence_state(response),
        _ => EvidenceState::NotRun,
    }
}

fn dependency_receipt_proof_incomplete(response: &Value) -> bool {
    if response
        .get("target_resolution_issues")
        .and_then(Value::as_array)
        .is_some_and(|issues| !issues.is_empty())
    {
        return true;
    }
    let Some(placements) = response.get("placements") else {
        return true;
    };
    let Some(line_items) = response.get("line_items") else {
        return true;
    };
    dependency_proof_incomplete(placements, line_items)
}

pub(crate) fn dependency_probe_decision(
    target_resolution_issues: &[String],
    placement_summary: &Value,
    line_item_summary: &Value,
) -> &'static str {
    let placement_dependencies = placement_summary
        .get("target_placement_match_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0;
    let line_item_dependencies = line_item_summary
        .get("dependency_match_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0;
    if placement_dependencies || line_item_dependencies {
        "dependencies_found"
    } else if line_item_summary
        .get("proof_state")
        .and_then(Value::as_str)
        .is_some_and(|state| state == "blocked")
        || placement_summary
            .get("proof_state")
            .and_then(Value::as_str)
            .is_some_and(|state| state == "blocked")
    {
        "blocked"
    } else if !target_resolution_issues.is_empty() {
        "missing_or_ambiguous_targets"
    } else if dependency_proof_incomplete(placement_summary, line_item_summary) {
        "incomplete_no_dependencies_observed"
    } else {
        "no_dependencies_observed"
    }
}

fn dependency_proof_incomplete(placement_summary: &Value, line_item_summary: &Value) -> bool {
    placement_summary
        .get("proof_state")
        .and_then(Value::as_str)
        .map(|state| state != "complete_for_page")
        .unwrap_or(true)
        || line_item_summary
            .get("proof_state")
            .and_then(Value::as_str)
            .map(|state| state != "complete")
            .unwrap_or(true)
}

pub(crate) fn exchange_evidence_state(response: &Value) -> EvidenceState {
    let confirmed_target_exposure = response
        .get("ad_units")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|row| row.get("decision").and_then(Value::as_str) == Some("attention_required"))
        || response
            .get("yield_groups")
            .and_then(|value| value.get("decision"))
            .and_then(Value::as_str)
            == Some("targeted_exposed");
    let blocked = [
        "private_auctions",
        "private_auction_deals",
        "yield_groups",
        "rest_discovery",
    ]
    .into_iter()
    .any(|surface| {
        response
            .get(surface)
            .and_then(|value| value.get("proof_state"))
            .and_then(Value::as_str)
            == Some("blocked")
    });
    let yield_group_activity_unknown = exchange_yield_activity_unknown(response);

    let api_complete = response
        .get("certainty")
        .and_then(Value::as_object)
        .is_some_and(|certainty| {
            [
                "can_prove_requested_ad_unit_flags",
                "can_prove_private_auction_absence_or_presence",
                "can_prove_private_deal_absence_or_presence",
                "can_prove_yield_group_targeting",
            ]
            .into_iter()
            .all(|field| certainty.get(field).and_then(Value::as_bool) == Some(true))
        });
    let private_market_attention = ["private_auctions", "private_auction_deals"]
        .into_iter()
        .any(|surface| {
            response
                .get(surface)
                .and_then(|value| value.get("row_count_in_page"))
                .and_then(Value::as_u64)
                .is_some_and(|count| count > 0)
        });
    if confirmed_target_exposure {
        return if api_complete && !yield_group_activity_unknown && !blocked {
            EvidenceState::CompleteBlocked
        } else {
            EvidenceState::PartialBlocked
        };
    }
    if private_market_attention {
        return EvidenceState::PartialBlocked;
    }
    if blocked {
        return blocked_evidence_state(response);
    }
    if yield_group_activity_unknown {
        return EvidenceState::PartialCapped;
    }
    if api_complete {
        EvidenceState::ManualUiProofRequired
    } else {
        EvidenceState::PartialCapped
    }
}

fn exchange_yield_activity_unknown(response: &Value) -> bool {
    response.get("yield_groups").is_some_and(|yield_groups| {
        yield_groups.get("decision").and_then(Value::as_str) == Some("targeted_activity_unknown")
            || yield_groups
                .get("targeted_activity_unknown")
                .and_then(Value::as_array)
                .is_some_and(|matches| !matches.is_empty())
            || yield_groups
                .get("targeting_class_counts")
                .and_then(|counts| counts.get("targeted_activity_unknown"))
                .and_then(Value::as_u64)
                .is_some_and(|count| count > 0)
    })
}

fn exact_target_row(row: &Value, network_code: &str) -> bool {
    let Some(ad_unit_id) = row.get("ad_unit_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(resource_name) = row.get("resource_name").and_then(Value::as_str) else {
        return false;
    };
    row.get("proof_state").and_then(Value::as_str) == Some("resolved_exact")
        && exact_ad_unit_id_from_resource_name(network_code, resource_name).as_deref()
            == Some(ad_unit_id)
}

fn blocked_evidence_state(response: &Value) -> EvidenceState {
    if contains_permission_block(response) {
        EvidenceState::BlockedPermission
    } else {
        EvidenceState::BlockedRead
    }
}

fn contains_permission_block(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            (object.get("proof_state").and_then(Value::as_str) == Some("blocked")
                && object.get("block_class").and_then(Value::as_str) == Some("permission"))
                || object.values().any(contains_permission_block)
        }
        Value::Array(values) => values.iter().any(contains_permission_block),
        _ => false,
    }
}

pub(crate) fn guarded_success(
    data: Value,
    meta: Value,
    started: Instant,
) -> Result<CallToolResult, &'static str> {
    let envelope = contract::success_envelope_with_meta(data, meta, started);
    guard_envelope(envelope)
}

pub(crate) fn guard_envelope(envelope: Value) -> Result<CallToolResult, &'static str> {
    let envelope_bytes = serde_json::to_vec(&envelope)
        .map_err(|_| "tool result could not be serialized for its Contract V1 envelope guard")?;
    if envelope_bytes.len() > MAX_CONTRACT_ENVELOPE_BYTES {
        return Err(
            "tool result exceeded its 8 KiB Contract V1 envelope cap; narrow the target set, reduce page limits, or omit optional raw output",
        );
    }

    let result = CallToolResult::structured(envelope);
    let transport = serde_json::to_vec(&result)
        .map_err(|_| "tool result could not be serialized for its RMCP transport-size guard")?;
    if transport.len() > MAX_RMCP_TRANSPORT_BYTES {
        return Err(
            "tool result exceeded its 20 KiB RMCP transport cap after protocol encoding; narrow the target set, reduce page limits, or omit optional raw output",
        );
    }
    Ok(result)
}

pub(crate) fn validated_receipt_binding(value: &Value) -> Option<bool> {
    let fingerprint = value.get("result_fingerprint")?.as_str()?;
    if !valid_result_hash(fingerprint) {
        return None;
    }
    let mut fingerprint_input = value.clone();
    let object = fingerprint_input.as_object_mut()?;
    object.remove("result_fingerprint");
    object.remove("evidence_receipt_template");
    if stable_fingerprint(&fingerprint_input.to_string()) != fingerprint {
        return None;
    }

    let receipt = value.get("evidence_receipt_template")?.as_object()?;
    match receipt.get("result_hash") {
        Some(Value::String(receipt_hash)) if receipt_hash == fingerprint => Some(true),
        None if receipt.get("state").and_then(Value::as_str) == Some("not_generated") => {
            Some(false)
        }
        _ => None,
    }
}

fn current_unix_seconds() -> Result<u64, AdManagerError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| {
            AdManagerError::invalid(
                "system_time",
                "current system clock is before the Unix epoch",
            )
        })
}

fn validate_target_ids(ids: &[String]) -> Result<Vec<String>, AdManagerError> {
    if ids.is_empty() || ids.len() > MAX_EVIDENCE_TARGETS {
        return Err(AdManagerError::invalid(
            "target_ad_unit_ids",
            format!("must contain one to {MAX_EVIDENCE_TARGETS} exact ad-unit ids"),
        ));
    }

    let mut canonical = BTreeSet::new();
    for id in ids {
        let id = validate_canonical_numeric_id("target_ad_unit_ids", id)?;
        if !canonical.insert(id) {
            return Err(AdManagerError::invalid(
                "target_ad_unit_ids",
                format!("contains duplicate exact ad-unit id `{id}`"),
            ));
        }
    }
    Ok(canonical.into_iter().map(ToOwned::to_owned).collect())
}

fn validate_canonical_numeric_id<'a>(
    field: &'static str,
    value: &'a str,
) -> Result<&'a str, AdManagerError> {
    if value.is_empty() || value.len() > 20 || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(AdManagerError::invalid(
            field,
            "must use a canonical positive numeric identifier of at most 20 digits",
        ));
    }
    if value.starts_with('0') {
        return Err(AdManagerError::invalid(
            field,
            "must use canonical positive numeric form without whitespace or leading zeroes",
        ));
    }
    match value.parse::<u64>() {
        Ok(parsed) if parsed > 0 => Ok(value),
        _ => Err(AdManagerError::invalid(
            field,
            "must be a valid positive 64-bit unsigned integer",
        )),
    }
}

pub(crate) fn valid_result_hash(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde_json::json;

    use crate::fingerprint::stable_fingerprint;

    use super::{
        EVIDENCE_PRODUCER_CONTRACT_VERSION, EvidenceSource, EvidenceState,
        MAX_CONTRACT_ENVELOPE_BYTES, MAX_RMCP_TRANSPORT_BYTES, dependency_evidence_state,
        evidence_receipt_target_ids, evidence_receipt_template, exchange_evidence_state,
        guarded_success, validated_receipt_binding,
    };

    fn finalized_value(mut data: serde_json::Value, binds_receipt: bool) -> serde_json::Value {
        let fingerprint = stable_fingerprint(&data.to_string());
        data["result_fingerprint"] = json!(fingerprint.clone());
        data["evidence_receipt_template"] = if binds_receipt {
            json!({"result_hash": fingerprint})
        } else {
            json!({"state": "not_generated"})
        };
        data
    }

    #[test]
    fn receipt_template_is_versioned_and_canonically_target_bound() {
        let receipt = evidence_receipt_template(
            "1234567",
            EvidenceSource::DependencyProbe,
            EvidenceState::CompleteClear,
            "0123456789abcdef",
            vec!["200".to_string(), "100".to_string()],
        )
        .expect("canonical receipt");

        assert_eq!(receipt["network_code"], "1234567");
        assert_eq!(receipt["source"], "dependency_probe");
        assert_eq!(receipt["source_version"], "gam-evidence-producer-v3");
        assert_eq!(
            receipt["source_version"],
            EVIDENCE_PRODUCER_CONTRACT_VERSION
        );
        assert_eq!(receipt["state"], "complete_clear");
        assert_eq!(receipt["result_hash"], "0123456789abcdef");
        assert_eq!(receipt["target_ad_unit_ids"], json!(["100", "200"]));
        assert_eq!(receipt["provenance"], "caller_supplied_unverified");
    }

    #[test]
    fn receipt_template_rejects_inexact_bindings() {
        let reject = |network_code, result_hash, ids: &[&str]| {
            assert!(
                evidence_receipt_template(
                    network_code,
                    EvidenceSource::DependencyProbe,
                    EvidenceState::CompleteClear,
                    result_hash,
                    ids.iter().map(|id| id.to_string()).collect(),
                )
                .is_err()
            );
        };
        reject("01234567", "0123456789abcdef", &["100"]);
        reject("1234567", " 0123456789abcdef", &["100"]);
        reject("1234567", "0123456789abcdeF", &["100"]);
        reject("1234567", "0123456789abcdef", &["0100"]);
        reject("1234567", "0123456789abcdef", &["18446744073709551616"]);
        reject("1234567", "0123456789abcdef", &["100", "100"]);
        reject("1234567", "0123456789abcdef", &[]);
    }

    #[test]
    fn shared_receipt_scope_requires_exact_canonical_targets() {
        let response = json!({
            "ad_units": [
                {
                    "ad_unit_id": "200",
                    "resource_name": "networks/1234567/adUnits/200",
                    "proof_state": "resolved_exact"
                },
                {
                    "ad_unit_id": "100",
                    "resource_name": "networks/1234567/adUnits/100",
                    "proof_state": "resolved_exact"
                }
            ],
            "target_resolution_issues": []
        });
        assert_eq!(
            evidence_receipt_target_ids(&response, "1234567"),
            Some(vec!["100".to_string(), "200".to_string()])
        );

        let mut inexact = response.clone();
        inexact["ad_units"][0]["resource_name"] = json!("networks/1234567/adUnits/0200");
        assert!(evidence_receipt_target_ids(&inexact, "1234567").is_none());
        assert!(evidence_receipt_target_ids(&response, "01234567").is_none());

        let mut unresolved = response;
        unresolved["target_resolution_issues"] = json!(["ambiguous target"]);
        assert!(evidence_receipt_target_ids(&unresolved, "1234567").is_none());
    }

    #[test]
    fn shared_evidence_state_preserves_block_and_completeness_policy() {
        assert_eq!(
            dependency_evidence_state("dependencies_found", &json!({})),
            EvidenceState::PartialBlocked
        );
        assert_eq!(
            dependency_evidence_state(
                "dependencies_found",
                &json!({
                    "target_resolution_issues": [],
                    "placements": {"proof_state": "complete_for_page"},
                    "line_items": {"proof_state": "complete"}
                })
            ),
            EvidenceState::CompleteBlocked
        );
        assert_eq!(
            dependency_evidence_state(
                "dependencies_found",
                &json!({
                    "target_resolution_issues": [],
                    "placements": {"proof_state": "complete_for_page"},
                    "line_items": {"proof_state": "blocked"}
                })
            ),
            EvidenceState::PartialBlocked
        );
        assert_eq!(
            dependency_evidence_state(
                "blocked",
                &json!({"line_items":{"proof_state":"blocked","block_class":"permission"}})
            ),
            EvidenceState::BlockedPermission
        );
        assert_eq!(
            exchange_evidence_state(&json!({
                "ad_units": [],
                "private_auctions": {"proof_state": "complete_empty"},
                "private_auction_deals": {"proof_state": "complete_empty"},
                "yield_groups": {"proof_state": "complete", "decision": "no_target_matches"},
                "rest_discovery": {"proof_state": "metadata_read"},
                "certainty": {
                    "can_prove_requested_ad_unit_flags": true,
                    "can_prove_private_auction_absence_or_presence": true,
                    "can_prove_private_deal_absence_or_presence": true,
                    "can_prove_yield_group_targeting": true
                }
            })),
            EvidenceState::ManualUiProofRequired
        );

        let partial_private_market = json!({
            "ad_units": [{"decision": "clear_on_exposed_flags"}],
            "private_auctions": {"proof_state": "sample_only", "row_count_in_page": 1},
            "private_auction_deals": {"proof_state": "complete_empty", "row_count_in_page": 0},
            "yield_groups": {"proof_state": "complete", "decision": "no_target_matches"},
            "rest_discovery": {"proof_state": "metadata_read"},
            "certainty": {
                "can_prove_requested_ad_unit_flags": true,
                "can_prove_private_auction_absence_or_presence": false,
                "can_prove_private_deal_absence_or_presence": true,
                "can_prove_yield_group_targeting": true
            }
        });
        assert_eq!(
            exchange_evidence_state(&partial_private_market),
            EvidenceState::PartialBlocked
        );

        let mut complete_target_exposure = json!({
            "ad_units": [{"decision": "attention_required"}],
            "private_auctions": {"proof_state": "complete_empty", "row_count_in_page": 0},
            "private_auction_deals": {"proof_state": "complete_empty", "row_count_in_page": 0},
            "yield_groups": {"proof_state": "complete", "decision": "no_target_matches"},
            "rest_discovery": {"proof_state": "metadata_read"},
            "certainty": {
                "can_prove_requested_ad_unit_flags": true,
                "can_prove_private_auction_absence_or_presence": true,
                "can_prove_private_deal_absence_or_presence": true,
                "can_prove_yield_group_targeting": true
            }
        });
        assert_eq!(
            exchange_evidence_state(&complete_target_exposure),
            EvidenceState::CompleteBlocked
        );

        complete_target_exposure["certainty"]["can_prove_yield_group_targeting"] = json!(false);
        assert_eq!(
            exchange_evidence_state(&complete_target_exposure),
            EvidenceState::PartialBlocked
        );
    }

    #[test]
    fn wire_guard_rejects_oversized_results() {
        for (data, expected) in [
            (
                json!({"optional_raw_output":"x".repeat(MAX_CONTRACT_ENVELOPE_BYTES)}),
                "8 KiB Contract V1 envelope cap",
            ),
            (
                json!({"optional_raw_output":"\"".repeat(3_800)}),
                "20 KiB RMCP transport cap",
            ),
        ] {
            let failure =
                guarded_success(data, json!({"mutation_performed":false}), Instant::now())
                    .expect_err("oversized result must fail its wire guard");
            assert!(failure.contains(expected));
        }
    }

    #[test]
    fn wire_guard_accepts_bounded_results_and_validates_receipt_bindings() {
        let bounded = guarded_success(
            json!({"network_code":"1234567","ad_units":[{"ad_unit_id":"200"}]}),
            json!({"mutation_performed":false}),
            Instant::now(),
        )
        .expect("bounded result");
        assert!(
            serde_json::to_vec(&bounded)
                .expect("serialize bounded result")
                .len()
                < MAX_RMCP_TRANSPORT_BYTES
        );

        let generated = finalized_value(json!({"decision":"clear"}), true);
        assert_eq!(validated_receipt_binding(&generated), Some(true));
        let not_generated = finalized_value(json!({"decision":"partial"}), false);
        assert_eq!(validated_receipt_binding(&not_generated), Some(false));

        let mut mismatched_receipt = generated.clone();
        mismatched_receipt["evidence_receipt_template"]["result_hash"] = json!("fedcba9876543210");
        assert_eq!(validated_receipt_binding(&mismatched_receipt), None);

        let mut false_binding_without_state = not_generated;
        false_binding_without_state["evidence_receipt_template"] = json!({});
        assert_eq!(
            validated_receipt_binding(&false_binding_without_state),
            None
        );

        let mut changed_after_fingerprinting = generated;
        changed_after_fingerprinting["decision"] = json!("changed_after_fingerprinting");
        assert_eq!(
            validated_receipt_binding(&changed_after_fingerprinting),
            None
        );
    }
}
