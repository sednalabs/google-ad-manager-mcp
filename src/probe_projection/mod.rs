//! Deterministic, fail-closed projections for oversized probe results.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use mcp_toolkit::rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::AdManagerError;
use crate::contract;
use crate::evidence::{
    EvidenceSource, dependency_probe_decision, guard_error_envelope, guarded_success,
};
use crate::fingerprint::stable_fingerprint;

mod dependency;
mod exchange;
mod ledger;
mod receipt;
mod validation;

use ledger::{Class, Ledger, select};
use receipt::{validate_projection, verify_receipt};
use validation::*;

#[cfg(test)]
use receipt::expected_receipt_state;

const PROJECTION_VERSION: &str = "gam-probe-result-projection-v1";
const ERROR_PREFIX_BYTES: usize = 512;
const FAILURE_BYTES: usize = 384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeKind {
    ExchangeProtection,
    AdUnitDependency,
}

impl ProbeKind {
    const fn name(self) -> &'static str {
        match self {
            Self::ExchangeProtection => "exchange_protection",
            Self::AdUnitDependency => "ad_unit_dependency",
        }
    }

    const fn evidence_source(self) -> EvidenceSource {
        match self {
            Self::ExchangeProtection => EvidenceSource::ExchangeProtectionReview,
            Self::AdUnitDependency => EvidenceSource::DependencyProbe,
        }
    }
}

pub(crate) fn bounded_probe_success(
    kind: ProbeKind,
    full: Value,
    meta: Value,
    started: Instant,
    field: &'static str,
) -> CallToolResult {
    if let Ok(result) = guarded_success(full.clone(), meta.clone(), started) {
        return result;
    }
    let compact = match compact_success(kind, &full, &meta) {
        Ok(value) => value,
        Err(error) => return fail_closed(field, &error, started),
    };
    guarded_success(compact, meta, started)
        .unwrap_or_else(|error| fail_closed(field, error, started))
}

pub(crate) fn bounded_probe_error(
    kind: ProbeKind,
    error: AdManagerError,
    started: Instant,
    field: &'static str,
) -> CallToolResult {
    let full = contract::error_envelope(&error, started);
    if let Ok(result) = guard_error_envelope(full.clone()) {
        return result;
    }
    let compact = match compact_error(kind, &full) {
        Ok(value) => value,
        Err(error) => return fail_closed(field, &error, started),
    };
    guard_error_envelope(compact).unwrap_or_else(|error| fail_closed(field, error, started))
}

fn compact_success(kind: ProbeKind, full: &Value, meta: &Value) -> Result<Value, String> {
    verify_no_mutation(kind, full, meta)?;
    let receipt = verify_receipt(kind, full)?;
    let (mut compact, ledger) = project(kind, full)?;
    let map = as_object_mut(&mut compact, "compact probe")?;
    map.insert(
        "source_result_fingerprint".into(),
        json!(receipt.source_fingerprint.clone()),
    );
    map.insert(
        "result_projection".into(),
        projection_meta(kind, receipt.binds, ledger),
    );
    let fingerprint = stable_fingerprint(&compact.to_string());
    as_object_mut(&mut compact, "compact probe")?
        .insert("result_fingerprint".into(), json!(fingerprint));

    let mut returned_receipt = receipt.value.clone();
    if receipt.binds {
        as_object_mut(&mut returned_receipt, "generated receipt")?
            .insert("result_hash".into(), json!(fingerprint));
    }
    as_object_mut(&mut compact, "compact probe")?
        .insert("evidence_receipt_template".into(), returned_receipt);
    validate_projection(kind, full, &compact, &receipt)?;
    Ok(compact)
}

fn project(kind: ProbeKind, full: &Value) -> Result<(Value, Vec<Value>), String> {
    match kind {
        ProbeKind::ExchangeProtection => exchange::project_exchange(full),
        ProbeKind::AdUnitDependency => dependency::project_dependency(full),
    }
}

fn compact_error(kind: ProbeKind, full: &Value) -> Result<Value, String> {
    let root = object(full, "error envelope")?;
    let error = object(get(root, "error", "error envelope")?, "error")?;
    exact_keys(
        error,
        &["code", "reason", "message", "category", "hint"],
        "error",
    )?;
    let message = text(error, "message", "error")?;
    let prefix = utf8_prefix(message, ERROR_PREFIX_BYTES);
    if prefix.len() == message.len() {
        return Err("oversized error had no truncatable message".into());
    }
    let mut meta = object(get(root, "meta", "error envelope")?, "error meta")?.clone();
    meta.insert(
        "result_projection".into(),
        json!({
            "version": PROJECTION_VERSION,
            "kind": kind.name(),
            "truncated": true,
            "omission_count": 1,
            "omissions": [{
                "path": "/error/message",
                "class": "redacted_message",
                "unit": "utf8_bytes",
                "source_count": message.len(),
                "retained_count": prefix.len(),
                "omitted_count": message.len() - prefix.len(),
                "derived_witness_count": 0,
                "source_value_fingerprint": stable_fingerprint(&Value::String(message.into()).to_string()),
            }],
        }),
    );
    Ok(json!({
        "ok": false,
        "error": {
            "code": error["code"], "reason": error["reason"], "message": prefix,
            "category": error["category"], "hint": error["hint"],
        },
        "meta": meta,
    }))
}

fn projection_meta(kind: ProbeKind, binds: bool, omissions: Vec<Value>) -> Value {
    json!({
        "version": PROJECTION_VERSION,
        "kind": kind.name(),
        "truncated": true,
        "mutation_prohibition_verified": true,
        "receipt_binds_returned_projection": binds,
        "omission_count": omissions.len(),
        "omissions": omissions,
    })
}

fn fail_closed(field: &'static str, error: &str, started: Instant) -> CallToolResult {
    contract::result_contract_error(
        field,
        format!(
            "bounded probe projection failed closed: {}",
            utf8_prefix(error, FAILURE_BYTES)
        ),
        started,
    )
}

#[cfg(test)]
mod tests;
