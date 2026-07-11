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
        "9223372036854775808",
        "18446744073709551616",
    ] {
        assert!(
            validate_targets("1234567", &[ad_unit_id.to_string()]).is_err(),
            "ad unit `{ad_unit_id}` must be rejected"
        );
    }
    assert!(validate_targets("9223372036854775807", &["9223372036854775807".to_string()]).is_ok());
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
        network_code: "1234567".to_string(),
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
                {"size":{"width":160,"height":600},"environmentType":"BROWSER"},
                {"size":{"width":160,"height":600},"environmentType":"BROWSER"},
                {"size":{"width":160,"height":1200},"environmentType":"BROWSER"}
            ],
            "hasChildren": false,
            "updateTime": "2026-07-10T00:00:00Z"
        }),
    );
    assert_eq!(summary["proof_state"], "complete_clear");
    assert_eq!(summary["identity_matches_request"], true);
    assert_eq!(summary["current"]["sizes"]["source_count"], 3);
    assert_eq!(summary["current"]["sizes"]["retained_count"], 3);
    assert_eq!(summary["current"]["sizes"]["truncated"], false);
    assert!(
        summary["current"]["sizes"]["source_fingerprint"]
            .as_str()
            .is_some_and(|value| value.len() == 16)
    );
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
        network_code: "1234567".to_string(),
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
        true,
    );
    assert_eq!(permission["proof_state"], "blocked_permission");
    assert!(!permission.to_string().contains("private provider detail"));
}

#[test]
fn malformed_identity_and_cross_network_parent_never_clear() {
    let target = RetirementTarget {
        network_code: "1234567".to_string(),
        ad_unit_id: "200".to_string(),
        resource_name: "networks/1234567/adUnits/200".to_string(),
    };
    let malformed = summarize_identity(&target, &json!({"name":"networks/1234567/adUnits/200"}));
    assert_eq!(malformed["proof_state"], "not_run");
    assert_eq!(malformed["identity_matches_request"], true);
    assert_eq!(malformed["shape_complete"], false);

    let foreign_parent = summarize_identity(
        &target,
        &json!({
            "name":"networks/1234567/adUnits/200",
            "adUnitCode":"fixture_unit",
            "status":"ACTIVE",
            "adUnitSizes":[],
            "hasChildren":false,
            "parentAdUnit":"networks/9999999/adUnits/42",
            "updateTime":"2026-07-10T00:00:00Z"
        }),
    );
    assert_eq!(foreign_parent["proof_state"], "not_run");
    assert_eq!(foreign_parent["current"]["parent_ad_unit_id"], Value::Null);
    assert!(
        foreign_parent["shape_issues"]
            .as_array()
            .is_some_and(|issues| issues
                .iter()
                .any(|issue| issue == "parent_ad_unit_invalid_or_cross_network"))
    );

    let invalid_environment = summarize_identity(
        &target,
        &json!({
            "name":"networks/1234567/adUnits/200",
            "adUnitCode":"fixture_unit",
            "status":"ACTIVE",
            "adUnitSizes":[{"size":{"width":160,"height":600},"environmentType":"INVALID"}],
            "hasChildren":false,
            "updateTime":"2026-07-10T00:00:00Z"
        }),
    );
    assert_eq!(invalid_environment["proof_state"], "not_run");
    assert!(
        invalid_environment["shape_issues"]
            .as_array()
            .is_some_and(|issues| issues
                .iter()
                .any(|issue| issue == "ad_unit_size_environment_invalid"))
    );

    let invalid_status = summarize_identity(
        &target,
        &json!({
            "name":"networks/1234567/adUnits/200",
            "adUnitCode":"fixture_unit",
            "status":"DELETED",
            "adUnitSizes":[],
            "hasChildren":false,
            "updateTime":"2026-07-10T00:00:00Z"
        }),
    );
    assert_eq!(invalid_status["proof_state"], "not_run");
    assert!(
        invalid_status["shape_issues"]
            .as_array()
            .is_some_and(|issues| issues
                .iter()
                .any(|issue| issue == "status_unknown_or_unspecified"))
    );
}

