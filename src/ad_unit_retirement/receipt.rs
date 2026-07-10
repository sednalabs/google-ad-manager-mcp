use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AdManagerError;
use crate::fingerprint::stable_fingerprint;

use super::MAX_RETIREMENT_TARGETS;
use super::inventory::{validate_network_code, validate_numeric_ids};

const EVIDENCE_MAX_TTL_SECONDS: u64 = 31 * 24 * 60 * 60;
const EVIDENCE_FUTURE_SKEW_SECONDS: u64 = 5 * 60;
pub(super) const MIN_ACTIVITY_WINDOW_SECONDS: u64 = 30 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetirementEvidenceState {
    CompleteClear,
    CompleteBlocked,
    PartialCapped,
    BlockedPermission,
    BlockedRead,
    UnsupportedSurface,
    ManualUiProofRequired,
    NotRun,
}

impl RetirementEvidenceState {
    fn as_str(self) -> &'static str {
        match self {
            Self::CompleteClear => "complete_clear",
            Self::CompleteBlocked => "complete_blocked",
            Self::PartialCapped => "partial_capped",
            Self::BlockedPermission => "blocked_permission",
            Self::BlockedRead => "blocked_read",
            Self::UnsupportedSurface => "unsupported_surface",
            Self::ManualUiProofRequired => "manual_ui_proof_required",
            Self::NotRun => "not_run",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetirementEvidenceSource {
    DependencyProbe,
    DeliveryReport,
    ExchangeProtectionReview,
    SiteContract,
    Telemetry,
}

impl RetirementEvidenceSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::DependencyProbe => "dependency_probe",
            Self::DeliveryReport => "delivery_report",
            Self::ExchangeProtectionReview => "exchange_protection_review",
            Self::SiteContract => "site_contract",
            Self::Telemetry => "telemetry",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub(crate) struct RetirementEvidenceReceipt {
    /// Network code covered by this evidence receipt.
    pub network_code: String,
    /// Expected evidence source for this proof surface.
    pub source: RetirementEvidenceSource,
    /// Source tool, schema, or contract version that produced the result hash.
    pub source_version: String,
    /// Evidence conclusion before freshness, target binding, and provenance grading.
    pub state: RetirementEvidenceState,
    /// Opaque result hash from the source proof. Raw payloads are not accepted.
    pub result_hash: Option<String>,
    /// Unix epoch seconds when the source proof completed.
    pub observed_at_unix_seconds: Option<u64>,
    /// Maximum age accepted for this proof. Maximum 31 days.
    pub ttl_seconds: Option<u64>,
    /// Exact canonical ad-unit ids covered by the source proof.
    #[serde(default)]
    pub target_ad_unit_ids: Vec<String>,
    /// Optional evidence-window start. Required for delivery-report and telemetry receipts.
    pub window_start_unix_seconds: Option<u64>,
    /// Optional evidence-window end. Required for delivery-report and telemetry receipts.
    pub window_end_unix_seconds: Option<u64>,
    /// Required for exchange/protection complete-clear proof while GAM UI-only surfaces remain unsupported by API.
    #[serde(default)]
    pub manual_ui_proof_included: bool,
    /// Optional bounded, non-sensitive operator note. The note is not echoed.
    pub note: Option<String>,
}

pub(crate) fn evidence_receipt_template(
    network_code: &str,
    source: RetirementEvidenceSource,
    state: RetirementEvidenceState,
    result_hash: &str,
    target_ad_unit_ids: Vec<String>,
) -> Result<Value, AdManagerError> {
    let network_code = validate_network_code(network_code)?;
    let target_ad_unit_ids = validate_numeric_ids(
        "target_ad_unit_ids",
        &target_ad_unit_ids,
        MAX_RETIREMENT_TARGETS,
    )?;
    if !valid_evidence_hash(result_hash) || result_hash != result_hash.trim() {
        return Err(AdManagerError::invalid(
            "result_hash",
            "must be a canonical opaque fingerprint between 16 and 128 characters",
        ));
    }
    Ok(json!({
        "network_code": network_code,
        "source": source.as_str(),
        "source_version": env!("CARGO_PKG_VERSION"),
        "state": state.as_str(),
        "result_hash": result_hash,
        "observed_at_unix_seconds": current_unix_seconds()?,
        "ttl_seconds": 3_600,
        "target_ad_unit_ids": target_ad_unit_ids,
        "provenance": "caller_supplied_unverified",
        "window_start_unix_seconds": null,
        "window_end_unix_seconds": null,
        "manual_ui_proof_included": false,
        "operator_action": "Preserve the exact source result and target scope. The assessor grades this receipt but cannot verify operator identity or authorize retirement."
    }))
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
    if receipts.len() > 5 {
        return Err(AdManagerError::invalid(
            "evidence",
            "must contain at most one receipt for each of the five evidence sources",
        ));
    }
    let receipt_for = |source: RetirementEvidenceSource| -> Result<Option<&RetirementEvidenceReceipt>, AdManagerError> {
        let mut matches = receipts.iter().filter(|receipt| receipt.source == source);
        let first = matches.next();
        if matches.next().is_some() {
            return Err(AdManagerError::invalid(
                "evidence",
                format!("contains duplicate {} receipts", source.as_str()),
            ));
        }
        Ok(first)
    };
    Ok(json!({
        "dependency": grade_evidence("dependency", RetirementEvidenceSource::DependencyProbe, receipt_for(RetirementEvidenceSource::DependencyProbe)?, network_code, target_ids, now, false)?,
        "delivery": grade_evidence("delivery", RetirementEvidenceSource::DeliveryReport, receipt_for(RetirementEvidenceSource::DeliveryReport)?, network_code, target_ids, now, false)?,
        "exchange_protection": grade_evidence("exchange_protection", RetirementEvidenceSource::ExchangeProtectionReview, receipt_for(RetirementEvidenceSource::ExchangeProtectionReview)?, network_code, target_ids, now, true)?,
        "site_contract": grade_evidence("site_contract", RetirementEvidenceSource::SiteContract, receipt_for(RetirementEvidenceSource::SiteContract)?, network_code, target_ids, now, false)?,
        "telemetry": grade_evidence("telemetry", RetirementEvidenceSource::Telemetry, receipt_for(RetirementEvidenceSource::Telemetry)?, network_code, target_ids, now, false)?,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn grade_evidence(
    surface: &str,
    expected_source: RetirementEvidenceSource,
    receipt: Option<&RetirementEvidenceReceipt>,
    network_code: &str,
    target_ids: &[String],
    now: u64,
    manual_ui_required_for_clear: bool,
) -> Result<Value, AdManagerError> {
    let Some(receipt) = receipt else {
        return Ok(json!({
            "surface": surface,
            "state": "not_run",
            "binding_valid": false,
            "complete_for_summary": false,
            "reason": "no evidence receipt was supplied",
        }));
    };
    validate_note(receipt.note.as_deref())?;
    if matches!(receipt.state, RetirementEvidenceState::NotRun) {
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

    let receipt_ids = validate_numeric_ids(
        "evidence",
        &receipt.target_ad_unit_ids,
        MAX_RETIREMENT_TARGETS,
    )?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let expected_ids = target_ids.iter().cloned().collect::<BTreeSet<_>>();
    let canonical_receipt_network = validate_network_code(&receipt.network_code).ok();
    let network_matches = canonical_receipt_network.as_deref() == Some(network_code);
    let source_matches = receipt.source.as_str() == expected_source.as_str();
    let version_valid = valid_source_version(&receipt.source_version)
        && receipt.source_version == receipt.source_version.trim();
    let safe_source_version = version_valid.then(|| receipt.source_version.clone());
    let targets_match = receipt_ids == expected_ids;
    let safe_result_hash = receipt.result_hash.as_deref().and_then(|value| {
        (valid_evidence_hash(value) && value == value.trim()).then(|| value.to_string())
    });
    let hash_valid = safe_result_hash.is_some();
    let ttl_valid = receipt
        .ttl_seconds
        .is_some_and(|ttl| ttl > 0 && ttl <= EVIDENCE_MAX_TTL_SECONDS);
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
        && targets_match
        && hash_valid
        && ttl_valid
        && timestamp_valid
        && window_valid;

    let (state, reason) = if !structurally_complete {
        (
            "invalid_binding",
            "the receipt has a network, source, version, target, hash, timestamp, activity-window, or TTL mismatch",
        )
    } else if stale {
        (
            "stale",
            "the evidence observation or activity-window end exceeded its TTL",
        )
    } else if matches!(receipt.state, RetirementEvidenceState::ManualUiProofRequired) {
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
    } else if manual_ui_required_for_clear
        && matches!(receipt.state, RetirementEvidenceState::CompleteClear)
        && !receipt.manual_ui_proof_included
    {
        (
            "manual_ui_proof_required",
            "API proof alone cannot clear GAM protections, inventory rules, and unified pricing surfaces",
        )
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
            RetirementEvidenceState::CompleteClear | RetirementEvidenceState::CompleteBlocked
        ) || (matches!(
                receipt.state,
                RetirementEvidenceState::ManualUiProofRequired
            ) && receipt.manual_ui_proof_included))
        && !(manual_ui_required_for_clear
            && matches!(receipt.state, RetirementEvidenceState::CompleteClear)
            && !receipt.manual_ui_proof_included);
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
        "network_code": canonical_receipt_network,
        "network_matches": network_matches,
        "source": receipt.source.as_str(),
        "expected_source": expected_source.as_str(),
        "source_matches": source_matches,
        "source_version": safe_source_version,
        "provenance": "caller_supplied_unverified",
        "binding_valid": structurally_complete,
        "complete_for_summary": complete_for_summary,
        "target_binding_complete": targets_match,
        "result_hash": safe_result_hash,
        "hash_valid": hash_valid,
        "observed_at_unix_seconds": observed,
        "window_start_unix_seconds": receipt.window_start_unix_seconds,
        "window_end_unix_seconds": receipt.window_end_unix_seconds,
        "window_valid": window_valid,
        "ttl_seconds": receipt.ttl_seconds,
        "observation_age_seconds": observation_age,
        "window_end_age_seconds": window_end_age,
        "freshness_age_seconds": freshness_age,
        "manual_ui_proof_included": receipt.manual_ui_proof_included,
        "receipt_binding_fingerprint": stable_fingerprint(&receipt_binding.to_string()),
        "reason": reason,
        "note_present": receipt.note.is_some(),
    }))
}

fn validate_note(note: Option<&str>) -> Result<(), AdManagerError> {
    if note.is_some_and(|value| {
        value.len() > 500 || value.chars().any(|ch| ch.is_control() && ch != '\t')
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

fn valid_evidence_window(
    source: RetirementEvidenceSource,
    start: Option<u64>,
    end: Option<u64>,
    observed: Option<u64>,
) -> bool {
    let required = matches!(
        source,
        RetirementEvidenceSource::DeliveryReport | RetirementEvidenceSource::Telemetry
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
