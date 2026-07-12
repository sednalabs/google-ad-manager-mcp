use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evidence::{
    DEFAULT_EVIDENCE_TTL_SECONDS, EVIDENCE_OPERATOR_ACTION, EVIDENCE_PRODUCER_CONTRACT_VERSION,
    EVIDENCE_PROVENANCE, EvidenceSource, EvidenceState,
    valid_result_hash as valid_producer_result_hash,
};
use crate::{AdManagerError, fingerprint::stable_fingerprint};

use super::inventory::validate_canonical_positive_id;
use super::{MAX_RETIREMENT_TARGETS, RetirementAdUnitIdSchema};

const EVIDENCE_MAX_TTL_SECONDS: u64 = 31 * 24 * 60 * 60;
const EVIDENCE_FUTURE_SKEW_SECONDS: u64 = 5 * 60;
pub(super) const MIN_ACTIVITY_WINDOW_SECONDS: u64 = 30 * 24 * 60 * 60;

#[allow(dead_code)]
#[derive(JsonSchema)]
struct RetirementEvidenceHashSchema(
    #[schemars(length(min = 16, max = 128), regex(pattern = r"^[A-Za-z0-9:_.-]+$"))] String,
);

#[allow(dead_code)]
#[derive(JsonSchema)]
struct RetirementEvidenceNoteSchema(#[schemars(length(max = 500))] String);

#[allow(dead_code)]
#[derive(JsonSchema)]
struct RetirementEvidenceProvenanceSchema(
    #[schemars(length(min = 1, max = 64), regex(pattern = r"^[a-z_]+$"))] String,
);

#[allow(dead_code)]
#[derive(JsonSchema)]
struct RetirementEvidenceActionSchema(#[schemars(length(min = 1, max = 256))] String);

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetirementEvidenceReceipt {
    /// Network code covered by this evidence receipt.
    #[schemars(
        length(min = 1, max = 19),
        regex(
            pattern = r"^(?:[1-9][0-9]{0,17}|[1-8][0-9]{18}|9[01][0-9]{17}|92[01][0-9]{16}|922[0-2][0-9]{15}|9223[0-2][0-9]{14}|92233[0-6][0-9]{13}|922337[01][0-9]{12}|92233720[0-2][0-9]{10}|922337203[0-5][0-9]{9}|9223372036[0-7][0-9]{8}|92233720368[0-4][0-9]{7}|922337203685[0-3][0-9]{6}|9223372036854[0-6][0-9]{5}|92233720368547[0-6][0-9]{4}|922337203685477[0-4][0-9]{3}|9223372036854775[0-7][0-9]{2}|922337203685477580[0-7])$"
        )
    )]
    pub network_code: String,
    /// Expected evidence source for this proof surface.
    pub source: EvidenceSource,
    /// Source tool, schema, or contract version that produced the result hash.
    #[schemars(length(min = 1, max = 64), regex(pattern = r"^[A-Za-z0-9._+-]+$"))]
    pub source_version: String,
    /// Evidence conclusion before freshness, target binding, and provenance grading.
    pub state: EvidenceState,
    /// Opaque result hash from the source proof. Raw payloads are not accepted.
    #[schemars(with = "Option<RetirementEvidenceHashSchema>")]
    pub result_hash: Option<String>,
    /// Unix epoch seconds when the source proof completed.
    pub observed_at_unix_seconds: Option<u64>,
    /// Maximum age accepted for this proof. Maximum 31 days.
    #[schemars(range(min = 1, max = 2678400))]
    pub ttl_seconds: Option<u64>,
    /// Exact canonical ad-unit ids covered by the source proof.
    #[schemars(with = "Vec<RetirementAdUnitIdSchema>", length(min = 1, max = 10))]
    pub target_ad_unit_ids: Vec<String>,
    /// Optional evidence-window start. Required for delivery-report and telemetry receipts.
    pub window_start_unix_seconds: Option<u64>,
    /// Optional evidence-window end. Required for delivery-report and telemetry receipts.
    pub window_end_unix_seconds: Option<u64>,
    /// Required for exchange/protection complete-clear proof while GAM UI-only surfaces remain unsupported by API.
    #[serde(default)]
    pub manual_ui_proof_included: bool,
    /// Optional bounded, non-sensitive operator note. The note is never echoed.
    #[schemars(with = "Option<RetirementEvidenceNoteSchema>")]
    pub note: Option<String>,
    /// Fixed provenance marker emitted by built-in evidence producers.
    #[schemars(with = "Option<RetirementEvidenceProvenanceSchema>")]
    pub provenance: Option<String>,
    /// Fixed non-authorisation guidance emitted by built-in evidence producers.
    #[schemars(with = "Option<RetirementEvidenceActionSchema>")]
    pub operator_action: Option<String>,
}

