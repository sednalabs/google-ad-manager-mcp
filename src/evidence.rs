use std::collections::BTreeSet;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use mcp_toolkit::rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::{AdManagerError, contract};

pub(crate) const EVIDENCE_PRODUCER_CONTRACT_VERSION: &str = "gam-evidence-producer-v1";
pub(crate) const MAX_EVIDENCE_TARGETS: usize = 10;
pub(crate) const MAX_MODEL_VISIBLE_RESULT_BYTES: usize = 8 * 1024;
pub(crate) const MAX_RMCP_TRANSPORT_BYTES: usize = 20 * 1024;
const DEFAULT_EVIDENCE_TTL_SECONDS: u64 = 3_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvidenceSource {
    DependencyProbe,
    ExchangeProtectionReview,
}

impl EvidenceSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DependencyProbe => "dependency_probe",
            Self::ExchangeProtectionReview => "exchange_protection_review",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvidenceState {
    CompleteClear,
    CompleteBlocked,
    PartialCapped,
    BlockedPermission,
    BlockedRead,
    ManualUiProofRequired,
    NotRun,
}

impl EvidenceState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CompleteClear => "complete_clear",
            Self::CompleteBlocked => "complete_blocked",
            Self::PartialCapped => "partial_capped",
            Self::BlockedPermission => "blocked_permission",
            Self::BlockedRead => "blocked_read",
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
        "provenance": "caller_supplied_unverified",
        "window_start_unix_seconds": null,
        "window_end_unix_seconds": null,
        "manual_ui_proof_included": false,
        "operator_action": "Preserve the exact producer result and target scope. This caller-supplied receipt does not authorize a GAM mutation."
    }))
}

pub(crate) fn success_with_wire_guard(
    data: Value,
    compact_data: Option<Value>,
    meta: Value,
    started: Instant,
    field: &'static str,
) -> CallToolResult {
    match guarded_success(data, meta.clone(), started) {
        Ok(result) => result,
        Err(primary_failure) => match compact_data {
            Some(compact) => match guarded_success(compact, meta, started) {
                Ok(result) => result,
                Err(failure) => contract::error(AdManagerError::invalid(field, failure), started),
            },
            None => contract::error(AdManagerError::invalid(field, primary_failure), started),
        },
    }
}

fn guarded_success(
    data: Value,
    meta: Value,
    started: Instant,
) -> Result<CallToolResult, &'static str> {
    let envelope = contract::success_envelope_with_meta(data, meta, started);
    let model_visible = serde_json::to_vec(&envelope)
        .map_err(|_| "tool result could not be serialized for its model-visible size guard")?;
    if model_visible.len() > MAX_MODEL_VISIBLE_RESULT_BYTES {
        return Err(
            "tool result exceeded its 8 KiB model-visible Contract V1 cap; narrow the target set, reduce page limits, or omit optional raw output",
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
        if !canonical.insert(id.clone()) {
            return Err(AdManagerError::invalid(
                "target_ad_unit_ids",
                format!("contains duplicate exact ad-unit id `{id}`"),
            ));
        }
    }
    Ok(canonical.into_iter().collect())
}

fn validate_canonical_numeric_id(
    field: &'static str,
    value: &str,
) -> Result<String, AdManagerError> {
    if value.is_empty() || value.len() > 20 || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(AdManagerError::invalid(
            field,
            "must use a canonical positive numeric identifier of at most 20 digits",
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
                "must use a canonical positive numeric identifier of at most 20 digits",
            )
        })?;
    if canonical != value {
        return Err(AdManagerError::invalid(
            field,
            "must use canonical numeric form without whitespace or leading zeroes",
        ));
    }
    Ok(canonical)
}

fn valid_result_hash(value: &str) -> bool {
    value.len() == 16
        && value.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_hexdigit() && byte.is_ascii_lowercase())
        })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde_json::json;

    use super::{
        EVIDENCE_PRODUCER_CONTRACT_VERSION, EvidenceSource, EvidenceState,
        MAX_MODEL_VISIBLE_RESULT_BYTES, MAX_RMCP_TRANSPORT_BYTES, evidence_receipt_template,
        success_with_wire_guard,
    };

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
        for (network_code, result_hash, ids) in [
            ("01234567", "0123456789abcdef", vec!["100".to_string()]),
            ("1234567", " 0123456789abcdef", vec!["100".to_string()]),
            ("1234567", "0123456789abcdeF", vec!["100".to_string()]),
            ("1234567", "0123456789abcdef", vec!["0100".to_string()]),
            (
                "1234567",
                "0123456789abcdef",
                vec!["100".to_string(), "100".to_string()],
            ),
            ("1234567", "0123456789abcdef", Vec::new()),
        ] {
            assert!(
                evidence_receipt_template(
                    network_code,
                    EvidenceSource::DependencyProbe,
                    EvidenceState::CompleteClear,
                    result_hash,
                    ids,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn wire_guard_rejects_oversized_model_visible_contract() {
        let result = success_with_wire_guard(
            json!({"optional_raw_output":"x".repeat(MAX_MODEL_VISIBLE_RESULT_BYTES)}),
            None,
            json!({"mutation_performed":false}),
            Instant::now(),
            "probe_result",
        );
        let serialized = serde_json::to_vec(&result).expect("serialize guarded result");
        assert!(serialized.len() < MAX_RMCP_TRANSPORT_BYTES);
        let rendered = String::from_utf8(serialized).expect("UTF-8 result");
        assert!(rendered.contains("\"ok\":false"));
        assert!(rendered.contains("invalid_input"));
        assert!(rendered.contains("8 KiB model-visible Contract V1 cap"));
    }

    #[test]
    fn wire_guard_uses_bounded_compact_fallback() {
        let bounded = success_with_wire_guard(
            json!({"network_code":"1234567","ad_units":[{"ad_unit_id":"200"}]}),
            None,
            json!({"mutation_performed":false}),
            Instant::now(),
            "probe_result",
        );
        assert!(serde_json::to_string(&bounded).unwrap().contains("\"ok\":true"));

        let result = success_with_wire_guard(
            json!({"optional_raw_output":"x".repeat(MAX_MODEL_VISIBLE_RESULT_BYTES)}),
            Some(json!({"result_projection":{"truncated":true}})),
            json!({"mutation_performed":false}),
            Instant::now(),
            "probe_result",
        );
        let serialized = serde_json::to_string(&result).expect("serialize guarded result");
        assert!(serialized.len() < MAX_RMCP_TRANSPORT_BYTES);
        assert!(serialized.contains("\"ok\":true"));
        assert!(serialized.contains("\"truncated\":true"));
    }

    #[test]
    fn wire_guard_rejects_oversized_rmcp_transport() {
        let result = success_with_wire_guard(
            json!({"optional_raw_output":"\"".repeat(3_800)}),
            None,
            json!({"mutation_performed":false}),
            Instant::now(),
            "probe_result",
        );
        let serialized = serde_json::to_vec(&result).expect("serialize guarded result");
        assert!(serialized.len() < MAX_RMCP_TRANSPORT_BYTES);
        let rendered = String::from_utf8(serialized).expect("UTF-8 result");
        assert!(rendered.contains("\"ok\":false"));
        assert!(rendered.contains("20 KiB RMCP transport cap"));
    }
}
