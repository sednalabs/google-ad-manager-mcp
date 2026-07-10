use serde_json::Value;

use crate::evidence::{
    EVIDENCE_PRODUCER_CONTRACT_VERSION, dependency_evidence_state,
    evidence_receipt_target_ids, exchange_evidence_state, validated_receipt_binding,
};

use super::validation::{array, as_object_mut, canonical_id, exact_keys, get, object, text};
use super::{ProbeKind, project, projection_meta};

#[derive(Clone)]
pub(super) struct Receipt {
    pub(super) binds: bool,
    pub(super) source_fingerprint: String,
    pub(super) value: Value,
}

pub(super) fn verify_receipt(kind: ProbeKind, full: &Value) -> Result<Receipt, String> {
    let binds = validated_receipt_binding(full)
        .ok_or_else(|| "source fingerprint or receipt binding was invalid".to_string())?;
    let root = object(full, "probe")?;
    let network = text(root, "network_code", "probe")?;
    if !canonical_id(network) {
        return Err("probe network was not a canonical numeric id".into());
    }
    let fingerprint = text(root, "result_fingerprint", "probe")?.to_string();
    let value = get(root, "evidence_receipt_template", "probe")?.clone();
    let receipt = object(&value, "receipt")?;
    let eligible = evidence_receipt_target_ids(full, network);
    if binds != eligible.is_some() {
        return Err("receipt state disagreed with exact target eligibility".into());
    }
    if binds {
        exact_keys(
            receipt,
            &[
                "network_code",
                "source",
                "source_version",
                "state",
                "result_hash",
                "observed_at_unix_seconds",
                "ttl_seconds",
                "target_ad_unit_ids",
                "provenance",
                "window_start_unix_seconds",
                "window_end_unix_seconds",
                "manual_ui_proof_included",
                "operator_action",
            ],
            "generated receipt",
        )?;
        if text(receipt, "network_code", "receipt")? != network
            || text(receipt, "source", "receipt")? != kind.evidence_source().as_str()
            || text(receipt, "source_version", "receipt")?
                != EVIDENCE_PRODUCER_CONTRACT_VERSION
            || text(receipt, "state", "receipt")? != expected_receipt_state(kind, full)?
            || text(receipt, "result_hash", "receipt")? != fingerprint
        {
            return Err("receipt source, network, state, version, or hash was invalid".into());
        }
        if receipt
            .get("observed_at_unix_seconds")
            .and_then(Value::as_u64)
            .is_none()
            || receipt.get("ttl_seconds").and_then(Value::as_u64) != Some(3_600)
            || receipt.get("provenance").and_then(Value::as_str)
                != Some("caller_supplied_unverified")
            || receipt.get("window_start_unix_seconds") != Some(&Value::Null)
            || receipt.get("window_end_unix_seconds") != Some(&Value::Null)
            || receipt.get("manual_ui_proof_included").and_then(Value::as_bool) != Some(false)
            || receipt
                .get("operator_action")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err("receipt metadata did not match the producer contract".into());
        }
        let actual = array(receipt, "target_ad_unit_ids", "receipt")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "receipt target was not a string".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if Some(actual) != eligible {
            return Err("receipt targets disagreed with exact source targets".into());
        }
    } else {
        exact_keys(
            receipt,
            &["source", "source_version", "state", "reason"],
            "not-generated receipt",
        )?;
        if receipt.get("result_hash").is_some()
            || text(receipt, "source", "not-generated receipt")?
                != kind.evidence_source().as_str()
            || text(receipt, "source_version", "not-generated receipt")?
                != EVIDENCE_PRODUCER_CONTRACT_VERSION
            || receipt.get("state").and_then(Value::as_str) != Some("not_generated")
            || receipt
                .get("reason")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err("not-generated receipt had a hash or invalid state".into());
        }
    }
    Ok(Receipt {
        binds,
        source_fingerprint: fingerprint,
        value,
    })
}

pub(super) fn expected_receipt_state(kind: ProbeKind, full: &Value) -> Result<&'static str, String> {
    match kind {
        ProbeKind::AdUnitDependency => {
            let decision = full
                .get("dependency_decision")
                .and_then(Value::as_str)
                .ok_or_else(|| "dependency decision was missing".to_string())?;
            Ok(dependency_evidence_state(decision, full).as_str())
        }
        ProbeKind::ExchangeProtection => Ok(exchange_evidence_state(full).as_str()),
    }
}

pub(super) fn validate_projection(
    kind: ProbeKind,
    full: &Value,
    compact: &Value,
    receipt: &Receipt,
) -> Result<(), String> {
    let (expected, ledger) = project(kind, full)?;
    let map = object(compact, "compact probe")?;
    if map.get("source_result_fingerprint").and_then(Value::as_str)
        != Some(receipt.source_fingerprint.as_str())
        || map.get("result_projection")
            != Some(&projection_meta(kind, receipt.binds, ledger))
        || validated_receipt_binding(compact) != Some(receipt.binds)
    {
        return Err("compact fingerprint, ledger, or binding claim was invalid".into());
    }
    let mut body = compact.clone();
    let body_map = as_object_mut(&mut body, "compact probe")?;
    for key in [
        "source_result_fingerprint",
        "result_projection",
        "result_fingerprint",
        "evidence_receipt_template",
    ] {
        body_map.remove(key);
    }
    if body != expected {
        return Err("compact authoritative semantics disagreed with source".into());
    }

    let mut source_receipt = receipt.value.clone();
    let mut returned_receipt = get(map, "evidence_receipt_template", "compact probe")?.clone();
    if receipt.binds {
        as_object_mut(&mut source_receipt, "source receipt")?.remove("result_hash");
        as_object_mut(&mut returned_receipt, "returned receipt")?.remove("result_hash");
    }
    if source_receipt != returned_receipt {
        return Err("compact receipt changed a field other than generated result_hash".into());
    }
    Ok(())
}
