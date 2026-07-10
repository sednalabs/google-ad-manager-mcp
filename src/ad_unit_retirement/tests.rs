use super::decision::*;
use super::descendants::*;
use super::inventory::*;
use super::receipt::*;
use super::*;

fn receipt(
    source: RetirementEvidenceSource,
    state: RetirementEvidenceState,
    observed_at: u64,
) -> RetirementEvidenceReceipt {
    let windowed = matches!(
        source,
        RetirementEvidenceSource::DeliveryReport | RetirementEvidenceSource::Telemetry
    );
    RetirementEvidenceReceipt {
        network_code: "1234567".to_string(),
        source,
        source_version: "0.1.1-alpha.0".to_string(),
        state,
        result_hash: Some("sha256:0123456789abcdef".to_string()),
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

fn child_claims(values: &[(&str, bool)]) -> BTreeMap<String, bool> {
    values
        .iter()
        .map(|(id, has_children)| ((*id).to_string(), *has_children))
        .collect()
}

#[test]
fn targets_are_network_bound_and_canonical() {
    let targets = validate_targets("1234567", &["00200".to_string()])
        .expect("numeric ids should canonicalize");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].ad_unit_id, "200");
    assert!(validate_network_code("not-a-network").is_err());
    assert!(validate_targets("1234567", &[]).is_err());
    assert!(validate_targets("1234567", &["200".to_string(), "0200".to_string()]).is_err());
}

#[test]
fn identity_is_compact_and_exact_target_bound() {
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
            "adUnitSizes": [{"size":{"width":160,"height":600}}],
            "hasChildren": false,
            "updateTime": "2026-07-10T00:00:00Z"
        }),
    );
    assert_eq!(summary["proof_state"], "complete_clear");
    assert_eq!(summary["current"]["sizes"][0], "160x600");
    assert!(summary["current"].get("display_name").is_none());
    assert!(summary["current"].get("description").is_none());
}

#[test]
fn identity_permission_failure_is_explicitly_blocked() {
    let target = RetirementTarget {
        ad_unit_id: "200".to_string(),
        resource_name: "networks/1234567/adUnits/200".to_string(),
    };
    let summary = blocked_identity(
        &target,
        AdManagerError::UpstreamApi {
            status: 403,
            message: "fixture permission denial".to_string(),
        },
    );
    assert_eq!(summary["proof_state"], "blocked_permission");
    assert!(
        summary
            .to_string()
            .find("fixture permission denial")
            .is_none()
    );
}

#[test]
fn final_oversized_page_is_capped_without_next_token() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", false)]),
        1,
    );
    let next = scan.consume_page(&json!({
            "adUnits": [
                {"name":"networks/1234567/adUnits/200","parentPath":[],"hasChildren":false},
                {"name":"networks/1234567/adUnits/201","parentAdUnit":"networks/1234567/adUnits/200","parentPath":[{"parentAdUnit":"networks/1234567/adUnits/200"}],"status":"ACTIVE"}
            ]
        }));
    assert!(next.is_none());
    let summary = scan.finish(1);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert_eq!(summary["catalog_capped"], true);
    assert_eq!(summary["scan_complete"], false);
}

#[test]
fn empty_pagination_page_fails_closed() {
    let mut scan = DescendantScan::new("1234567", &["200".to_string()], &BTreeMap::new(), 100);
    let next = scan.consume_page(&json!({
        "adUnits": [],
        "nextPageToken": "another-page"
    }));
    assert!(next.is_none());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert_eq!(summary["zero_progress_page"], true);
}

#[test]
fn repeated_page_token_and_child_flag_mismatch_fail_closed() {
    let claims = child_claims(&[("200", true)]);
    let mut scan = DescendantScan::new("1234567", &["200".to_string()], &claims, 100);
    let next = scan.consume_page(&json!({
        "adUnits": [{
            "name":"networks/1234567/adUnits/200",
            "parentPath":[],
            "hasChildren":false
        }],
        "nextPageToken":"same-token"
    }));
    assert_eq!(next.as_deref(), Some("same-token"));
    let next = scan.consume_page(&json!({
        "adUnits": [{
            "name":"networks/1234567/adUnits/300",
            "parentPath":[],
            "hasChildren":false
        }],
        "nextPageToken":"same-token"
    }));
    assert!(next.is_none());
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert_eq!(summary["repeated_page_token"], true);
    assert_eq!(summary["identity_list_child_flag_mismatch"], true);
}

