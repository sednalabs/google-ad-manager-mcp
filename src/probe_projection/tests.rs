use super::*;
use serde_json::Map;

use crate::evidence::{
    EVIDENCE_PRODUCER_CONTRACT_VERSION, MAX_CONTRACT_ENVELOPE_BYTES,
    MAX_RMCP_TRANSPORT_BYTES, validated_receipt_binding,
};

fn structured(result: CallToolResult) -> Value {
    result.structured_content.expect("structured result")
}

fn seal(mut data: Value, kind: ProbeKind, generated: bool) -> Value {
    let state = expected_receipt_state(kind, &data).expect("fixture evidence state");
    let hash = stable_fingerprint(&data.to_string());
    data["result_fingerprint"] = json!(hash);
    data["evidence_receipt_template"] = if generated {
        let ids = data["ad_units"].as_array().unwrap().iter()
            .map(|row| row["ad_unit_id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        json!({
            "network_code":"1234567","source":kind.evidence_source().as_str(),
            "source_version":EVIDENCE_PRODUCER_CONTRACT_VERSION,"state":state,
            "result_hash":hash,"observed_at_unix_seconds":123,"ttl_seconds":3600,
            "target_ad_unit_ids":ids,"provenance":"caller_supplied_unverified",
            "window_start_unix_seconds":null,"window_end_unix_seconds":null,
            "manual_ui_proof_included":false,
            "operator_action":"Preserve this synthetic producer result."
        })
    } else {
        json!({
            "source":kind.evidence_source().as_str(),
            "source_version":EVIDENCE_PRODUCER_CONTRACT_VERSION,
            "state":"not_generated",
            "reason":"fixture is not receipt eligible"
        })
    };
    data
}

fn rebind(mut data: Value) -> Value {
    let mut receipt = data
        .as_object_mut()
        .unwrap()
        .remove("evidence_receipt_template")
        .unwrap();
    data.as_object_mut().unwrap().remove("result_fingerprint");
    let hash = stable_fingerprint(&data.to_string());
    data["result_fingerprint"] = json!(hash);
    if receipt.get("result_hash").is_some() {
        receipt["result_hash"] = json!(hash);
    }
    data["evidence_receipt_template"] = receipt;
    data
}

fn ad_unit(id: usize, exchange: bool) -> Value {
    let mut row = json!({
        "ad_unit_id":id.to_string(),"resource_name":format!("networks/1234567/adUnits/{id}"),
        "proof_state":"resolved_exact"
    });
    if exchange {
        row["decision"] = json!("clear_on_exposed_flags");
        row["proof_complete"] = json!(true);
        row["applied_adsense_enabled"] = json!(false);
        row["effective_adsense_enabled"] = json!(false);
        row["explicitly_targeted"] = json!(true);
    }
    row
}

fn exchange(generated: bool, raw: usize) -> Value {
    let count = if generated { 1 } else { 50 };
    let target_ad_unit_ids = (1..=count).map(|id| id.to_string()).collect::<Vec<_>>();
    seal(json!({
        "network_code":"1234567","overall_decision":"partial_api_proof",
        "ad_units":(1..=count).map(|id| ad_unit(id,true)).collect::<Vec<_>>(),
        "private_auctions":{"collection":"private_auctions","proof_state":"complete_empty","row_count_in_page":0,"page_size":100,"next_page_token_present":false,"capped_or_possibly_more":false,"sample":[]},
        "private_auction_deals":{"collection":"private_auction_deals","proof_state":"complete_empty","row_count_in_page":0,"page_size":100,"next_page_token_present":false,"capped_or_possibly_more":false,"sample":[]},
        "yield_groups":{"surface":"yield_groups","decision":"no_target_matches","proof_state":"complete","request_id":"r","request_id_truncated":false,"response_time":"1","response_time_truncated":false,"total_result_set_size":0,"inspected_results":0,"response_truncated":false,"target_ad_unit_ids":target_ad_unit_ids,"target_ad_unit_matches":[],"targeted_exposed":[],"targeted_and_excluded":[],"targeted_inactive":[],"targeted_activity_unknown":[],"mutation_performed":false,"upstream_response_xml":"x".repeat(raw)},
        "rest_discovery":{"proof_state":"metadata_read","resource_count":1,"interesting_resources":["yieldGroups"]},
        "unsupported_or_unintegrated_surfaces":[{"surface":"protections","proof_state":"not_proven","api_exposure":"not_seen","note":"not integrated"}],
        "attention_reasons":[],"partial_reasons":["manual proof remains"],
        "certainty":{"can_prove_requested_ad_unit_flags":true,"can_prove_private_auction_absence_or_presence":true,"can_prove_private_deal_absence_or_presence":true,"can_prove_yield_group_targeting":true,"cannot_prove_via_current_api":["protections","inventory_rules","unified_pricing_rules"]}
    }), ProbeKind::ExchangeProtection, generated)
}

