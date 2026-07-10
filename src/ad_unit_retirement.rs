mod decision;
mod descendants;
mod inventory;
mod receipt;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AdManagerError, AdManagerServer};

use decision::recommendation;
use descendants::{blocked_descendants, scan_descendants};
use inventory::{
    blocked_identity, summarize_identities, summarize_identity, validate_network_code,
    validate_targets,
};
use receipt::{RetirementEvidenceReceipt, current_unix_seconds, grade_evidence_bundle};

pub(crate) use receipt::{
    RetirementEvidenceSource, RetirementEvidenceState, evidence_receipt_template,
};

const MAX_RETIREMENT_TARGETS: usize = 10;
const MAX_INNER_DATA_BYTES: usize = 5 * 1024;
pub(crate) const MAX_MODEL_VISIBLE_RESULT_BYTES: usize = 8 * 1024;
pub(crate) const MAX_WIRE_RESULT_BYTES: usize = 20 * 1024;

#[allow(dead_code)]
#[derive(JsonSchema)]
struct RetirementAdUnitIdSchema(
    #[schemars(length(min = 1, max = 20), regex(pattern = r"^[0-9]+$"))] String,
);

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AdUnitRetirementAssessmentArgs {
    /// Raw positive numeric Ad Manager network code, for example 1234567.
    #[schemars(length(min = 1, max = 20), regex(pattern = r"^[1-9][0-9]{0,19}$"))]
    pub network_code: String,
    /// Exact positive numeric ad-unit ids to assess.
    #[schemars(with = "Vec<RetirementAdUnitIdSchema>", length(min = 1, max = 10))]
    pub ad_unit_ids: Vec<String>,
    /// Freshness-bound external proof receipts. Raw reports and telemetry are not accepted.
    #[serde(default)]
    #[schemars(length(max = 5))]
    pub evidence: Vec<RetirementEvidenceReceipt>,
    /// Number of ad-unit rows to fetch per descendant-scan page. Defaults to 1000, max 1000.
    #[schemars(range(min = 1, max = 1000))]
    pub ad_unit_page_size: Option<u32>,
    /// Maximum ad-unit rows to inspect before the descendant proof becomes partial. Defaults to 5000, max 10000.
    #[schemars(range(min = 1, max = 10000))]
    pub max_ad_units: Option<u32>,
}

pub(crate) async fn assess_ad_unit_retirement(
    server: &AdManagerServer,
    args: &AdUnitRetirementAssessmentArgs,
) -> Result<Value, AdManagerError> {
    let network_code = validate_network_code(&args.network_code)?;
    let targets = validate_targets(&network_code, &args.ad_unit_ids)?;
    let page_size = bounded_scan_parameter("ad_unit_page_size", args.ad_unit_page_size, 1_000)?;
    let max_ad_units = bounded_scan_parameter("max_ad_units", args.max_ad_units, 10_000)?;
    let target_ids = targets
        .iter()
        .map(|target| target.ad_unit_id.clone())
        .collect::<Vec<_>>();

    let mut identities = Vec::with_capacity(targets.len());
    for target in &targets {
        let summary = match server
            .client()
            .get_ad_unit(&network_code, &target.resource_name)
            .await
        {
            Ok(row) => summarize_identity(target, &row),
            Err(err) => blocked_identity(target, err),
        };
        identities.push(summary);
    }
    let identity = summarize_identities(&identities);
    let identity_child_claims = identities
        .iter()
        .filter_map(|value| {
            Some((
                value.get("ad_unit_id")?.as_str()?.to_string(),
                value.get("current")?.get("has_children")?.as_bool()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let descendants = match scan_descendants(
        server,
        &network_code,
        &target_ids,
        &identity_child_claims,
        page_size,
        max_ad_units,
    )
    .await
    {
        Ok(summary) => summary,
        Err(err) => blocked_descendants(err),
    };
    let now_unix_seconds = current_unix_seconds()?;
    let evidence =
        grade_evidence_bundle(&args.evidence, &network_code, &target_ids, now_unix_seconds)?;
    build_assessment_response(network_code, target_ids, identity, descendants, evidence)
}

fn build_assessment_response(
    network_code: String,
    target_ids: Vec<String>,
    identity: Value,
    descendants: Value,
    evidence: Value,
) -> Result<Value, AdManagerError> {
    let recommendation = recommendation(&identity, &descendants, &evidence);
    let response = json!({
        "network_code": network_code,
        "target_ad_unit_ids": target_ids,
        "identity": identity,
        "descendants": descendants,
        "evidence": evidence,
        "recommendation": recommendation,
        "mutation_performed": false,
        "authorization": {
            "archive_or_deactivate_authorized": false,
            "reason": "This assessment grades supplied evidence but never verifies operator identity, authorizes, or applies an archive, deactivate, rename, or retarget operation."
        },
        "response_contract": {
            "compact": true,
            "max_inner_data_bytes": MAX_INNER_DATA_BYTES,
            "max_model_visible_result_bytes": MAX_MODEL_VISIBLE_RESULT_BYTES,
            "max_wire_result_bytes": MAX_WIRE_RESULT_BYTES,
        }
    });
    ensure_response_size(&response)?;
    Ok(response)
}

pub(crate) fn response_bytes(response: &Value) -> usize {
    response.to_string().len()
}

fn ensure_response_size(response: &Value) -> Result<(), AdManagerError> {
    let bytes = response_bytes(response);
    if bytes > MAX_INNER_DATA_BYTES {
        return Err(AdManagerError::invalid(
            "assessment_result",
            format!(
                "compact assessment would be {bytes} bytes, above the {MAX_INNER_DATA_BYTES}-byte inner-data cap; assess fewer targets"
            ),
        ));
    }
    Ok(())
}

fn bounded_scan_parameter(
    field: &'static str,
    value: Option<u32>,
    maximum: u32,
) -> Result<u32, AdManagerError> {
    let value = value.unwrap_or(if field == "ad_unit_page_size" {
        1_000
    } else {
        5_000
    });
    if !(1..=maximum).contains(&value) {
        return Err(AdManagerError::invalid(
            field,
            format!("must be between 1 and {maximum}"),
        ));
    }
    Ok(value)
}