#[test]
fn intra_target_child_requires_sequence_but_is_not_external_blocker() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string(), "201".to_string()],
        &child_claims(&[("200", true), ("201", false)]),
        100,
    );
    scan.consume_page(&json!({
            "adUnits": [
                {"name":"networks/1234567/adUnits/200","parentPath":[],"hasChildren":true,"status":"ACTIVE"},
                {"name":"networks/1234567/adUnits/201","parentAdUnit":"networks/1234567/adUnits/200","parentPath":[{"parentAdUnit":"networks/1234567/adUnits/200"}],"hasChildren":false,"status":"ACTIVE"}
            ]
        }));
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "complete_clear");
    assert_eq!(summary["blocking_external_descendant_count"], 0);
    assert_eq!(summary["requires_child_first_sequence"], true);
    assert_eq!(summary["required_child_first_target_order"][0], "201");
}

#[test]
fn active_external_descendant_blocks_retirement() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", true)]),
        100,
    );
    scan.consume_page(&json!({
            "adUnits": [
                {"name":"networks/1234567/adUnits/200","parentPath":[],"hasChildren":true,"status":"ACTIVE"},
                {"name":"networks/1234567/adUnits/999","parentAdUnit":"networks/1234567/adUnits/200","parentPath":[{"parentAdUnit":"networks/1234567/adUnits/200"}],"status":"ACTIVE"}
            ]
        }));
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "complete_blocked");
    assert_eq!(summary["blocking_external_descendant_count"], 1);
}

#[test]
fn successful_two_page_scan_reconciles_full_ancestry() {
    let mut scan = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", true)]),
        100,
    );
    let next = scan.consume_page(&json!({
        "adUnits": [{"name":"networks/1234567/adUnits/200","parentPath":[],"hasChildren":true}],
        "nextPageToken":"page-2"
    }));
    assert_eq!(next.as_deref(), Some("page-2"));
    assert!(
        scan.consume_page(&json!({
            "adUnits": [{
                "name":"networks/1234567/adUnits/201",
                "parentAdUnit":"networks/1234567/adUnits/200",
                "parentPath":[{"parentAdUnit":"networks/1234567/adUnits/200"}],
                "status":"ARCHIVED"
            }]
        }))
        .is_none()
    );
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "complete_clear");
    assert_eq!(summary["page_count"], 2);
}

#[test]
fn sparse_or_cross_network_hierarchy_fails_closed() {
    let mut sparse = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", true)]),
        100,
    );
    sparse.consume_page(&json!({"adUnits":[
        {"name":"networks/1234567/adUnits/200","parentPath":[],"hasChildren":true},
        {"name":"networks/1234567/adUnits/201","parentAdUnit":"networks/1234567/adUnits/200","parentPath":[{"parentAdUnit":"networks/1234567/adUnits/200"}],"hasChildren":true},
        {"name":"networks/1234567/adUnits/202","parentAdUnit":"networks/1234567/adUnits/201","parentPath":[{"parentAdUnit":"networks/1234567/adUnits/201"}],"status":"ACTIVE"}
    ]}));
    let summary = sparse.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert_eq!(summary["ancestry_mismatch"], true);

    let mut cross_network = DescendantScan::new(
        "1234567",
        &["200".to_string()],
        &child_claims(&[("200", true)]),
        100,
    );
    cross_network.consume_page(&json!({"adUnits":[
        {"name":"networks/1234567/adUnits/200","parentPath":[],"hasChildren":true},
        {"name":"networks/9999999/adUnits/201","parentAdUnit":"networks/9999999/adUnits/200","parentPath":[{"parentAdUnit":"networks/9999999/adUnits/200"}],"status":"ACTIVE"}
    ]}));
    let summary = cross_network.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert_eq!(summary["invalid_network_resource"], true);
}

#[test]
fn missing_target_catalog_row_fails_closed() {
    let mut scan = DescendantScan::new("1234567", &["200".to_string()], &BTreeMap::new(), 100);
    scan.consume_page(&json!({
        "adUnits":[{"name":"networks/1234567/adUnits/300","parentPath":[],"hasChildren":false}]
    }));
    let summary = scan.finish(100);
    assert_eq!(summary["proof_state"], "partial_capped");
    assert_eq!(summary["missing_target_list_row"], true);
}