pub(super) fn current_unix_seconds() -> Result<u64, AdManagerError> {
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

pub(super) fn grade_evidence_bundle(
    receipts: &[RetirementEvidenceReceipt],
    network_code: &str,
    target_ids: &[String],
    now: u64,
) -> Result<Value, AdManagerError> {
    validate_evidence_bundle_structure(receipts)?;
    let receipt_for =
        |source: EvidenceSource| receipts.iter().find(|receipt| receipt.source == source);
    Ok(json!({
        "dependency": compact_complete_evidence(grade_evidence("dependency", EvidenceSource::DependencyProbe, receipt_for(EvidenceSource::DependencyProbe), network_code, target_ids, now, false)?),
        "delivery": compact_complete_evidence(grade_evidence("delivery", EvidenceSource::DeliveryReport, receipt_for(EvidenceSource::DeliveryReport), network_code, target_ids, now, false)?),
        "exchange_protection": compact_complete_evidence(grade_evidence("exchange_protection", EvidenceSource::ExchangeProtectionReview, receipt_for(EvidenceSource::ExchangeProtectionReview), network_code, target_ids, now, true)?),
        "site_contract": compact_complete_evidence(grade_evidence("site_contract", EvidenceSource::SiteContract, receipt_for(EvidenceSource::SiteContract), network_code, target_ids, now, false)?),
        "telemetry": compact_complete_evidence(grade_evidence("telemetry", EvidenceSource::Telemetry, receipt_for(EvidenceSource::Telemetry), network_code, target_ids, now, false)?),
    }))
}

fn compact_complete_evidence(evidence: Value) -> Value {
    if !matches!(
        evidence.get("state").and_then(Value::as_str),
        Some("complete_clear" | "complete_blocked")
    ) {
        return evidence;
    }
    let manual_ui_proof_included = evidence
        .get("manual_ui_proof_included")
        .and_then(Value::as_bool)
        .is_some_and(|included| included);
    let mut compact = json!({
        "state": evidence.get("state").cloned().unwrap_or(Value::Null),
        "binding_valid": evidence.get("binding_valid").cloned().unwrap_or(Value::Null),
        "complete_for_summary": evidence.get("complete_for_summary").cloned().unwrap_or(Value::Null),
        "freshness_age_seconds": evidence.get("freshness_age_seconds").cloned().unwrap_or(Value::Null),
        "receipt_binding_fingerprint": evidence.get("receipt_binding_fingerprint").cloned().unwrap_or(Value::Null),
    });
    if manual_ui_proof_included {
        compact["manual_ui_proof_included"] = Value::Bool(true);
    }
    compact
}

pub(super) fn validate_evidence_bundle_structure(
    receipts: &[RetirementEvidenceReceipt],
) -> Result<(), AdManagerError> {
    if receipts.len() > 5 {
        return Err(AdManagerError::invalid(
            "evidence",
            "must contain at most one receipt for each of the five evidence sources",
        ));
    }
    let mut sources = BTreeSet::new();
    for receipt in receipts {
        validate_note(receipt.note.as_deref())?;
        if !sources.insert(receipt.source.as_str()) {
            return Err(AdManagerError::invalid(
                "evidence",
                format!("contains duplicate {} receipts", receipt.source.as_str()),
            ));
        }
    }
    Ok(())
}

pub(super) fn grade_evidence(
    surface: &str,
    expected_source: EvidenceSource,
    receipt: Option<&RetirementEvidenceReceipt>,
    network_code: &str,
    target_ids: &[String],
    now: u64,
    manual_ui_required_for_clear: bool,
) -> Result<Value, AdManagerError> {
    let Some(receipt) = receipt else {
        return Ok(json!({
            "state": "not_run",
        }));
    };
    if matches!(receipt.state, EvidenceState::NotRun) {
        return Ok(json!({
            "surface": surface,
            "state": "not_run",
            "input_state": receipt.state.as_str(),
            "source": receipt.source.as_str(),
            "provenance": "caller_supplied_unverified",
            "binding_valid": false,
            "complete_for_summary": false,
            "reason": "the source proof was explicitly marked not run",
        }));
    }

    let receipt_ids = canonical_id_set(&receipt.target_ad_unit_ids);
    let expected_ids = target_ids.iter().cloned().collect::<BTreeSet<_>>();
    let canonical_receipt_network =
        validate_canonical_positive_id("evidence_network_code", &receipt.network_code).ok();
    let network_matches = canonical_receipt_network.as_deref() == Some(network_code);
    let source_matches = receipt.source == expected_source;
    let version_valid = valid_source_version(&receipt.source_version)
        && receipt.source_version == receipt.source_version.trim()
        && supported_source_version(receipt.source, &receipt.source_version);
    let safe_source_version = version_valid.then(|| receipt.source_version.clone());
    let targets_match = receipt_ids.as_ref() == Some(&expected_ids);
    let producer_contract = matches!(
        receipt.source,
        EvidenceSource::DependencyProbe | EvidenceSource::ExchangeProtectionReview
    );
    let safe_result_hash = receipt.result_hash.as_deref().and_then(|value| {
        let valid = if producer_contract {
            valid_producer_result_hash(value)
        } else {
            valid_evidence_hash(value) && value == value.trim()
        };
        valid.then(|| value.to_string())
    });
    let hash_valid = safe_result_hash.is_some();
    let ttl_valid = if producer_contract {
        receipt.ttl_seconds == Some(DEFAULT_EVIDENCE_TTL_SECONDS)
    } else {
        receipt
            .ttl_seconds
            .is_some_and(|ttl| ttl > 0 && ttl <= EVIDENCE_MAX_TTL_SECONDS)
    };
    let state_valid = valid_source_state(receipt.source, receipt.state);
    let producer_metadata_valid = if producer_contract {
        receipt.provenance.as_deref() == Some(EVIDENCE_PROVENANCE)
            && receipt.operator_action.as_deref() == Some(EVIDENCE_OPERATOR_ACTION)
    } else {
        receipt.provenance.is_none() && receipt.operator_action.is_none()
    };
    let observed = receipt.observed_at_unix_seconds;
    let timestamp_valid =
        observed.is_some_and(|value| value <= now.saturating_add(EVIDENCE_FUTURE_SKEW_SECONDS));
    let window_valid = valid_evidence_window(
        expected_source,
        receipt.window_start_unix_seconds,
        receipt.window_end_unix_seconds,
        observed,
    );
    let observation_age = observed.map(|value| now.saturating_sub(value));
    let window_end_age = receipt
        .window_end_unix_seconds
        .map(|value| now.saturating_sub(value));
    let freshness_age = observation_age.into_iter().chain(window_end_age).max();
    let stale = freshness_age
        .zip(receipt.ttl_seconds)
        .is_some_and(|(age, ttl)| ttl > 0 && age > ttl);
    let structurally_complete = network_matches
        && source_matches
        && version_valid
        && state_valid
        && producer_metadata_valid
        && targets_match
        && hash_valid
        && ttl_valid
        && timestamp_valid
        && window_valid;
    let mut binding_errors = Vec::new();
    for (valid, label) in [
        (network_matches, "network"),
        (source_matches, "source"),
        (version_valid, "source_version"),
        (state_valid, "source_state"),
        (producer_metadata_valid, "producer_metadata"),
        (targets_match, "targets"),
        (hash_valid, "result_hash"),
        (ttl_valid, "ttl"),
        (timestamp_valid, "observed_at"),
        (window_valid, "activity_window"),
    ] {
        if !valid {
            binding_errors.push(label);
        }
    }

    let (state, reason) = if !structurally_complete {
        (
            "invalid_binding",
            "the receipt has a network, source, version, state, producer-metadata, target, hash, timestamp, activity-window, or TTL mismatch",
        )
    } else if stale {
        (
            "stale",
            "the evidence observation or activity-window end exceeded its TTL",
        )
    } else if manual_ui_required_for_clear
        && matches!(receipt.state, EvidenceState::ManualUiProofRequired)
    {
        if receipt.manual_ui_proof_included {
            (
                "complete_clear",
                "the caller recorded completion of the required GAM UI review; operator identity remains unverified",
            )
        } else {
            (
                "manual_ui_proof_required",
                "the required GAM UI-only protection review has not been recorded",
            )
        }
    } else {
        (
            receipt.state.as_str(),
            "the caller-supplied receipt is structurally source-bound, exact-target bound, and within its TTL; operator identity is not verified",
        )
    };
    let complete_for_summary = structurally_complete
        && !stale
        && (matches!(
            receipt.state,
            EvidenceState::CompleteClear | EvidenceState::CompleteBlocked
        ) || (manual_ui_required_for_clear
            && matches!(receipt.state, EvidenceState::ManualUiProofRequired)
            && receipt.manual_ui_proof_included));
    let receipt_binding = json!({
        "network_code": canonical_receipt_network,
        "source": receipt.source.as_str(),
        "state": receipt.state.as_str(),
        "source_version": safe_source_version,
        "result_hash": safe_result_hash,
        "target_ad_unit_ids": receipt_ids,
        "observed_at_unix_seconds": observed,
        "ttl_seconds": receipt.ttl_seconds,
        "window_start_unix_seconds": receipt.window_start_unix_seconds,
        "window_end_unix_seconds": receipt.window_end_unix_seconds,
        "manual_ui_proof_included": receipt.manual_ui_proof_included,
    });
    Ok(json!({
        "surface": surface,
        "state": state,
        "input_state": receipt.state.as_str(),
        "provenance": "caller_supplied_unverified",
        "binding_valid": structurally_complete,
        "binding_errors": binding_errors,
        "complete_for_summary": complete_for_summary,
        "freshness_age_seconds": freshness_age,
        "manual_ui_proof_included": receipt.manual_ui_proof_included,
        "receipt_binding_fingerprint": stable_fingerprint(&receipt_binding.to_string()),
        "reason": reason,
        "note_present": receipt.note.is_some(),
    }))
}

fn canonical_id_set(ids: &[String]) -> Option<BTreeSet<String>> {
    if ids.is_empty() || ids.len() > MAX_RETIREMENT_TARGETS {
        return None;
    }
    let mut canonical = BTreeSet::new();
    for id in ids {
        let id = validate_canonical_positive_id("evidence_target_ad_unit_ids", id).ok()?;
        if !canonical.insert(id) {
            return None;
        }
    }
    Some(canonical)
}

pub(super) fn validate_note(note: Option<&str>) -> Result<(), AdManagerError> {
    if note.is_some_and(|value| {
        value.chars().count() > 500 || value.chars().any(|ch| ch.is_control() && ch != '\t')
    }) {
        return Err(AdManagerError::invalid(
            "evidence",
            "evidence notes must be at most 500 characters and contain no line or control characters",
        ));
    }
    Ok(())
}

fn valid_source_version(value: &str) -> bool {
    let trimmed = value.trim();
    (1..=64).contains(&trimmed.len())
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+'))
}

fn supported_source_version(source: EvidenceSource, value: &str) -> bool {
    match source {
        EvidenceSource::DependencyProbe | EvidenceSource::ExchangeProtectionReview => {
            value == EVIDENCE_PRODUCER_CONTRACT_VERSION
        }
        EvidenceSource::DeliveryReport => value == "gam-report-v1",
        EvidenceSource::SiteContract => value == "site-contract-v1",
        EvidenceSource::Telemetry => value == "telemetry-v1",
    }
}

fn valid_source_state(source: EvidenceSource, state: EvidenceState) -> bool {
    match source {
        EvidenceSource::DependencyProbe => matches!(
            state,
            EvidenceState::CompleteClear
                | EvidenceState::CompleteBlocked
                | EvidenceState::PartialBlocked
                | EvidenceState::PartialCapped
                | EvidenceState::BlockedPermission
                | EvidenceState::BlockedRead
                | EvidenceState::NotRun
        ),
        EvidenceSource::ExchangeProtectionReview => matches!(
            state,
            EvidenceState::CompleteBlocked
                | EvidenceState::PartialBlocked
                | EvidenceState::PartialCapped
                | EvidenceState::BlockedPermission
                | EvidenceState::BlockedRead
                | EvidenceState::ManualUiProofRequired
                | EvidenceState::NotRun
        ),
        EvidenceSource::DeliveryReport
        | EvidenceSource::SiteContract
        | EvidenceSource::Telemetry => !matches!(state, EvidenceState::ManualUiProofRequired),
    }
}

fn valid_evidence_window(
    source: EvidenceSource,
    start: Option<u64>,
    end: Option<u64>,
    observed: Option<u64>,
) -> bool {
    let required = matches!(
        source,
        EvidenceSource::DeliveryReport | EvidenceSource::Telemetry
    );
    match (start, end, observed) {
        (Some(start), Some(end), Some(observed)) => {
            start < end
                && end <= observed
                && end.saturating_sub(start) >= MIN_ACTIVITY_WINDOW_SECONDS
        }
        (None, None, _) => !required,
        _ => false,
    }
}

fn valid_evidence_hash(value: &str) -> bool {
    let trimmed = value.trim();
    (16..=128).contains(&trimmed.len())
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-' | '.'))
}
