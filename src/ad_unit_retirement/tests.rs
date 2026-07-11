use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::evidence::{EVIDENCE_PRODUCER_CONTRACT_VERSION, EvidenceSource, EvidenceState};

use super::descendants::*;
use super::inventory::*;
use super::receipt::*;
use super::*;

fn args(network_code: &str, ad_unit_ids: &[&str]) -> AdUnitRetirementAssessmentArgs {
    AdUnitRetirementAssessmentArgs {
        network_code: network_code.to_string(),
        ad_unit_ids: ad_unit_ids
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        evidence: Vec::new(),
        ad_unit_page_size: Some(100),
        max_ad_units: Some(100),
    }
}

fn receipt(
    source: EvidenceSource,
    state: EvidenceState,
    observed_at: u64,
) -> RetirementEvidenceReceipt {
    let windowed = matches!(
        source,
        EvidenceSource::DeliveryReport | EvidenceSource::Telemetry
    );
    RetirementEvidenceReceipt {
        network_code: "1234567".to_string(),
        source,
        source_version: match source {
            EvidenceSource::DependencyProbe | EvidenceSource::ExchangeProtectionReview => {
                EVIDENCE_PRODUCER_CONTRACT_VERSION
            }
            EvidenceSource::DeliveryReport => "gam-report-v1",
            EvidenceSource::SiteContract => "site-contract-v1",
            EvidenceSource::Telemetry => "telemetry-v1",
        }
        .to_string(),
        state,
        result_hash: Some(
            if matches!(
                source,
                EvidenceSource::DependencyProbe | EvidenceSource::ExchangeProtectionReview
            ) {
                "0123456789abcdef"
            } else {
                "sha256:0123456789abcdef"
            }
            .to_string(),
        ),
        observed_at_unix_seconds: Some(observed_at),
        ttl_seconds: Some(3_600),
        target_ad_unit_ids: vec!["200".to_string()],
        window_start_unix_seconds: windowed
            .then_some(observed_at.saturating_sub(MIN_ACTIVITY_WINDOW_SECONDS)),
        window_end_unix_seconds: windowed.then_some(observed_at),
        manual_ui_proof_included: false,
        note: None,
    }
}

