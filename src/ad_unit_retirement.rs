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
const MAX_INNER_DATA_BYTES: usize = 7 * 1024;

#[allow(dead_code)]
#[derive(JsonSchema)]
struct RetirementAdUnitIdSchema(
    #[schemars(
        length(min = 1, max = 19),
        regex(
            pattern = r"^(?:[1-9][0-9]{0,17}|[1-8][0-9]{18}|9[01][0-9]{17}|92[01][0-9]{16}|922[0-2][0-9]{15}|9223[0-2][0-9]{14}|92233[0-6][0-9]{13}|922337[01][0-9]{12}|92233720[0-2][0-9]{10}|922337203[0-5][0-9]{9}|9223372036[0-7][0-9]{8}|92233720368[0-4][0-9]{7}|922337203685[0-3][0-9]{6}|9223372036854[0-6][0-9]{5}|92233720368547[0-6][0-9]{4}|922337203685477[0-4][0-9]{3}|9223372036854775[0-7][0-9]{2}|922337203685477580[0-7])$"
        )
    )]
    String,
);

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AdUnitRetirementAssessmentArgs {
    /// Canonical positive numeric Ad Manager network code, for example 1234567.
    #[schemars(
        length(min = 1, max = 19),
        regex(
            pattern = r"^(?:[1-9][0-9]{0,17}|[1-8][0-9]{18}|9[01][0-9]{17}|92[01][0-9]{16}|922[0-2][0-9]{15}|9223[0-2][0-9]{14}|92233[0-6][0-9]{13}|922337[01][0-9]{12}|92233720[0-2][0-9]{10}|922337203[0-5][0-9]{9}|9223372036[0-7][0-9]{8}|92233720368[0-4][0-9]{7}|922337203685[0-3][0-9]{6}|9223372036854[0-6][0-9]{5}|92233720368547[0-6][0-9]{4}|922337203685477[0-4][0-9]{3}|9223372036854775[0-7][0-9]{2}|922337203685477580[0-7])$"
        )
    )]
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
    Fut: Future<Output = (Result<Value, AdManagerError>, bool)>,
{
    let targets = validate_targets(&args.network_code, &args.ad_unit_ids)?;
    let network_code = args.network_code.clone();
    let target_ids = targets
        .iter()
        .map(|target| target.ad_unit_id.clone())
        .collect::<Vec<_>>();

    let mut identities = Vec::with_capacity(targets.len());
    let mut request_attempted_count = 0usize;
    for target in &targets {
        let (result, request_attempted) =
            read_ad_unit(network_code.clone(), target.resource_name.clone()).await;
        request_attempted_count += usize::from(request_attempted);
        let summary = match result {
            Ok(row) => summarize_identity(target, &row),
            Err(err) => blocked_identity(target, err, request_attempted),
        };
        identities.push(summary);
    }

    build_preflight_response(
        network_code,
        target_ids,
        summarize_identities(&identities),
        request_attempted_count,
    )
}

fn build_preflight_response(
    network_code: String,
    target_ids: Vec<String>,
    identity: Value,
    request_attempted_count: usize,
) -> Result<Value, AdManagerError> {
    let target_count = target_ids.len();
    let assessment_fingerprint = stable_fingerprint(&identity.to_string());
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
        "assessment_fingerprint": assessment_fingerprint,
        "provider_requests": {
            "target_count": target_count,
            "attempted_count": request_attempted_count,
            "not_sent_count": target_count.saturating_sub(request_attempted_count)
        },
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
