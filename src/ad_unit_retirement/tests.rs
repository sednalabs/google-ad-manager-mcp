use serde_json::json;

use super::inventory::*;
use super::*;

fn args(network_code: &str, ad_unit_ids: &[&str]) -> AdUnitRetirementAssessmentArgs {
    AdUnitRetirementAssessmentArgs {
        network_code: network_code.to_string(),
        ad_unit_ids: ad_unit_ids
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    }
}

#[test]
fn targets_require_canonical_positive_network_and_ids() {
    let targets = validate_targets("1234567", &["200".to_string(), "300".to_string()])
        .expect("canonical targets");
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].resource_name, "networks/1234567/adUnits/200");

    for network_code in ["", "0", "01234567", " 1234567", "1234567 ", "network"] {
        assert!(
            validate_targets(network_code, &["200".to_string()]).is_err(),
            "network `{network_code}` must be rejected"
        );
    }
    for ad_unit_id in [
        "",
        "0",
        "0200",
        " 200",
        "200 ",
        "unit",
        "18446744073709551616",
    ] {
        assert!(
            validate_targets("1234567", &[ad_unit_id.to_string()]).is_err(),
            "ad unit `{ad_unit_id}` must be rejected"
        );
    }
}

#[test]
fn targets_reject_empty_duplicate_and_over_limit_sets() {
    assert!(validate_targets("1234567", &[]).is_err());
    assert!(validate_targets("1234567", &["200".to_string(), "200".to_string()]).is_err());
    assert!(
        validate_targets(
            "1234567",
            &(1..=11).map(|value| value.to_string()).collect::<Vec<_>>(),
        )
        .is_err()
    );
}

#[test]
fn identity_summary_is_compact_exact_and_fingerprinted() {
    let target = RetirementTarget {
        ad_unit_id: "200".to_string(),
        resource_name: "networks/1234567/adUnits/200".to_string(),
    };
    let summary = summarize_identity(
        &target,
        &json!({
            "name": "networks/1234567/adUnits/200",
            "adUnitCode": "fixture_unit",
            "displayName": "not returned",
            "description": "not returned",
            "status": "ACTIVE",
            "adUnitSizes": [
                {"size":{"width":160,"height":600}},
                {"size":{"width":160,"height":600}},
                {"size":{"width":160,"height":1200}}
            ],
            "hasChildren": false,
            "updateTime": "2026-07-10T00:00:00Z"
        }),
    );
    assert_eq!(summary["proof_state"], "complete_clear");
    assert_eq!(summary["identity_matches_request"], true);
    assert_eq!(summary["current"]["sizes"], json!(["160x1200", "160x600"]));
    assert!(
        summary["identity_fingerprint"]
            .as_str()
            .is_some_and(|value| value.len() == 16)
    );
    assert!(summary["current"].get("display_name").is_none());
    assert!(summary["current"].get("description").is_none());
}

#[test]
fn identity_mismatch_and_permission_failure_block_without_leaking_details() {
    let target = RetirementTarget {
        ad_unit_id: "200".to_string(),
        resource_name: "networks/1234567/adUnits/200".to_string(),
    };
    let mismatch = summarize_identity(
        &target,
        &json!({"name":"networks/9999999/adUnits/200","status":"ACTIVE"}),
    );
    assert_eq!(mismatch["proof_state"], "complete_blocked");
    assert_eq!(mismatch["identity_matches_request"], false);

    let permission = blocked_identity(
        &target,
        AdManagerError::UpstreamApi {
            status: 403,
            message: "private provider detail".to_string(),
        },
    );
    assert_eq!(permission["proof_state"], "blocked_permission");
    assert!(!permission.to_string().contains("private provider detail"));
}

#[tokio::test]
async fn successful_preflight_keeps_later_surfaces_not_run() {
    let response = assess_ad_unit_retirement_with_reader(
        &args("1234567", &["200"]),
        |network_code, resource_name| async move {
            assert_eq!(network_code, "1234567");
            assert_eq!(resource_name, "networks/1234567/adUnits/200");
            Ok(json!({
                "name": resource_name,
                "adUnitCode": "fixture_unit",
                "status": "ACTIVE",
                "adUnitSizes": [{"size":{"width":300,"height":250}}],
                "hasChildren": false,
                "updateTime": "2026-07-10T00:00:00Z"
            }))
        },
    )
    .await
    .expect("successful exact-identity preflight");

    assert_eq!(response["identity"]["proof_state"], "complete_clear");
    assert_eq!(response["descendants"]["proof_state"], "not_run");
    assert_eq!(response["evidence"]["proof_state"], "not_run");
    assert_eq!(response["recommendation"]["decision"], "not_run");
    assert_eq!(
        response["recommendation"]["safe_to_archive_or_retire"],
        false
    );
    assert_eq!(response["mutation_performed"], false);
    assert_eq!(
        response["authorization"]["archive_or_deactivate_authorized"],
        false
    );
    assert!(response_bytes(&response) <= MAX_INNER_DATA_BYTES);
}

#[test]
fn response_size_guard_fails_closed() {
    let oversized_identity = json!({"proof_state":"complete_clear","value":"x".repeat(6 * 1024)});
    assert!(
        build_preflight_response(
            "1234567".to_string(),
            vec!["200".to_string()],
            oversized_identity,
        )
        .is_err()
    );
}
