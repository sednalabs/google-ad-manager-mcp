mod descendants;
mod inventory;
mod receipt;

#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, future::Future};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AdManagerError, fingerprint::stable_fingerprint};

use descendants::{DescendantScanInput, scan_descendants_with_reader, scoped_numeric_id};
use inventory::{blocked_identity, summarize_identities, summarize_identity, validate_targets};
use receipt::{RetirementEvidenceReceipt, current_unix_seconds, grade_evidence_bundle};

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
    /// Freshness-bound external proof receipts. Raw reports and telemetry are not accepted.
    #[serde(default)]
    #[schemars(length(max = 5))]
    pub evidence: Vec<RetirementEvidenceReceipt>,
    /// Number of ordered ad-unit catalog rows to request per hierarchy page. Defaults to 1000.
    #[schemars(range(min = 1, max = 1000))]
    pub ad_unit_page_size: Option<u32>,
    /// Maximum catalog rows to reconcile before hierarchy proof fails closed. Defaults to 5000.
    #[schemars(range(min = 1, max = 10000))]
    pub max_ad_units: Option<u32>,
}

pub(crate) async fn assess_ad_unit_retirement_with_readers<NF, NFut, IF, IFut, LF, LFut>(
    args: &AdUnitRetirementAssessmentArgs,
    mut read_network: NF,
    mut read_ad_unit: IF,
    read_ad_unit_page: LF,
) -> Result<Value, AdManagerError>
where
    NF: FnMut(String) -> NFut,
    NFut: Future<Output = (Result<Value, AdManagerError>, bool)>,
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

    let (network_result, network_request_attempted) = read_network(network_code.clone()).await;
    let effective_root_id = network_result
        .ok()
        .and_then(|row| effective_root_id_from_network(&network_code, &row));
    let (google_root_id, effective_root_request_attempted) = if let Some(effective_root_id) =
        effective_root_id.as_deref()
    {
        let resource_name = format!("networks/{network_code}/adUnits/{effective_root_id}");
        let (result, request_attempted) = read_ad_unit(network_code.clone(), resource_name).await;
        (
            result.ok().and_then(|row| {
                google_root_id_from_effective_root(&network_code, effective_root_id, &row)
            }),
            request_attempted,
        )
    } else {
        (None, false)
    };
    let root_identity = effective_root_id.as_deref().zip(google_root_id.as_deref());

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
        DescendantScanInput {
            target_ids: &target_ids,
            identity_child_claims: &identity_child_claims,
            identity_parent_claims: &identity_parent_claims,
            root_identity,
            max_rows: max_ad_units,
        },
        page_size,
        read_ad_unit_page,
    )
    .await;
    let evidence = grade_evidence_bundle(
        &args.evidence,
        &network_code,
        &target_ids,
        current_unix_seconds()?,
    )?;

    build_preflight_response(
        network_code,
        target_ids,
        identity,
        descendants,
        evidence,
        ProviderRequestSummary {
            network_attempted_count: usize::from(network_request_attempted),
            effective_root_attempted_count: usize::from(effective_root_request_attempted),
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
    network_attempted_count: usize,
    effective_root_attempted_count: usize,
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
    evidence: Value,
    provider_requests: ProviderRequestSummary,
    scan_config: HierarchyScanConfig,
) -> Result<Value, AdManagerError> {
    let target_count = target_ids.len();
    let assessment_fingerprint = stable_fingerprint(
        &json!({"identity":&identity,"descendants":&descendants,"evidence":&evidence}).to_string(),
    );
    let total_request_attempted_count = provider_requests.identity_attempted_count
        + provider_requests.network_attempted_count
        + provider_requests.effective_root_attempted_count
        + provider_requests.descendant_page_attempted_count;
    let response = json!({
        "network_code": network_code,
        "target_ad_unit_ids": target_ids,
        "identity": identity,
        "descendants": descendants,
        "evidence": evidence,
        "recommendation": {
            "decision": "not_run",
            "automated_retirement_eligible": false,
            "safe_to_archive_or_retire": false,
            "reason": "Evidence is graded but the final operator-review recommendation is a later assessment stage."
        },
        "assessment_fingerprint": assessment_fingerprint,
        "provider_requests": {
            "target_count": target_count,
            "attempted_count": total_request_attempted_count,
            "network_attempted_count": provider_requests.network_attempted_count,
            "effective_root_attempted_count": provider_requests.effective_root_attempted_count,
            "identity_attempted_count": provider_requests.identity_attempted_count,
            "identity_not_sent_count": target_count.saturating_sub(provider_requests.identity_attempted_count),
            "descendant_page_attempted_count": provider_requests.descendant_page_attempted_count,
        },
        "mutation_performed": false,
        "authorization": {
            "archive_or_deactivate_authorized": false,
            "reason": "This read-only assessment grades supplied evidence but does not verify operator identity, authorize, or apply an archive, deactivate, rename, or retarget operation."
        },
        "response_contract": {
            "stage": "evidence_grading",
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

fn effective_root_id_from_network(network_code: &str, row: &Value) -> Option<String> {
    let expected_name = format!("networks/{network_code}");
    if row.get("name")?.as_str()? != expected_name
        || row.get("networkCode")?.as_str()? != network_code
    {
        return None;
    }
    scoped_numeric_id(row.get("effectiveRootAdUnit")?.as_str()?, network_code)
}

fn google_root_id_from_effective_root(
    network_code: &str,
    effective_root_id: &str,
    row: &Value,
) -> Option<String> {
    let expected_name = format!("networks/{network_code}/adUnits/{effective_root_id}");
    if row.get("name")?.as_str()? != expected_name {
        return None;
    }
    scoped_numeric_id(row.get("parentAdUnit")?.as_str()?, network_code)
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