#[test]
fn evidence_is_network_source_target_and_freshness_bound() {
    let mut evidence = receipt(
        EvidenceSource::DeliveryReport,
        EvidenceState::CompleteClear,
        3_999_900,
    );
    let clear = grade_evidence(
        "delivery",
        EvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("clear evidence");
    assert_eq!(clear["state"], "complete_clear");
    assert_eq!(clear["binding_valid"], true);

    evidence.source = EvidenceSource::Telemetry;
    let mismatch = grade_evidence(
        "delivery",
        EvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("mismatched evidence");
    assert_eq!(mismatch["state"], "invalid_binding");
    assert!(mismatch["binding_errors"].as_array().is_some_and(|errors| {
        errors.iter().any(|error| error.as_str() == Some("source"))
            && errors
                .iter()
                .any(|error| error.as_str() == Some("source_version"))
    }));

    evidence.source = EvidenceSource::DeliveryReport;
    evidence.source_version = "gam-report-v1".to_string();
    evidence.target_ad_unit_ids = vec!["0200".to_string()];
    let invalid_target = grade_evidence(
        "delivery",
        EvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("noncanonical target grades fail-closed");
    assert_eq!(invalid_target["state"], "invalid_binding");
    assert!(
        invalid_target["binding_errors"]
            .as_array()
            .is_some_and(|errors| { errors.iter().any(|error| error.as_str() == Some("targets")) })
    );
}

#[test]
fn evidence_rejects_unknown_versions_without_echoing_unbounded_enums() {
    let mut evidence = receipt(
        EvidenceSource::DeliveryReport,
        EvidenceState::CompleteClear,
        3_999_900,
    );
    evidence.source_version = "gam-report-v99".to_string();
    let graded = grade_evidence(
        "delivery",
        EvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("unknown version grades fail-closed");
    assert_eq!(graded["state"], "invalid_binding");

    let payload = json!({
        "network_code":"1234567",
        "source":"x".repeat(16 * 1024),
        "source_version":"gam-report-v1",
        "state":"complete_clear",
        "result_hash":"sha256:0123456789abcdef",
        "observed_at_unix_seconds":3_999_900,
        "ttl_seconds":3_600,
        "target_ad_unit_ids":["200"],
        "window_start_unix_seconds":1_407_900,
        "window_end_unix_seconds":3_999_900,
        "manual_ui_proof_included":false,
        "note":null
    });
    let error = serde_json::from_value::<RetirementEvidenceReceipt>(payload)
        .expect_err("unknown evidence sources must be rejected")
        .to_string();
    assert_eq!(error, "unsupported retirement evidence source");
}

#[test]
fn producer_v3_receipts_enforce_hash_ttl_and_source_state_contracts() {
    let mut dependency = receipt(
        EvidenceSource::DependencyProbe,
        EvidenceState::CompleteClear,
        3_999_900,
    );
    dependency.result_hash = Some("sha256:0123456789abcdef".to_string());
    let bad_hash = grade_evidence(
        "dependency",
        EvidenceSource::DependencyProbe,
        Some(&dependency),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("producer hash mismatch grades fail-closed");
    assert_eq!(bad_hash["state"], "invalid_binding");
    assert!(bad_hash["binding_errors"].as_array().is_some_and(|errors| {
        errors
            .iter()
            .any(|error| error.as_str() == Some("result_hash"))
    }));

    dependency.result_hash = Some("0123456789abcdef".to_string());
    dependency.ttl_seconds = Some(7_200);
    let bad_ttl = grade_evidence(
        "dependency",
        EvidenceSource::DependencyProbe,
        Some(&dependency),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("producer TTL mismatch grades fail-closed");
    assert_eq!(bad_ttl["state"], "invalid_binding");
    assert!(
        bad_ttl["binding_errors"]
            .as_array()
            .is_some_and(|errors| { errors.iter().any(|error| error.as_str() == Some("ttl")) })
    );

    let mut protection = receipt(
        EvidenceSource::ExchangeProtectionReview,
        EvidenceState::CompleteClear,
        3_999_900,
    );
    protection.manual_ui_proof_included = true;
    let impossible_state = grade_evidence(
        "exchange_protection",
        EvidenceSource::ExchangeProtectionReview,
        Some(&protection),
        "1234567",
        &["200".to_string()],
        4_000_000,
        true,
    )
    .expect("source-impossible state grades fail-closed");
    assert_eq!(impossible_state["state"], "invalid_binding");
    assert!(
        impossible_state["binding_errors"]
            .as_array()
            .is_some_and(|errors| {
                errors
                    .iter()
                    .any(|error| error.as_str() == Some("source_state"))
            })
    );
}

#[test]
fn receipt_target_ids_are_required_and_partial_blockers_remain_incomplete() {
    let payload = json!({
        "network_code":"1234567",
        "source":"dependency_probe",
        "source_version":EVIDENCE_PRODUCER_CONTRACT_VERSION,
        "state":"complete_clear",
        "result_hash":"0123456789abcdef",
        "observed_at_unix_seconds":3_999_900,
        "ttl_seconds":3_600,
        "window_start_unix_seconds":null,
        "window_end_unix_seconds":null,
        "manual_ui_proof_included":false,
        "note":null
    });
    assert!(serde_json::from_value::<RetirementEvidenceReceipt>(payload).is_err());

    let raw_payload = json!({
        "network_code":"1234567",
        "source":"dependency_probe",
        "source_version":EVIDENCE_PRODUCER_CONTRACT_VERSION,
        "state":"complete_clear",
        "result_hash":"0123456789abcdef",
        "observed_at_unix_seconds":3_999_900,
        "ttl_seconds":3_600,
        "target_ad_unit_ids":["200"],
        "window_start_unix_seconds":null,
        "window_end_unix_seconds":null,
        "manual_ui_proof_included":false,
        "note":null,
        "raw_report":{"rows":[{"sensitive":"payload"}]}
    });
    assert!(serde_json::from_value::<RetirementEvidenceReceipt>(raw_payload).is_err());

    let partial = receipt(
        EvidenceSource::DependencyProbe,
        EvidenceState::PartialBlocked,
        3_999_900,
    );
    let graded = grade_evidence(
        "dependency",
        EvidenceSource::DependencyProbe,
        Some(&partial),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("partial blocker grades safely");
    assert_eq!(graded["state"], "partial_blocked");
    assert_eq!(graded["complete_for_summary"], false);
}

#[test]
fn evidence_windows_and_ttl_fail_closed() {
    let mut evidence = receipt(
        EvidenceSource::DeliveryReport,
        EvidenceState::CompleteClear,
        3_995_000,
    );
    evidence.ttl_seconds = Some(60);
    let stale = grade_evidence(
        "delivery",
        EvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("stale evidence");
    assert_eq!(stale["state"], "stale");

    evidence.observed_at_unix_seconds = Some(3_999_900);
    evidence.window_start_unix_seconds = Some(3_999_800);
    evidence.window_end_unix_seconds = Some(3_999_900);
    evidence.ttl_seconds = Some(3_600);
    let short = grade_evidence(
        "delivery",
        EvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("short window grades fail-closed");
    assert_eq!(short["state"], "invalid_binding");

    evidence.window_start_unix_seconds = None;
    evidence.window_end_unix_seconds = None;
    let missing = grade_evidence(
        "delivery",
        EvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("missing window grades fail-closed");
    assert_eq!(missing["state"], "invalid_binding");
}

#[test]
fn protection_clear_requires_manual_ui_proof() {
    let mut evidence = receipt(
        EvidenceSource::ExchangeProtectionReview,
        EvidenceState::ManualUiProofRequired,
        3_999_900,
    );
    let required = grade_evidence(
        "exchange_protection",
        EvidenceSource::ExchangeProtectionReview,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        true,
    )
    .expect("manual proof required");
    assert_eq!(required["state"], "manual_ui_proof_required");
    assert_eq!(required["complete_for_summary"], false);

    evidence.manual_ui_proof_included = true;
    let clear = grade_evidence(
        "exchange_protection",
        EvidenceSource::ExchangeProtectionReview,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        true,
    )
    .expect("manual proof accepted");
    assert_eq!(clear["state"], "complete_clear");
    assert_eq!(clear["complete_for_summary"], true);
}

#[test]
fn evidence_bundle_rejects_duplicate_sources_and_reports_missing_surfaces() {
    let dependency = receipt(
        EvidenceSource::DependencyProbe,
        EvidenceState::CompleteClear,
        3_999_900,
    );
    assert!(
        grade_evidence_bundle(
            &[dependency.clone(), dependency],
            "1234567",
            &["200".to_string()],
            4_000_000,
        )
        .is_err()
    );
    let empty = grade_evidence_bundle(&[], "1234567", &["200".to_string()], 4_000_000)
        .expect("missing receipts remain explicit");
    for surface in [
        "dependency",
        "delivery",
        "exchange_protection",
        "site_contract",
        "telemetry",
    ] {
        assert_eq!(empty[surface]["state"], "not_run");
        assert_eq!(empty[surface]["complete_for_summary"], false);
    }
}

#[tokio::test]
async fn invalid_evidence_fails_before_any_provider_request() {
    let dependency = receipt(
        EvidenceSource::DependencyProbe,
        EvidenceState::CompleteClear,
        3_999_900,
    );
    let mut invalid_args = args("1234567", &["200"]);
    invalid_args.evidence = vec![dependency.clone(), dependency];

    let result =
        assess_ad_unit_retirement_with_readers(
            &invalid_args,
            |_network_code| async move { panic!("network reader must not run") },
            |_network_code, _resource_name| async move { panic!("identity reader must not run") },
            |_network_code, _page_size, _page_token| async move {
                panic!("catalog reader must not run")
            },
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn evidence_freshness_is_rechecked_after_provider_reads() {
    let observed_at = current_unix_seconds().expect("current time");
    let mut delivery = receipt(
        EvidenceSource::DeliveryReport,
        EvidenceState::CompleteClear,
        observed_at,
    );
    delivery.ttl_seconds = Some(1);
    let mut assessment_args = args("1234567", &["200"]);
    assessment_args.evidence = vec![delivery];

    let response = assess_ad_unit_retirement_with_readers(
        &assessment_args,
        |_network_code| async move { (Ok(network_row("100")), true) },
        |_network_code, resource_name| async move {
            if resource_name == "networks/1234567/adUnits/100" {
                return (Ok(effective_root_row("100", "50")), true);
            }
            (
                Ok(json!({
                    "name":resource_name,
                    "adUnitCode":"fixture_unit",
                    "status":"ACTIVE",
                    "adUnitSizes":[],
                    "parentAdUnit":"networks/1234567/adUnits/100",
                    "hasChildren":false,
                    "updateTime":"2026-07-10T00:00:00Z"
                })),
                true,
            )
        },
        |_network_code, _page_size, _page_token| async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let payload = json!({"adUnits":[
                catalog_row("50", None, true, "ACTIVE"),
                catalog_row("100", Some("50"), true, "ACTIVE"),
                catalog_row("200", Some("100"), false, "ACTIVE")
            ]});
            let bytes = payload.to_string().len();
            (Ok((payload, bytes)), true)
        },
    )
    .await
    .expect("assessment returns stale grading");
    assert_eq!(response["evidence"]["delivery"]["state"], "stale");
    assert_eq!(
        response["evidence"]["delivery"]["complete_for_summary"],
        false
    );
}

#[test]
fn assessment_fingerprint_is_network_and_target_bound() {
    let identity = json!({"proof_state":"complete_clear","result_fingerprint":"identity"});
    let descendants =
        json!({"proof_state":"complete_clear","descendant_result_fingerprint":"descendants"});
    let evidence = json!({"dependency":{"state":"not_run"}});
    let build = |network_code: &str, target_id: &str| {
        build_preflight_response(
            network_code.to_string(),
            vec![target_id.to_string()],
            identity.clone(),
            descendants.clone(),
            evidence.clone(),
            ProviderRequestSummary {
                network_attempted_count: 1,
                effective_root_attempted_count: 1,
                identity_attempted_count: 1,
                descendant_page_attempted_count: 1,
            },
            HierarchyScanConfig {
                page_size: 100,
                max_ad_units: 100,
            },
        )
        .expect("bounded assessment")
    };
    assert_ne!(
        build("1234567", "200")["assessment_fingerprint"],
        build("7654321", "200")["assessment_fingerprint"]
    );
    assert_ne!(
        build("1234567", "200")["assessment_fingerprint"],
        build("1234567", "201")["assessment_fingerprint"]
    );
}

fn child_claims(values: &[(&str, bool)]) -> BTreeMap<String, bool> {
    values
        .iter()
        .map(|(id, has_children)| ((*id).to_string(), *has_children))
        .collect()
}

fn network_row(effective_root_id: &str) -> Value {
    json!({
        "name":"networks/1234567",
        "networkCode":"1234567",
        "effectiveRootAdUnit":format!("networks/1234567/adUnits/{effective_root_id}")
    })
}

fn effective_root_row(effective_root_id: &str, google_root_id: &str) -> Value {
    json!({
        "name":format!("networks/1234567/adUnits/{effective_root_id}"),
        "parentAdUnit":format!("networks/1234567/adUnits/{google_root_id}")
    })
}

fn catalog_row(id: &str, parent_id: Option<&str>, has_children: bool, status: &str) -> Value {
    let ancestors = parent_id
        .map(|parent_id| {
            vec![json!({
                "parentAdUnit":format!("networks/1234567/adUnits/{parent_id}")
            })]
        })
        .unwrap_or_default();
    json!({
        "name":format!("networks/1234567/adUnits/{id}"),
        "parentAdUnit":parent_id.map(|parent_id| format!("networks/1234567/adUnits/{parent_id}")),
        "parentPath":ancestors,
        "hasChildren":has_children,
        "status":status,
        "updateTime":"2026-07-10T00:00:00Z"
    })
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
fn effective_root_identity_requires_exact_same_network_resources() {
    assert_eq!(
        effective_root_id_from_network("1234567", &network_row("100")).as_deref(),
        Some("100")
    );
    assert_eq!(
        google_root_id_from_effective_root("1234567", "100", &effective_root_row("100", "50"))
            .as_deref(),
        Some("50")
    );
    assert!(
        effective_root_id_from_network(
            "1234567",
            &json!({
                "name":"networks/1234567",
                "networkCode":"1234567",
                "effectiveRootAdUnit":"networks/7654321/adUnits/100"
            })
        )
        .is_none()
    );
    assert!(
        google_root_id_from_effective_root(
            "1234567",
            "100",
            &json!({
                "name":"networks/1234567/adUnits/100",
                "parentAdUnit":"networks/7654321/adUnits/50"
            })
        )
        .is_none()
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
    assert!(summary["current"].get("ad_unit_id").is_none());
    assert!(summary.get("resource_name").is_none());
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

#[test]
fn hierarchy_scan_rejects_malformed_final_pages_tokens_and_rows() {
    let cases = [
        json!({}),
        json!({"adUnits":null}),
        json!({"adUnits":[],"nextPageToken":42}),
        json!({"adUnits":[{}]}),
        json!({"adUnits":[{"name":42}]}),
    ];
    for malformed in cases {
        let mut scan = DescendantScan::new(
            "1234567",
            &["200".to_string()],
            &child_claims(&[("200", false)]),
            100,
        );
        let first = json!({
            "adUnits":[catalog_row("200", None, false, "ACTIVE")],
            "nextPageToken":"page-2"
        });
        assert_eq!(
            scan.consume_page(&first, first.to_string().len())
                .as_deref(),
            Some("page-2")
        );
        assert!(
            scan.consume_page(&malformed, malformed.to_string().len())
                .is_none()
        );
        let summary = scan.finish(100);
        assert_eq!(summary["proof_state"], "partial_capped");
        assert_eq!(summary["hierarchy_reconciled"], false);
    }
}

#[test]
fn hierarchy_scan_bounds_rows_pages_tokens_and_response_bytes() {
    let mut row_capped = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        1,
    );
    let page = json!({
        "adUnits":[
            catalog_row("200", None, false, "ACTIVE"),
            catalog_row("201", None, false, "ACTIVE")
        ]
    });
    row_capped.consume_page(&page, page.to_string().len());
    let summary = row_capped.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert!(
        summary["issues"]
            .as_array()
            .is_some_and(|issues| issues.iter().any(|issue| issue == "row_cap_reached"))
    );

    let mut byte_capped = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        100,
    );
    let page = json!({"adUnits":[catalog_row("200", None, false, "ACTIVE")]});
    byte_capped.consume_page(&page, MAX_DESCENDANT_PAGE_BYTES + 1);
    let summary = byte_capped.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert!(summary["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue == "page_response_bytes_exceeded")
    }));

    let mut token_loop = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        100,
    );
    let first =
        json!({"adUnits":[catalog_row("200", None, false, "ACTIVE")],"nextPageToken":"repeat"});
    let second =
        json!({"adUnits":[catalog_row("300", None, false, "ACTIVE")],"nextPageToken":"repeat"});
    assert_eq!(
        token_loop
            .consume_page(&first, first.to_string().len())
            .as_deref(),
        Some("repeat")
    );
    assert!(
        token_loop
            .consume_page(&second, second.to_string().len())
            .is_none()
    );
    let summary = token_loop.finish(100);
    assert!(
        summary["issues"]
            .as_array()
            .is_some_and(|issues| issues.iter().any(|issue| issue == "repeated_page_token"))
    );
}

#[test]
fn hierarchy_scan_requires_strict_order_and_bidirectional_child_flags() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        100,
    );
    let page = json!({"adUnits":[
        catalog_row("201", Some("200"), false, "ACTIVE"),
        catalog_row("200", None, false, "ACTIVE")
    ]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    let issues = summary["issues"].as_array().expect("issue list");
    assert!(issues.iter().any(|issue| issue == "catalog_order_invalid"));
    assert!(
        issues
            .iter()
            .any(|issue| issue == "catalog_child_flag_mismatch")
    );
    assert_eq!(summary["proof_state"], "partial_blocked");

    let mut true_without_child = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", true)]),
        100,
    );
    let page = json!({"adUnits":[catalog_row("200", None, true, "ACTIVE")]});
    true_without_child.consume_page(&page, page.to_string().len());
    let summary = true_without_child.finish(100);
    assert!(summary["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue == "catalog_child_flag_mismatch")
    }));
}

#[test]
fn hierarchy_scan_orders_resource_names_by_numeric_ad_unit_id() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["10".to_string()],
        &child_claims(&[("10", false)]),
        100,
    );
    let page = json!({"adUnits":[
        catalog_row("9", None, true, "ACTIVE"),
        catalog_row("10", Some("9"), false, "ACTIVE")
    ]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "complete_clear");
    assert!(summary.get("issues").is_none());
}

#[test]
fn hierarchy_scan_requires_one_root_and_reconciles_target_parent_identity() {
    let mut multiple_roots = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        100,
    );
    let page = json!({"adUnits":[
        catalog_row("100", None, false, "ACTIVE"),
        catalog_row("200", None, false, "ACTIVE")
    ]});
    multiple_roots.consume_page(&page, page.to_string().len());
    let summary = multiple_roots.finish(100);
    assert!(summary["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue == "catalog_root_count_invalid")
    }));

    let expected_parents = BTreeMap::from([("200".to_string(), Some("100".to_string()))]);
    let mut stripped_parent = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        100,
    )
    .with_expected_parent_ids(&expected_parents);
    let page = json!({"adUnits":[catalog_row("200", None, false, "ACTIVE")]});
    stripped_parent.consume_page(&page, page.to_string().len());
    let summary = stripped_parent.finish(100);
    assert!(summary["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue == "identity_catalog_parent_mismatch")
    }));
    assert_eq!(summary["proof_state"], "partial_capped");
}

#[test]
fn root_catalog_rows_may_omit_or_null_parent_path() {
    for parent_path in [None, Some(Value::Null)] {
        let mut row = catalog_row("200", None, false, "ACTIVE");
        let object = row.as_object_mut().expect("catalog row object");
        match &parent_path {
            Some(value) => {
                object.insert("parentPath".to_string(), value.clone());
            }
            None => {
                object.remove("parentPath");
            }
        }
        let mut scan = DescendantScan::new(
            "1234567",
            &["200".to_string()],
            &child_claims(&[("200", false)]),
            100,
        );
        let page = json!({"adUnits":[row]});
        scan.consume_page(&page, page.to_string().len());
        let summary = scan.finish(100);
        assert_eq!(summary["proof_state"], "complete_clear");
        assert!(summary.get("issues").is_none());
    }
}

#[test]
fn live_shaped_catalog_may_omit_the_google_created_root_from_parent_path() {
    let mut network_root = catalog_row("15422", None, true, "ACTIVE");
    network_root
        .as_object_mut()
        .expect("catalog row object")
        .remove("parentPath");
    let mut publisher_root = catalog_row("303152", Some("15422"), true, "ACTIVE");
    publisher_root
        .as_object_mut()
        .expect("catalog row object")
        .remove("parentPath");
    let target = json!({
        "name":"networks/1234567/adUnits/182784272",
        "parentAdUnit":"networks/1234567/adUnits/303152",
        "parentPath":[{"parentAdUnit":"networks/1234567/adUnits/303152"}],
        "hasChildren":false,
        "status":"ACTIVE",
        "updateTime":"2026-02-10T04:54:39.470Z"
    });
    let expected_parents = BTreeMap::from([("182784272".to_string(), Some("303152".to_string()))]);
    let mut scan = DescendantScan::new(
        "1234567",
        &["182784272".to_string()],
        &child_claims(&[("182784272", false)]),
        100,
    )
    .with_expected_parent_ids(&expected_parents)
    .require_root_identity(Some(("303152", "15422")));
    let page = json!({"adUnits":[network_root,publisher_root,target]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "complete_clear");
    assert!(summary.get("issues").is_none());
}

#[test]
fn omitted_parent_path_below_the_root_still_fails_closed() {
    let mut direct_root_child = catalog_row("200", Some("100"), true, "ACTIVE");
    direct_root_child
        .as_object_mut()
        .expect("catalog row object")
        .remove("parentPath");
    let mut grandchild = catalog_row("300", Some("200"), false, "ACTIVE");
    grandchild
        .as_object_mut()
        .expect("catalog row object")
        .remove("parentPath");
    let mut scan = DescendantScan::new(
        "1234567",
        &["300".to_string()],
        &child_claims(&[("300", false)]),
        100,
    )
    .require_root_identity(Some(("200", "100")));
    let page = json!({"adUnits":[
        catalog_row("100", None, true, "ACTIVE"),
        direct_root_child,
        grandchild
    ]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert!(summary["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue == "catalog_ancestry_mismatch")
    }));
}

#[test]
fn authoritative_root_identity_rejects_a_sparse_fabricated_root() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["300".to_string()],
        &child_claims(&[("300", false)]),
        100,
    )
    .require_root_identity(Some(("200", "100")));
    let page = json!({"adUnits":[
        catalog_row("200", None, true, "ACTIVE"),
        catalog_row("300", Some("200"), false, "ACTIVE")
    ]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert!(summary["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue == "google_root_catalog_mismatch")
    }));
}

#[test]
fn unavailable_authoritative_root_identity_never_clears() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        100,
    )
    .require_root_identity(None);
    let page = json!({"adUnits":[catalog_row("200", None, false, "ACTIVE")]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert!(summary["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue == "effective_root_identity_unverified")
    }));
}

#[test]
fn unavailable_identity_child_flag_does_not_create_a_false_mismatch() {
    let mut scan = DescendantScan::new("1234567", &["200".to_string()], &BTreeMap::new(), 100);
    let page = json!({"adUnits":[catalog_row("200", None, false, "ACTIVE")]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "complete_clear");
    assert!(summary.get("issues").is_none());
}

#[test]
fn duplicate_rows_cannot_erase_an_observed_positive_blocker() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", true)]),
        100,
    );
    let page = json!({"adUnits":[
        catalog_row("200", None, true, "ACTIVE"),
        catalog_row("201", Some("200"), false, "INACTIVE"),
        catalog_row("201", Some("200"), false, "ARCHIVED")
    ]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_blocked");
    assert_eq!(summary["blocking_external_descendant_count"], 1);
    assert!(
        summary["issues"]
            .as_array()
            .is_some_and(|issues| issues.iter().any(|issue| issue == "duplicate_catalog_id"))
    );
}

#[test]
fn hierarchy_scan_validates_complete_root_to_parent_paths() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", true)]),
        100,
    );
    let malformed_grandchild = json!({
        "name":"networks/1234567/adUnits/300",
        "parentAdUnit":"networks/1234567/adUnits/201",
        "parentPath":[],
        "hasChildren":false,
        "status":"ACTIVE",
        "updateTime":"2026-07-10T00:00:00Z"
    });
    let page = json!({"adUnits":[
        catalog_row("200", None, true, "ACTIVE"),
        catalog_row("201", Some("200"), true, "ACTIVE"),
        malformed_grandchild
    ]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_blocked");
    assert!(summary["issues"].as_array().is_some_and(|issues| {
        issues
            .iter()
            .any(|issue| issue == "catalog_ancestry_mismatch")
    }));
}

#[test]
fn known_external_descendant_remains_a_blocker_after_late_read_failure() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", true)]),
        100,
    );
    let page = json!({
        "adUnits":[
            catalog_row("200", None, true, "ACTIVE"),
            catalog_row("201", Some("200"), false, "INACTIVE")
        ],
        "nextPageToken":"page-2"
    });
    assert_eq!(
        scan.consume_page(&page, page.to_string().len()).as_deref(),
        Some("page-2")
    );
    scan.record_failure(
        AdManagerError::UpstreamApi {
            status: 503,
            message: "private upstream detail".to_string(),
        },
        true,
    );
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_blocked");
    assert_eq!(summary["blocking_external_descendant_count"], 1);
    assert_eq!(summary["external_descendant_status_counts"]["INACTIVE"], 1);
    assert_eq!(
        summary["provider_request_state"],
        "completed_then_attempted_incomplete"
    );
    assert!(!summary.to_string().contains("private upstream detail"));

    let mut pre_send_failure = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        100,
    );
    let page = json!({
        "adUnits":[catalog_row("200", None, false, "ACTIVE")],
        "nextPageToken":"page-2"
    });
    pre_send_failure.consume_page(&page, page.to_string().len());
    pre_send_failure.record_failure(
        AdManagerError::AuthBootstrap("private auth detail".to_string()),
        false,
    );
    let summary = pre_send_failure.finish(100);
    assert_eq!(summary["provider_request_state"], "completed_then_not_sent");
}

#[test]
fn archived_descendants_do_not_block_and_targets_are_ordered_child_first() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string(), "201".to_string()],
        &child_claims(&[("200", true), ("201", true)]),
        100,
    );
    let grandchild = json!({
        "name":"networks/1234567/adUnits/300",
        "parentAdUnit":"networks/1234567/adUnits/201",
        "parentPath":[
            {"parentAdUnit":"networks/1234567/adUnits/200"},
            {"parentAdUnit":"networks/1234567/adUnits/201"}
        ],
        "hasChildren":false,
        "status":"ARCHIVED",
        "updateTime":"2026-07-10T00:00:00Z"
    });
    let page = json!({"adUnits":[
        catalog_row("200", None, true, "ACTIVE"),
        catalog_row("201", Some("200"), true, "ACTIVE"),
        grandchild
    ]});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "complete_clear");
    assert_eq!(summary["blocking_external_descendant_count"], 0);
    assert_eq!(
        summary["required_child_first_target_order"],
        json!(["201", "200"])
    );
}

