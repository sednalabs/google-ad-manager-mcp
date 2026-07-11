mod inventory;

#[cfg(test)]
mod tests;

use std::future::Future;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AdManagerError, fingerprint::stable_fingerprint};

use inventory::{blocked_identity, summarize_identities, summarize_identity, validate_targets};

const MAX_RETIREMENT_TARGETS: usize = 10;
const MAX_INNER_DATA_BYTES: usize = 5 * 1024;

#[allow(dead_code)]
#[derive(JsonSchema)]
struct RetirementAdUnitIdSchema(
    #[schemars(length(min = 1, max = 20), regex(pattern = r"^[1-9][0-9]{0,19}$"))] String,
);

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AdUnitRetirementAssessmentArgs {
    /// Canonical positive numeric Ad Manager network code, for example 1234567.
    #[schemars(length(min = 1, max = 20), regex(pattern = r"^[1-9][0-9]{0,19}$"))]
    pub network_code: String,
    /// One to ten exact canonical positive numeric ad-unit ids to assess.
    #[schemars(with = "Vec<RetirementAdUnitIdSchema>", length(min = 1, max = 10))]
    pub ad_unit_ids: Vec<String>,
}

pub(crate) async fn assess_ad_unit_retirement_with_reader<F, Fut>(
    args: &AdUnitRetirementAssessmentArgs,
    mut read_ad_unit: F,
) -> Result<Value, AdManagerError>
where
    F: FnMut(String, String) -> Fut,
    Fut: Future<Output = Result<Value, AdManagerError>>,
{
    let targets = validate_targets(&args.network_code, &args.ad_unit_ids)?;
    let network_code = args.network_code.clone();
    let target_ids = targets
        .iter()
        .map(|target| target.ad_unit_id.clone())
        .collect::<Vec<_>>();

    let mut identities = Vec::with_capacity(targets.len());
    for target in &targets {
        let summary = match read_ad_unit(network_code.clone(), target.resource_name.clone()).await {
            Ok(row) => summarize_identity(target, &row),
            Err(err) => blocked_identity(target, err),
        };
        identities.push(summary);
    }

    build_preflight_response(network_code, target_ids, summarize_identities(&identities))
}

fn build_preflight_response(
    network_code: String,
    target_ids: Vec<String>,
    identity: Value,
) -> Result<Value, AdManagerError> {
    let response = json!({
        "network_code": network_code,
        "target_ad_unit_ids": target_ids,
        "identity": identity,
        "descendants": not_run_surface(
            "Descendant and hierarchy reconciliation is a later assessment stage."
        ),
        "evidence": not_run_surface(
            "Dependency, delivery, protection, site-contract, and telemetry evidence grading is a later assessment stage."
        ),
        "recommendation": {
            "decision": "not_run",
            "automated_retirement_eligible": false,
            "safe_to_archive_or_retire": false,
            "reason": "Exact current identity is preflight evidence only; it does not establish retirement eligibility."
        },
        "assessment_fingerprint": stable_fingerprint(&identity.to_string()),
        "mutation_performed": false,
        "authorization": {
            "archive_or_deactivate_authorized": false,
            "reason": "This read-only preflight does not authorize or apply an archive, deactivate, rename, or retarget operation."
        },
        "response_contract": {
            "stage": "exact_identity_preflight",
            "compact": true,
            "max_targets": MAX_RETIREMENT_TARGETS,
            "max_inner_data_bytes": MAX_INNER_DATA_BYTES
        }
    });
    ensure_response_size(&response)?;
    Ok(response)
}

fn not_run_surface(reason: &str) -> Value {
    json!({
        "proof_state": "not_run",
        "reason": reason
    })
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
