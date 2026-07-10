//! Neutral evidence-receipt and bounded-result contracts for read-only probes.

use std::collections::BTreeSet;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use mcp_toolkit::rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::{AdManagerError, contract, fingerprint::stable_fingerprint};

pub(crate) const EVIDENCE_PRODUCER_CONTRACT_VERSION: &str = "gam-evidence-producer-v1";
pub(crate) const MAX_EVIDENCE_TARGETS: usize = 10;
pub(crate) const MAX_CONTRACT_ENVELOPE_BYTES: usize = 8 * 1024;
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
    let guarded = match compact_data {
        None => guarded_success(data, meta, started).map_err(str::to_string),
        Some(compact) => match guarded_success(data, meta.clone(), started) {
            Ok(result) => Ok(result),
            Err(primary_failure) if !valid_compact_projection(&compact) => Err(format!(
                "primary result: {primary_failure}; compact projection failed its required truncated, fingerprint, and receipt-binding validation"
            )),
            Err(primary_failure) => guarded_success(compact, meta, started).map_err(|failure| {
                format!("primary result: {primary_failure}; compact projection: {failure}")
            }),
        },
    };
    match guarded {
        Ok(result) => result,
        Err(failure) => contract::result_contract_error(field, failure, started),
    }
}

fn guarded_success(
    data: Value,
    meta: Value,
    started: Instant,
) -> Result<CallToolResult, &'static str> {
    let envelope = contract::success_envelope_with_meta(data, meta, started);
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

fn valid_compact_projection(value: &Value) -> bool {
    let truncated = value
        .pointer("/result_projection/truncated")
        .and_then(Value::as_bool)
        == Some(true);
    let binding_claim = value
        .pointer("/result_projection/receipt_binds_returned_projection")
        .and_then(Value::as_bool);
    let fingerprint = value.get("result_fingerprint").and_then(Value::as_str);
    let receipt = value
        .get("evidence_receipt_template")
        .and_then(Value::as_object);
    let (Some(binding_claim), Some(fingerprint), Some(receipt)) =
        (binding_claim, fingerprint, receipt)
    else {
        return false;
    };
    if !truncated || !valid_result_hash(fingerprint) {
        return false;
    }

    let mut fingerprint_input = value.clone();
    let Some(object) = fingerprint_input.as_object_mut() else {
        return false;
    };
    object.remove("result_fingerprint");
    object.remove("evidence_receipt_template");
    if stable_fingerprint(&fingerprint_input.to_string()) != fingerprint {
        return false;
    }

    match (binding_claim, receipt.get("result_hash")) {
        (true, Some(Value::String(receipt_hash))) => receipt_hash == fingerprint,
        (false, None) => true,
        _ => false,
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

fn valid_result_hash(value: &str) -> bool {
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
        MAX_CONTRACT_ENVELOPE_BYTES, MAX_RMCP_TRANSPORT_BYTES, evidence_receipt_template,
        success_with_wire_guard,
    };

    fn render_guard(data: serde_json::Value, compact: Option<serde_json::Value>) -> String {
        let result = success_with_wire_guard(
            data,
            compact,
            json!({"mutation_performed":false}),
            Instant::now(),
            "probe_result",
        );
        let serialized = serde_json::to_vec(&result).expect("serialize guarded result");
        assert!(serialized.len() < MAX_RMCP_TRANSPORT_BYTES);
        String::from_utf8(serialized).expect("UTF-8 result")
    }

    fn compact_projection(mut data: serde_json::Value, binds_receipt: bool) -> serde_json::Value {
        data["result_projection"] = json!({
            "truncated": true,
            "receipt_binds_returned_projection": binds_receipt,
        });
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
            let rendered = render_guard(data, None);
            assert!(rendered.contains("\"ok\":false"));
            assert!(rendered.contains("result_contract_error"));
            assert!(rendered.contains(expected));
        }
    }

    #[test]
    fn wire_guard_uses_bounded_compact_fallback() {
        let bounded = render_guard(
            json!({"network_code":"1234567","ad_units":[{"ad_unit_id":"200"}]}),
            None,
        );
        assert!(bounded.contains("\"ok\":true"));

        let rendered = render_guard(
            json!({"optional_raw_output":"x".repeat(MAX_CONTRACT_ENVELOPE_BYTES)}),
            Some(compact_projection(json!({}), false)),
        );
        assert!(rendered.contains("\"ok\":true"));
        assert!(rendered.contains("\"truncated\":true"));

        let rejected = render_guard(
            json!({"optional_raw_output":"x".repeat(MAX_CONTRACT_ENVELOPE_BYTES)}),
            Some(json!({"result_projection":{"truncated":true}})),
        );
        assert!(rejected.contains("result_contract_error"));
        assert!(rejected.contains("compact projection failed its required"));

        let mut mismatched_receipt = compact_projection(json!({"decision":"clear"}), true);
        mismatched_receipt["evidence_receipt_template"]["result_hash"] = json!("fedcba9876543210");
        let rejected = render_guard(
            json!({"optional_raw_output":"x".repeat(MAX_CONTRACT_ENVELOPE_BYTES)}),
            Some(mismatched_receipt),
        );
        assert!(rejected.contains("result_contract_error"));

        for forged in [
            json!({
                "result_fingerprint":"0123456789abcdef",
                "evidence_receipt_template":{},
                "result_projection":{
                    "truncated":true,
                    "receipt_binds_returned_projection":true
                }
            }),
            compact_projection(json!({"decision":"clear"}), true),
        ] {
            let mut forged = forged;
            forged["decision"] = json!("changed_after_fingerprinting");
            let rejected = render_guard(
                json!({"optional_raw_output":"x".repeat(MAX_CONTRACT_ENVELOPE_BYTES)}),
                Some(forged),
            );
            assert!(rejected.contains("result_contract_error"));
        }
    }
}