#[test]
fn intra_target_relationship_sample_reports_truncation_and_full_count() {
    let target_ids = (200..210).map(|id| id.to_string()).collect::<Vec<_>>();
    let child_claims = (200..210)
        .map(|id| (id.to_string(), id < 209))
        .collect::<BTreeMap<_, _>>();
    let rows = (200..210)
        .map(|id| {
            let ancestors = (200..id)
                .map(|ancestor| {
                    json!({"parentAdUnit":format!("networks/1234567/adUnits/{ancestor}")})
                })
                .collect::<Vec<_>>();
            json!({
                "name":format!("networks/1234567/adUnits/{id}"),
                "parentAdUnit":(id > 200).then(|| format!("networks/1234567/adUnits/{}", id - 1)),
                "parentPath":ancestors,
                "hasChildren":id < 209,
                "status":"ACTIVE",
                "updateTime":"2026-07-10T00:00:00Z"
            })
        })
        .collect::<Vec<_>>();
    let mut scan = DescendantScan::new("1234567", &target_ids, &child_claims, 100);
    let page = json!({"adUnits":rows});
    scan.consume_page(&page, page.to_string().len());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "complete_clear");
    assert_eq!(summary["intra_target_relationship_count"], 9);
    assert_eq!(summary["intra_target_hierarchy_truncated"], true);
    assert_eq!(
        summary["intra_target_hierarchy"].as_array().map(Vec::len),
        Some(DESCENDANT_SAMPLE_LIMIT)
    );
}