fn dependency(generated: bool, raw: usize) -> Value {
    let count = if generated { 1 } else { 50 };
    let xml_sample_bytes = raw.min(4_096);
    let target_map = (1..=count)
        .map(|id| {
            (
                id.to_string(),
                if id == 1 { json!(["10"]) } else { json!([]) },
            )
        })
        .collect::<Map<String, Value>>();
    seal(json!({
        "network_code":"1234567","dependency_decision":"dependencies_found",
        "ad_units":(1..=count).map(|id| ad_unit(id,false)).collect::<Vec<_>>(),
        "placements":{"surface":"placements","proof_state":"complete_for_page","row_count_in_page":1,"page_size":500,"next_page_token_present":false,"capped_or_possibly_more":false,"membership_shape_unknown_count":0,"membership_shape_unknown_sample":[],"target_placement_match_count":1,"target_placement_matches_truncated":false,"target_placement_id_limit_per_ad_unit":200,"target_placement_ids_truncated":false,"target_placement_ids_by_ad_unit_id":target_map,"target_placement_matches_sample":[{"status":"ACTIVE","matched_ad_unit_ids":["1"]}],"mutation_performed":false},
        "line_items":{"surface":"line_items","decision":"dependencies_found","proof_state":"blocked","total_result_set_size":2,"inspected_results":1,"max_line_items":1000,"line_item_page_size":500,"response_truncated":false,"missing_total_result_set_size":false,"request_ids":["r"],"request_id_count":1,"request_ids_truncated":false,"response_times":["1"],"response_time_count":1,"response_times_truncated":false,"transport_metadata_sample_limit":50,"status_counts":{"DELIVERING":1},"dependency_match_count":1,"dependency_matches_sample":[{"status":"DELIVERING","activity_state":"delivering","target_matches":[{"ad_unit_id":"1","classification":"exact_target","dependency_excluded":false}],"upstream_xml_sample":"x".repeat(xml_sample_bytes),"upstream_xml_truncated":raw > xml_sample_bytes,"upstream_xml_bytes":raw}],"dependency_matches_truncated":false,"dependency_match_sample_limit":50,"mutation_performed":false,"block_class":"upstream","message":"late read blocked","message_truncated":false},
        "target_resolution_issues":[],
        "proof_flags":{"target_resolution_incomplete":false,"id_only_targets_have_unknown_ancestors":false,"placements_capped_or_shape_unknown":false,"line_items_capped_or_truncated":true,"soap_manage_scope_required":false,"line_items_blocked":true},
        "mutation_performed":false,"cleanup_decision":{"safe_to_archive_or_retire":false,"reason":"separate review required"}
    }), ProbeKind::AdUnitDependency, generated)
}

fn reseal(mut full: Value, kind: ProbeKind, generated: bool) -> Value {
    full.as_object_mut().unwrap().remove("result_fingerprint");
    full.as_object_mut().unwrap().remove("evidence_receipt_template");
    seal(full, kind, generated)
}

fn skipped_dependency() -> Value {
    let mut full = dependency(false, 0);
    full["dependency_decision"] = json!("missing_or_ambiguous_targets");
    full["ad_units"] = json!([]);
    full["placements"] = json!({
        "surface":"placements","proof_state":"complete_for_page","row_count_in_page":0,
        "page_size":500,"next_page_token_present":false,"capped_or_possibly_more":false,
        "membership_shape_unknown_count":0,"membership_shape_unknown_sample":[],
        "target_placement_match_count":0,"target_placement_matches_truncated":false,
        "target_placement_id_limit_per_ad_unit":200,"target_placement_ids_truncated":false,
        "target_placement_ids_by_ad_unit_id":{},"target_placement_matches_sample":[],
        "mutation_performed":false
    });
    full["line_items"] = json!({
        "surface":"line_items","decision":"skipped","proof_state":"skipped",
        "reason":"no resolved ad-unit ids were available","mutation_performed":false
    });
    full["target_resolution_issues"] = Value::Array(
        (0..50)
            .map(|index| json!(format!("ad unit code unresolved-{index}-{} did not resolve exactly", "x".repeat(160))))
            .collect(),
    );
    full["proof_flags"] = json!({
        "target_resolution_incomplete":true,"id_only_targets_have_unknown_ancestors":false,
        "placements_capped_or_shape_unknown":false,"line_items_capped_or_truncated":true,
        "soap_manage_scope_required":false,"line_items_blocked":false
    });
    reseal(full, ProbeKind::AdUnitDependency, false)
}