#[test]
fn size_fingerprint_covers_environment_companions_and_truncated_tail() {
    let target = RetirementTarget {
        network_code: "1234567".to_string(),
        ad_unit_id: "200".to_string(),
        resource_name: "networks/1234567/adUnits/200".to_string(),
    };
    let sizes = (0..21)
        .map(|index| {
            json!({
                "size":{"width":160,"height":600 + index},
                "environmentType":"VIDEO_PLAYER",
                "companions":[{"width":320,"height":50}]
            })
        })
        .collect::<Vec<_>>();
    let mut changed = sizes.clone();
    changed[20]["environmentType"] = Value::String("INVALID".to_string());
    let row = |ad_unit_sizes: Vec<Value>| {
        json!({
            "name":"networks/1234567/adUnits/200",
            "adUnitCode":"fixture_unit",
            "status":"ACTIVE",
            "adUnitSizes":ad_unit_sizes,
            "hasChildren":false,
            "updateTime":"2026-07-10T00:00:00Z"
        })
    };
    let first = summarize_identity(&target, &row(sizes));
    let second = summarize_identity(&target, &row(changed));
    assert_eq!(first["current"]["sizes"]["source_count"], 21);
    assert_eq!(first["current"]["sizes"]["retained_count"], 20);
    assert_eq!(first["current"]["sizes"]["truncated"], true);
    assert_eq!(first["proof_state"], "complete_clear");
    assert_eq!(second["proof_state"], "not_run");
    assert_ne!(
        first["identity_fingerprint"],
        second["identity_fingerprint"]
    );
}

#[test]
fn confirmed_blocker_plus_incomplete_target_is_partial_blocked() {
    let partial = summarize_identities(&[
        json!({"proof_state":"complete_blocked"}),
        json!({"proof_state":"blocked_auth"}),
    ]);
    assert_eq!(partial["proof_state"], "partial_blocked");

    let complete = summarize_identities(&[
        json!({"proof_state":"complete_blocked"}),
        json!({"proof_state":"complete_clear"}),
    ]);
    assert_eq!(complete["proof_state"], "complete_blocked");
}

#[test]
fn auth_bootstrap_is_not_reported_as_an_upstream_request() {
    let target = RetirementTarget {
        network_code: "1234567".to_string(),
        ad_unit_id: "200".to_string(),
        resource_name: "networks/1234567/adUnits/200".to_string(),
    };
    let summary = blocked_identity(
        &target,
        AdManagerError::AuthBootstrap("private auth detail".to_string()),
        false,
    );
    assert_eq!(summary["proof_state"], "blocked_auth");
    assert_eq!(summary["provider_request_state"], "not_sent");
    assert!(!summary.to_string().contains("private auth detail"));
}

#[tokio::test]
async fn successful_preflight_keeps_later_surfaces_not_run() {
    let response = assess_ad_unit_retirement_with_reader(
        &args("1234567", &["200"]),
        |network_code, resource_name| async move {
            assert_eq!(network_code, "1234567");
            assert_eq!(resource_name, "networks/1234567/adUnits/200");
            (
                Ok(json!({
                    "name": resource_name,
                    "adUnitCode": "fixture_unit",
                    "status": "ACTIVE",
                    "adUnitSizes": [{"size":{"width":300,"height":250},"environmentType":"BROWSER"}],
                    "hasChildren": false,
                    "updateTime": "2026-07-10T00:00:00Z"
                })),
                true,
            )
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
    assert_eq!(response["provider_requests"]["attempted_count"], 1);
    assert_eq!(response["provider_requests"]["not_sent_count"], 0);
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
            1,
        )
        .is_err()
    );
}