#[tokio::test]
async fn successful_preflight_reconciles_hierarchy_and_keeps_later_surfaces_not_run() {
    let response = assess_ad_unit_retirement_with_readers(
        &args("1234567", &["200"]),
        |_network_code| async move { (Ok(network_row("100")), true) },
        |network_code, resource_name| async move {
            assert_eq!(network_code, "1234567");
            if resource_name == "networks/1234567/adUnits/100" {
                return (Ok(effective_root_row("100", "50")), true);
            }
            (
                Ok(json!({
                    "name": resource_name,
                    "adUnitCode": "fixture_unit",
                    "status": "ACTIVE",
                    "adUnitSizes": [{"size":{"width":300,"height":250},"environmentType":"BROWSER"}],
                    "parentAdUnit":"networks/1234567/adUnits/100",
                    "hasChildren": false,
                    "updateTime": "2026-07-10T00:00:00Z"
                })),
                true,
            )
        },
        |_network_code, _page_size, _page_token| async move {
            let payload = json!({
                "adUnits":[
                    catalog_row("50", None, true, "ACTIVE"),
                    catalog_row("100", Some("50"), true, "ACTIVE"),
                    catalog_row("200", Some("100"), false, "ACTIVE")
                ]
            });
            let bytes = payload.to_string().len();
            (Ok((payload, bytes)), true)
        },
    )
    .await
    .expect("successful exact-identity preflight");

    assert_eq!(response["identity"]["proof_state"], "complete_clear");
    assert_eq!(response["descendants"]["proof_state"], "complete_clear");
    assert_eq!(response["evidence"]["dependency"]["state"], "not_run");
    assert_eq!(response["evidence"]["telemetry"]["state"], "not_run");
    assert_eq!(response["recommendation"]["decision"], "not_run");
    assert_eq!(
        response["recommendation"]["safe_to_archive_or_retire"],
        false
    );
    assert_eq!(response["mutation_performed"], false);
    assert_eq!(response["provider_requests"]["attempted_count"], 4);
    assert_eq!(response["provider_requests"]["network_attempted_count"], 1);
    assert_eq!(
        response["provider_requests"]["effective_root_attempted_count"],
        1
    );
    assert_eq!(response["provider_requests"]["identity_not_sent_count"], 0);
    assert_eq!(
        response["authorization"]["archive_or_deactivate_authorized"],
        false
    );
    assert!(response_bytes(&response) <= MAX_INNER_DATA_BYTES);
}

