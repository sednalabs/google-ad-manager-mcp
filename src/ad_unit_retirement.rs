mod descendants;
mod inventory;

#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, future::Future};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AdManagerError, fingerprint::stable_fingerprint};

use descendants::scan_descendants_with_reader;
use inventory::{blocked_identity, summarize_identities, summarize_identity, validate_targets};

pub(crate) use descendants::MAX_DESCENDANT_PAGE_BYTES;

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
    /// Number of ordered ad-unit catalog rows to request per hierarchy page. Defaults to 1000.
    #[schemars(range(min = 1, max = 1000))]
    pub ad_unit_page_size: Option<u32>,
    /// Maximum catalog rows to reconcile before hierarchy proof fails closed. Defaults to 5000.
    #[schemars(range(min = 1, max = 10000))]
    pub max_ad_units: Option<u32>,
}

pub(crate) async fn assess_ad_unit_retirement_with_readers<IF, IFut, LF, LFut>(
    args: &AdUnitRetirementAssessmentArgs,
    mut read_ad_unit: IF,
    read_ad_unit_page: LF,
) -> Result<Value, AdManagerError>
where
    IF: FnMut(String, String) -> IFut,
    IFut: Future<Output = (Result<Value, AdManagerError>, bool)>,
    LF: FnMut(String, u32, Option<String>) -> LFut,
    LFut: Future<Output = (Result<(Value, usize), AdManagerError>, bool)>,
{
    let targets = validate_targets(&args.network_code, &args.ad_unit_ids)?;
    let page_size = bounded_scan_parameter("ad_unit_page_size", args.ad_unit_page_size, 1_000)?;
    let max_ad_units = bounded_scan_parameter("max_ad_units", args.max_ad_units, 10_000)?;
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
    let identity_parent_claims = identities
        .iter()
        .filter_map(|value| {
            let ad_unit_id = value.get("ad_unit_id")?.as_str()?.to_string();
            let parent_id = match value.get("current")?.get("parent_ad_unit_id")? {
                Value::Null => None,
                Value::String(parent_id) => Some(parent_id.clone()),
                _ => return None,
            };
            Some((ad_unit_id, parent_id))
        })
        .collect::<BTreeMap<_, _>>();
    let (descendants, descendant_page_attempted_count) = scan_descendants_with_reader(
        &network_code,
        &target_ids,
        &identity_child_claims,
        &identity_parent_claims,
        page_size,
        max_ad_units,
        read_ad_unit_page,
    )
    .await;

    build_preflight_response(
        network_code,
        target_ids,
        identity,
        descendants,
        ProviderRequestSummary {
            identity_attempted_count: request_attempted_count,
            descendant_page_attempted_count,
        },
        HierarchyScanConfig {
            page_size,
            max_ad_units,
        },
    )
}

struct ProviderRequestSummary {
    identity_attempted_count: usize,
    descendant_page_attempted_count: usize,
}

struct HierarchyScanConfig {
    page_size: u32,
    max_ad_units: u32,
}

fn build_preflight_response(
    network_code: String,
    target_ids: Vec<String>,
    identity: Value,
    descendants: Value,
    provider_requests: ProviderRequestSummary,
    scan_config: HierarchyScanConfig,
) -> Result<Value, AdManagerError> {
    let target_count = target_ids.len();
    let assessment_fingerprint =
        stable_fingerprint(&json!({"identity":&identity,"descendants":&descendants}).to_string());
    let total_request_attempted_count = provider_requests.identity_attempted_count
        + provider_requests.descendant_page_attempted_count;
    let response = json!({
        "network_code": network_code,
        "target_ad_unit_ids": target_ids,
        "identity": identity,
        "descendants": descendants,
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
            "attempted_count": total_request_attempted_count,
            "identity_attempted_count": provider_requests.identity_attempted_count,
            "identity_not_sent_count": target_count.saturating_sub(provider_requests.identity_attempted_count),
            "descendant_page_attempted_count": provider_requests.descendant_page_attempted_count,
        },
        "mutation_performed": false,
        "authorization": {
            "archive_or_deactivate_authorized": false,
            "reason": "This read-only preflight does not authorize or apply an archive, deactivate, rename, or retarget operation."
        },
        "response_contract": {
            "stage": "hierarchy_reconciliation",
            "compact": true,
            "max_targets": MAX_RETIREMENT_TARGETS,
            "max_inner_data_bytes": MAX_INNER_DATA_BYTES,
            "ad_unit_page_size": scan_config.page_size,
            "max_ad_units": scan_config.max_ad_units,
            "max_descendant_page_bytes": MAX_DESCENDANT_PAGE_BYTES,
        }
    });
    ensure_response_size(&response)?;
    Ok(response)
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