fn permission_dependency() -> Value {
    let mut full = dependency(false, 0);
    full["dependency_decision"] = json!("blocked");
    for row in full["ad_units"].as_array_mut().unwrap() {
        row["ad_unit_codes"] = json!([]);
        row["resource_name"] = Value::Null;
        row["display_name"] = Value::Null;
        row["status"] = Value::Null;
        row["ad_unit_sizes"] = Value::Null;
        row["ancestor_ad_unit_ids"] = json!([]);
        row["proof_state"] = json!("id_only");
        row["proof_notes"] = json!([
            "ancestor targeting cannot be proven for an id-only target unless a code row is also resolved"
        ]);
    }
    for ids in full["placements"]["target_placement_ids_by_ad_unit_id"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        *ids = json!([]);
    }
    full["placements"]["target_placement_match_count"] = json!(0);
    full["placements"]["target_placement_matches_sample"] = json!([]);
    full["line_items"] = json!({
        "surface":"line_items","decision":"blocked","proof_state":"blocked",
        "block_class":"permission","reason":"manage scope required",
        "required_scope":"scope","current_scope":"readonly",
        "current_scope_truncated":false,"mutation_performed":false
    });
    full["proof_flags"] = json!({
        "target_resolution_incomplete":false,"id_only_targets_have_unknown_ancestors":true,
        "placements_capped_or_shape_unknown":false,"line_items_capped_or_truncated":true,
        "soap_manage_scope_required":true,"line_items_blocked":true
    });
    reseal(full, ProbeKind::AdUnitDependency, false)
}

#[test]
fn maximal_exchange_and_dependency_are_bounded_and_keep_receipt_state() {
        for (kind, full, binds) in [
            (ProbeKind::ExchangeProtection, exchange(true, 9_000), true),
            (
                ProbeKind::ExchangeProtection,
                exchange(false, 9_000),
                false,
            ),
            (ProbeKind::AdUnitDependency, dependency(true, 9_000), true),
            (ProbeKind::AdUnitDependency, dependency(false, 9_000), false),
        ] {
        let result = bounded_probe_success(
            kind, full, json!({"mutation_performed":false}), Instant::now(), "probe",
        );
        assert!(serde_json::to_vec(&result).unwrap().len() < MAX_RMCP_TRANSPORT_BYTES);
        let envelope = structured(result);
            assert_eq!(envelope["ok"], true);
            assert!(serde_json::to_vec(&envelope).unwrap().len() < MAX_CONTRACT_ENVELOPE_BYTES);
            assert_eq!(validated_receipt_binding(&envelope["data"]), Some(binds));
            assert!(envelope["data"].get("ad_units").is_none());
            if kind == ProbeKind::AdUnitDependency {
                assert_eq!(envelope["data"]["dependency_decision"], "dependencies_found");
                assert_eq!(envelope["data"]["line_items"]["proof_state"], "blocked");
                assert_eq!(envelope["data"]["proof_flags"]["line_items_blocked"], true);
                assert_eq!(
                    envelope["data"]["cleanup_decision"]["safe_to_archive_or_retire"],
                    false
                );
                assert_eq!(
                    envelope["data"]["line_items"]["upstream_xml_sample_summary"],
                    json!({
                        "sample_count": 1,
                        "source_bytes": 9_000,
                        "retained_bytes": 4_096,
                        "truncated_count": 1,
                    })
                );
            }
        }
}

#[test]
fn early_dependency_variants_compact_without_losing_their_proof_state() {
    for (full, decision, line_state) in [
        (
            skipped_dependency(),
            "missing_or_ambiguous_targets",
            "skipped",
        ),
        (permission_dependency(), "blocked", "blocked"),
    ] {
        assert!(serde_json::to_vec(&full).unwrap().len() > MAX_CONTRACT_ENVELOPE_BYTES);
        let result = bounded_probe_success(
            ProbeKind::AdUnitDependency,
            full,
            json!({"mutation_performed":false}),
            Instant::now(),
            "probe",
        );
        assert!(serde_json::to_vec(&result).unwrap().len() < MAX_RMCP_TRANSPORT_BYTES);
        let envelope = structured(result);
        assert_eq!(envelope["ok"], true);
        assert_eq!(envelope["data"]["dependency_decision"], decision);
        assert_eq!(envelope["data"]["line_items"]["proof_state"], line_state);
        assert_eq!(validated_receipt_binding(&envelope["data"]), Some(false));
    }
}