#[tokio::test]
async fn maximal_descendant_sample_stays_inside_inner_response_cap() {
    let response = assess_ad_unit_retirement_with_readers(
        &args("1234567", &["200"]),
        |_network_code| async move { (Ok(network_row("100")), true) },
        |_network_code, resource_name| async move {
            if resource_name == "networks/1234567/adUnits/100" {
                return (Ok(effective_root_row("100", "50")), true);
            }
            (
                Ok(json!({
                    "name":resource_name,
                    "adUnitCode":"fixture_root",
                    "status":"ACTIVE",
                    "adUnitSizes":[],
                    "parentAdUnit":"networks/1234567/adUnits/100",
                    "hasChildren":true,
                    "updateTime":"2026-07-10T00:00:00Z"
                })),
                true,
            )
        },
        |_network_code, _page_size, _page_token| async move {
            let mut rows = vec![
                catalog_row("50", None, true, "ACTIVE"),
                catalog_row("100", Some("50"), true, "ACTIVE"),
                catalog_row("200", Some("100"), true, "ACTIVE"),
            ];
            rows.extend((201..=209).map(|id| {
                json!({
                    "name":format!("networks/1234567/adUnits/{id}"),
                    "parentAdUnit":"networks/1234567/adUnits/200",
                    "parentPath":[
                        {"parentAdUnit":"networks/1234567/adUnits/100"},
                        {"parentAdUnit":"networks/1234567/adUnits/200"}
                    ],
                    "hasChildren":false,
                    "status":"ARCHIVED",
                    "updateTime":"2026-07-10T00:00:00Z"
                })
            }));
            let payload = json!({"adUnits":rows});
            let bytes = payload.to_string().len();
            (Ok((payload, bytes)), true)
        },
    )
    .await
    .expect("bounded maximal descendant sample");

    assert_eq!(
        response["descendants"]["external_descendant_sample"]
            .as_array()
            .map(Vec::len),
        Some(DESCENDANT_SAMPLE_LIMIT)
    );
    assert_eq!(
        response["descendants"]["external_descendant_sample_truncated"],
        true
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
            json!({"proof_state":"complete_clear"}),
            json!({"dependency":{"state":"not_run"}}),
            ProviderRequestSummary {
                network_attempted_count: 1,
                effective_root_attempted_count: 1,
                identity_attempted_count: 1,
                descendant_page_attempted_count: 1,
            },
            HierarchyScanConfig {
                page_size: 100,
                max_ad_units: 100,
            },
        )
        .is_err()
    );
}