#[test]
fn evidence_is_network_source_target_and_freshness_bound() {
    let mut evidence = receipt(
        RetirementEvidenceSource::DeliveryReport,
        RetirementEvidenceState::CompleteClear,
        3_999_900,
    );
    let clear = grade_evidence(
        "delivery",
        RetirementEvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("clear evidence");
    assert_eq!(clear["state"], "complete_clear");

    evidence.source = RetirementEvidenceSource::Telemetry;
    let mismatch = grade_evidence(
        "delivery",
        RetirementEvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("mismatched evidence");
    assert_eq!(mismatch["state"], "invalid_binding");
    assert_eq!(mismatch["source_matches"], false);

    evidence.source = RetirementEvidenceSource::DeliveryReport;
    evidence.window_start_unix_seconds = None;
    evidence.window_end_unix_seconds = None;
    let unbound = grade_evidence(
        "delivery",
        RetirementEvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("missing activity window");
    assert_eq!(unbound["state"], "invalid_binding");
    assert_eq!(unbound["window_valid"], false);
}

#[test]
fn evidence_rejects_stale_or_noncanonical_bounded_fields() {
    let mut evidence = receipt(
        RetirementEvidenceSource::DeliveryReport,
        RetirementEvidenceState::CompleteClear,
        3_995_000,
    );
    evidence.ttl_seconds = Some(60);
    let stale = grade_evidence(
        "delivery",
        RetirementEvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("stale evidence");
    assert_eq!(stale["state"], "stale");

    evidence.observed_at_unix_seconds = Some(3_999_900);
    evidence.window_start_unix_seconds = Some(3_999_900 - MIN_ACTIVITY_WINDOW_SECONDS);
    evidence.window_end_unix_seconds = Some(3_999_900);
    evidence.ttl_seconds = Some(3_600);
    evidence.result_hash = Some(format!("{}sha256:0123456789abcdef", " ".repeat(1_024)));
    let untrusted = grade_evidence(
        "delivery",
        RetirementEvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("noncanonical hash");
    assert_eq!(untrusted["state"], "invalid_binding");
    assert_eq!(untrusted["result_hash"], Value::Null);
    assert!(response_bytes(&untrusted) < 4_096);
}

#[test]
fn activity_windows_must_be_recent_nonzero_and_at_least_thirty_days() {
    let mut evidence = receipt(
        RetirementEvidenceSource::DeliveryReport,
        RetirementEvidenceState::CompleteClear,
        3_999_900,
    );
    evidence.window_start_unix_seconds = Some(3_999_800);
    let short = grade_evidence(
        "delivery",
        RetirementEvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("short window should grade fail-closed");
    assert_eq!(short["state"], "invalid_binding");

    evidence.window_start_unix_seconds = Some(0);
    evidence.window_end_unix_seconds = Some(MIN_ACTIVITY_WINDOW_SECONDS);
    let old = grade_evidence(
        "delivery",
        RetirementEvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("old window should grade stale");
    assert_eq!(old["state"], "stale");

    evidence.window_start_unix_seconds = Some(3_999_900);
    evidence.window_end_unix_seconds = Some(3_999_900);
    let zero = grade_evidence(
        "delivery",
        RetirementEvidenceSource::DeliveryReport,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        false,
    )
    .expect("zero window should grade fail-closed");
    assert_eq!(zero["state"], "invalid_binding");
}

#[test]
fn source_probe_receipt_template_is_canonical_and_unverified() {
    let template = evidence_receipt_template(
        "1234567",
        RetirementEvidenceSource::DependencyProbe,
        RetirementEvidenceState::CompleteClear,
        "0123456789abcdef",
        vec!["200".to_string()],
    )
    .expect("receipt template");
    assert_eq!(template["network_code"], "1234567");
    assert_eq!(template["source"], "dependency_probe");
    assert_eq!(template["provenance"], "caller_supplied_unverified");
    assert_eq!(template["target_ad_unit_ids"][0], "200");
}

#[test]
fn protection_clear_requires_manual_ui_proof() {
    let mut evidence = receipt(
        RetirementEvidenceSource::ExchangeProtectionReview,
        RetirementEvidenceState::ManualUiProofRequired,
        3_999_900,
    );
    let graded = grade_evidence(
        "exchange_protection",
        RetirementEvidenceSource::ExchangeProtectionReview,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        true,
    )
    .expect("manual proof required");
    assert_eq!(graded["state"], "manual_ui_proof_required");
    evidence.manual_ui_proof_included = true;
    let graded = grade_evidence(
        "exchange_protection",
        RetirementEvidenceSource::ExchangeProtectionReview,
        Some(&evidence),
        "1234567",
        &["200".to_string()],
        4_000_000,
        true,
    )
    .expect("manual proof accepted");
    assert_eq!(graded["state"], "complete_clear");
    assert_eq!(graded["complete_for_summary"], true);
}

#[test]
fn duplicate_evidence_sources_are_rejected() {
    let dependency = receipt(
        RetirementEvidenceSource::DependencyProbe,
        RetirementEvidenceState::CompleteClear,
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
}

#[test]
fn assessment_fingerprint_changes_with_receipt_binding() {
    let identity = json!({"proof_state":"complete_clear","result_fingerprint":"i"});
    let descendants = json!({"proof_state":"complete_clear","descendant_result_fingerprint":"d"});
    let mut evidence = json!({
        "dependency":{"state":"complete_clear","result_hash":"hash-a"},
        "delivery":{"state":"complete_clear"},
        "exchange_protection":{"state":"complete_clear"},
        "site_contract":{"state":"complete_clear"},
        "telemetry":{"state":"complete_clear"}
    });
    let first = recommendation(&identity, &descendants, &evidence);
    evidence["dependency"]["result_hash"] = Value::String("hash-b".to_string());
    let second = recommendation(&identity, &descendants, &evidence);
    assert_ne!(
        first["assessment_fingerprint"],
        second["assessment_fingerprint"]
    );
}

#[test]
fn every_incomplete_state_prevents_evidence_completion() {
    for state in [
        "partial_capped",
        "blocked_permission",
        "blocked_read",
        "unsupported_surface",
        "invalid_binding",
        "stale",
        "not_run",
        "manual_ui_proof_required",
    ] {
        let mut evidence = json!({
            "dependency":{"state":"complete_clear"},
            "delivery":{"state":"complete_clear"},
            "exchange_protection":{"state":"complete_clear"},
            "site_contract":{"state":"complete_clear"},
            "telemetry":{"state":"complete_clear"}
        });
        evidence["delivery"]["state"] = Value::String(state.to_string());
        let result = recommendation(
            &json!({"proof_state":"complete_clear"}),
            &json!({"proof_state":"complete_clear"}),
            &evidence,
        );
        assert_eq!(result["evidence_summary_complete"], false, "{state}");
        assert_eq!(result["decision"], "not_eligible_incomplete_evidence");
    }
}

#[test]
fn positive_recommendation_uses_real_evidence_grader() {
    let mut dependency = receipt(
        RetirementEvidenceSource::DependencyProbe,
        RetirementEvidenceState::CompleteClear,
        3_999_900,
    );
    let delivery = receipt(
        RetirementEvidenceSource::DeliveryReport,
        RetirementEvidenceState::CompleteClear,
        3_999_900,
    );
    let mut protection = receipt(
        RetirementEvidenceSource::ExchangeProtectionReview,
        RetirementEvidenceState::CompleteClear,
        3_999_900,
    );
    protection.manual_ui_proof_included = true;
    let site_contract = receipt(
        RetirementEvidenceSource::SiteContract,
        RetirementEvidenceState::CompleteClear,
        3_999_900,
    );
    let telemetry = receipt(
        RetirementEvidenceSource::Telemetry,
        RetirementEvidenceState::CompleteClear,
        3_999_900,
    );
    dependency.result_hash = Some("dependency:0123456789abcdef".to_string());
    let evidence = grade_evidence_bundle(
        &[dependency, delivery, protection, site_contract, telemetry],
        "1234567",
        &["200".to_string()],
        4_000_000,
    )
    .expect("graded bundle");
    let result = recommendation(
        &json!({"proof_state":"complete_clear"}),
        &json!({
            "proof_state":"complete_clear",
            "requires_child_first_sequence":false,
            "required_child_first_target_order":["200"]
        }),
        &evidence,
    );
    assert_eq!(
        result["decision"],
        "evidence_complete_operator_review_required"
    );
    assert_eq!(result["evidence_summary_complete"], true);
    assert_eq!(result["automated_retirement_eligible"], false);
}

#[test]
fn response_size_guard_fails_closed() {
    let oversized = json!({"value": "x".repeat(MAX_INNER_DATA_BYTES)});
    assert!(ensure_response_size(&oversized).is_err());
}