#[test]
fn unresolved_target_projection_keeps_a_bounded_actionable_witness() {
    let full = skipped_dependency();
    let compact = compact_success(
        ProbeKind::AdUnitDependency,
        &full,
        &json!({"mutation_performed":false}),
    )
    .unwrap();
    let issue_omission = compact["result_projection"]["omissions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == "/target_resolution_issues")
        .expect("target issue omission");
    assert_eq!(issue_omission["derived_witness_count"], 1);
    assert!(
        issue_omission["witness"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("unresolved-0"))
    );
}

#[test]
fn generated_and_not_generated_receipts_change_only_generated_hash() {
    for (kind, full) in [
        (ProbeKind::ExchangeProtection, exchange(true, 9_000)),
        (ProbeKind::AdUnitDependency, dependency(false, 9_000)),
    ] {
        let mut source = full["evidence_receipt_template"].clone();
        let compact = compact_success(kind, &full, &json!({"mutation_performed":false})).unwrap();
        let mut returned = compact["evidence_receipt_template"].clone();
        if kind == ProbeKind::ExchangeProtection {
            source.as_object_mut().unwrap().remove("result_hash");
            returned.as_object_mut().unwrap().remove("result_hash");
        }
        assert_eq!(source, returned);
    }
}

#[test]
fn malformed_source_receipts_fail_closed_before_projection() {
    let mut generated = exchange(true, 9_000);
    generated["evidence_receipt_template"]["ttl_seconds"] = json!(0);
    assert!(
        compact_success(
            ProbeKind::ExchangeProtection,
            &generated,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );

    let mut not_generated = dependency(false, 9_000);
    not_generated["evidence_receipt_template"]["unexpected"] = json!(true);
    assert!(
        compact_success(
            ProbeKind::AdUnitDependency,
            &not_generated,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );
}

#[test]
fn nested_evidence_cannot_escape_the_receipt_target_scope() {
    let mut exchange_full = exchange(true, 9_000);
    exchange_full["yield_groups"]["target_ad_unit_ids"] = json!(["1", "999"]);
    exchange_full = reseal(exchange_full, ProbeKind::ExchangeProtection, true);
    assert!(
        compact_success(
            ProbeKind::ExchangeProtection,
            &exchange_full,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );

    let mut dependency_full = dependency(true, 9_000);
    dependency_full["placements"]["target_placement_ids_by_ad_unit_id"]["999"] = json!([]);
    dependency_full = reseal(dependency_full, ProbeKind::AdUnitDependency, true);
    assert!(
        compact_success(
            ProbeKind::AdUnitDependency,
            &dependency_full,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );

    let mut dependency_sample = dependency(true, 9_000);
    dependency_sample["placements"]["target_placement_matches_sample"][0]
        ["matched_ad_unit_ids"] = json!(["999"]);
    dependency_sample = reseal(dependency_sample, ProbeKind::AdUnitDependency, true);
    assert!(
        compact_success(
            ProbeKind::AdUnitDependency,
            &dependency_sample,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );
}

#[test]
fn exchange_root_decision_must_match_the_retained_reason_surfaces() {
    let mut full = exchange(true, 9_000);
    full["overall_decision"] = json!("api_exposed_surfaces_clear");
    full = reseal(full, ProbeKind::ExchangeProtection, true);
    assert!(
        compact_success(
            ProbeKind::ExchangeProtection,
            &full,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );
}

#[test]
fn dependency_root_decision_must_match_the_retained_surfaces() {
    let mut full = dependency(true, 9_000);
    full["dependency_decision"] = json!("no_dependencies_observed");
    full = reseal(full, ProbeKind::AdUnitDependency, true);
    assert!(
        compact_success(
            ProbeKind::AdUnitDependency,
            &full,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );
}

#[test]
fn inconsistent_transport_metadata_counts_fail_closed() {
    let mut full = dependency(true, 9_000);
    full["line_items"]["request_id_count"] = json!(0);
    full.as_object_mut().unwrap().remove("result_fingerprint");
    full.as_object_mut()
        .unwrap()
        .remove("evidence_receipt_template");
    full = seal(full, ProbeKind::AdUnitDependency, true);

    assert!(
        compact_success(
            ProbeKind::AdUnitDependency,
            &full,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );

    let mut full = dependency(true, 9_000);
    full["line_items"]["request_id_count"] = json!(10);
    full["line_items"]["request_ids_truncated"] = json!(true);
    full = reseal(full, ProbeKind::AdUnitDependency, true);
    assert!(
        compact_success(
            ProbeKind::AdUnitDependency,
            &full,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );
}

#[test]
fn small_success_is_unchanged() {
    for (kind, full) in [
        (ProbeKind::ExchangeProtection, exchange(true, 0)),
        (ProbeKind::AdUnitDependency, dependency(true, 0)),
    ] {
        let envelope = structured(bounded_probe_success(
            kind,
            full.clone(),
            json!({"mutation_performed":false}),
            Instant::now(),
            "probe",
        ));
        assert_eq!(envelope["data"], full);
        assert!(envelope["data"].get("result_projection").is_none());
    }
}

#[test]
fn semantic_tamper_and_off_by_one_ledger_are_rejected() {
    let full = exchange(true, 9_000);
    let receipt = verify_receipt(ProbeKind::ExchangeProtection, &full).unwrap();
    let mut compact = compact_success(
        ProbeKind::ExchangeProtection, &full, &json!({"mutation_performed":false}),
    ).unwrap();
    compact["overall_decision"] = json!("attention_required");
    compact = rebind(compact);
    assert_eq!(validated_receipt_binding(&compact), Some(true));
    assert!(validate_projection(ProbeKind::ExchangeProtection, &full, &compact, &receipt).is_err());
    let mut compact = compact_success(
        ProbeKind::ExchangeProtection, &full, &json!({"mutation_performed":false}),
    ).unwrap();
    compact["result_projection"]["omissions"][0]["source_count"] = json!(2);
    compact = rebind(compact);
    assert_eq!(validated_receipt_binding(&compact), Some(true));
    assert!(validate_projection(ProbeKind::ExchangeProtection, &full, &compact, &receipt).is_err());

    let full = dependency(true, 9_000);
    let receipt = verify_receipt(ProbeKind::AdUnitDependency, &full).unwrap();
    let mut compact = compact_success(
        ProbeKind::AdUnitDependency, &full, &json!({"mutation_performed":false}),
    ).unwrap();
    compact["proof_flags"]["line_items_blocked"] = json!(false);
    compact["cleanup_decision"]["safe_to_archive_or_retire"] = json!(true);
    compact = rebind(compact);
    assert_eq!(validated_receipt_binding(&compact), Some(true));
    assert!(validate_projection(ProbeKind::AdUnitDependency, &full, &compact, &receipt).is_err());
}

#[test]
fn oversized_error_is_redacted_utf8_safe_and_byte_exact() {
    let error = AdManagerError::AuthBootstrap(format!(
        "access_token=secret {} {}", "€".repeat(3_000), "\\\"".repeat(2_000)
    ));
    let result = bounded_probe_error(
        ProbeKind::ExchangeProtection, error, Instant::now(), "probe",
    );
    assert!(serde_json::to_vec(&result).unwrap().len() < MAX_RMCP_TRANSPORT_BYTES);
    let envelope = structured(result);
    assert_eq!(envelope["error"]["code"], "auth_bootstrap");
    assert_eq!(
        envelope["meta"]["result_projection"]["version"],
        PROJECTION_VERSION
    );
    assert_eq!(envelope["meta"]["result_projection"]["truncated"], true);
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(message.is_char_boundary(message.len()));
    assert!(!message.contains("secret"));
    let row = &envelope["meta"]["result_projection"]["omissions"][0];
    assert_eq!(row["source_count"].as_u64().unwrap(), row["retained_count"].as_u64().unwrap() + row["omitted_count"].as_u64().unwrap());
    assert_eq!(row["retained_count"], json!(message.len()));
}

#[test]
fn compact_oversize_fails_closed() {
    let mut full = exchange(true, 9_000);
    full["unsupported_or_unintegrated_surfaces"][0]["note"] = json!("x".repeat(9_000));
    full.as_object_mut().unwrap().remove("result_fingerprint");
    full.as_object_mut().unwrap().remove("evidence_receipt_template");
    full = seal(full, ProbeKind::ExchangeProtection, true);
    let envelope = structured(bounded_probe_success(
        ProbeKind::ExchangeProtection, full, json!({"mutation_performed":false}),
        Instant::now(), "probe",
    ));
    assert_eq!(envelope["error"]["code"], "result_contract_error");
}
