use super::*;
use serde_json::Map;

use crate::evidence::{
    EVIDENCE_PRODUCER_CONTRACT_VERSION, MAX_CONTRACT_ENVELOPE_BYTES, MAX_RMCP_TRANSPORT_BYTES,
    validated_receipt_binding,
};

#[path = "tests/exchange.rs"]
mod exchange_cases;

fn structured(result: CallToolResult) -> Value {
    result.structured_content.expect("structured result")
}

fn seal(mut data: Value, kind: ProbeKind, generated: bool) -> Value {
    let state = expected_receipt_state(kind, &data).expect("fixture evidence state");
    let hash = stable_fingerprint(&data.to_string());
    data["result_fingerprint"] = json!(hash);
    data["evidence_receipt_template"] = if generated {
        let mut ids = data["ad_units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["ad_unit_id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        ids.sort();
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
        row["ad_unit_code"] = json!(format!("unit-{id}"));
        row["ancestor_ad_unit_ids"] = json!([]);
        row["ancestor_identity_complete"] = json!(true);
        row["display_name"] = json!(format!("Inventory unit {id}"));
        row["status"] = json!("ACTIVE");
        row["ad_unit_sizes"] = json!([]);
        row["decision"] = json!("clear_on_exposed_flags");
        row["proof_complete"] = json!(true);
        row["applied_adsense_enabled"] = json!(false);
        row["effective_adsense_enabled"] = json!(false);
        row["explicitly_targeted"] = json!(true);
    } else {
        row["ad_unit_code"] = json!(format!("unit-{id}"));
        row["display_name"] = json!(format!("Inventory unit {id}"));
        row["status"] = json!("ACTIVE");
        row["ad_unit_sizes"] = json!([
            {"width": 160, "height": 600},
            {"width": 160, "height": 1200}
        ]);
        row["ancestor_ad_unit_ids"] = json!([]);
        row["ancestor_identity_complete"] = json!(true);
    }
    row
}

fn exchange(generated: bool, raw: usize) -> Value {
    let count = if generated { 1 } else { 50 };
    let target_ad_unit_ids = (1..=count).map(|id| id.to_string()).collect::<Vec<_>>();
    seal(
        json!({
            "network_code":"1234567","overall_decision":"partial_api_proof",
            "ad_units":(1..=count).map(|id| ad_unit(id,true)).collect::<Vec<_>>(),
            "private_auctions":{"collection":"private_auctions","proof_state":"complete_empty","row_count_in_page":0,"page_size":100,"next_page_token_present":false,"capped_or_possibly_more":false,"sample":[]},
            "private_auction_deals":{"collection":"private_auction_deals","proof_state":"complete_empty","row_count_in_page":0,"page_size":100,"next_page_token_present":false,"capped_or_possibly_more":false,"sample":[]},
            "yield_groups":{"surface":"yield_groups","decision":"no_target_matches","proof_state":"complete","request_id":"r","request_id_truncated":false,"response_time":"1","response_time_truncated":false,"total_result_set_size":0,"inspected_results":0,"response_truncated":false,"target_ad_unit_ids":target_ad_unit_ids,"target_ad_unit_matches":[],"targeted_exposed":[],"targeted_and_excluded":[],"targeted_inactive":[],"targeted_activity_unknown":[],"mutation_performed":false,"upstream_response_xml":"x".repeat(raw)},
            "rest_discovery":{"proof_state":"metadata_read","resource_count":1,"interesting_resources":["yieldGroups"]},
            "unsupported_or_unintegrated_surfaces":[
                {"surface":"protections","proof_state":"not_proven","api_exposure":"not_seen_in_rest_discovery","note":"GAM protection objects are not implemented as a current MCP read surface."},
                {"surface":"inventory_rules","proof_state":"not_proven","api_exposure":"not_seen_in_rest_discovery","note":"GAM inventory-rule objects are not implemented as a current MCP read surface."},
                {"surface":"unified_pricing_rules","proof_state":"not_proven","api_exposure":"not_seen_in_rest_discovery","note":"GAM unified pricing rules are not implemented as a current MCP read surface."}
            ],
            "attention_reasons":[],"partial_reasons":["manual proof remains"],
            "certainty":{"can_prove_requested_ad_unit_flags":true,"can_prove_private_auction_absence_or_presence":true,"can_prove_private_deal_absence_or_presence":true,"can_prove_yield_group_targeting":true,"cannot_prove_via_current_api":["protections","inventory_rules","unified_pricing_rules"]}
        }),
        ProbeKind::ExchangeProtection,
        generated,
    )
}

fn dependency(generated: bool, raw: usize) -> Value {
    let count = match (generated, raw) {
        (true, 0) => 1,
        (true, _) => 10,
        (false, _) => 50,
    };
    let xml_sample_bytes = raw.min(4_096);
    let target_map = (1..=count)
        .map(|id| {
            (
                id.to_string(),
                if id == 1 { json!(["10"]) } else { json!([]) },
            )
        })
        .collect::<Map<String, Value>>();
    seal(
        json!({
            "network_code":"1234567","dependency_decision":"dependencies_found",
            "ad_units":(1..=count).map(|id| ad_unit(id,false)).collect::<Vec<_>>(),
            "placements":{"surface":"placements","proof_state":"complete_for_page","row_count_in_page":1,"page_size":500,"next_page_token_present":false,"capped_or_possibly_more":false,"membership_shape_unknown_count":0,"membership_shape_unknown_sample":[],"target_placement_match_count":1,"target_placement_matches_truncated":false,"target_placement_id_limit_per_ad_unit":200,"target_placement_ids_truncated":false,"target_placement_ids_by_ad_unit_id":target_map,"target_placement_matches_sample":[{"placement_id":"10","status":"ACTIVE","matched_ad_unit_ids":["1"]}],"mutation_performed":false},
            "line_items":{"surface":"line_items","decision":"dependencies_found","proof_state":"blocked","total_result_set_size":2,"inspected_results":1,"max_line_items":1000,"line_item_page_size":500,"response_truncated":false,"missing_total_result_set_size":false,"request_ids":["r"],"request_id_count":1,"request_ids_truncated":false,"response_times":["1"],"response_time_count":1,"response_times_truncated":false,"transport_metadata_sample_limit":50,"status_counts":{"DELIVERING":1},"dependency_match_count":1,"dependency_matches_sample":[{"status":"DELIVERING","activity_state":"delivering","target_matches":[{"ad_unit_id":"1","ad_unit_codes":["unit-1"],"classification":"exact_target","targeting_match":{"ad_unit_id":"1","include_descendants":false,"match_type":"exact"},"exclusion_match":null,"matched_placement_ids":[],"root_or_network_targeting":false,"dependency_excluded":false}],"upstream_xml_sample":"x".repeat(xml_sample_bytes),"upstream_xml_truncated":raw > xml_sample_bytes,"upstream_xml_bytes":raw}],"dependency_matches_truncated":false,"dependency_match_sample_limit":50,"mutation_performed":false,"block_class":"upstream","upstream_status":503,"request_id":"fault-r","request_id_truncated":false,"response_time":"2","response_time_truncated":false,"soap_fault":"ServerError.SERVER_ERROR","soap_fault_truncated":false,"message":"late read blocked","message_truncated":false},
            "target_resolution_issues":[],
            "proof_flags":{"target_resolution_incomplete":false,"id_only_targets_have_unknown_ancestors":false,"placements_capped_or_shape_unknown":false,"line_items_capped_or_truncated":true,"soap_manage_scope_required":false,"line_items_blocked":true},
            "mutation_performed":false,"cleanup_decision":{"safe_to_archive_or_retire":false,"reason":"separate review required"}
        }),
        ProbeKind::AdUnitDependency,
        generated,
    )
}

fn reseal(mut full: Value, kind: ProbeKind, generated: bool) -> Value {
    full.as_object_mut().unwrap().remove("result_fingerprint");
    full.as_object_mut()
        .unwrap()
        .remove("evidence_receipt_template");
    seal(full, kind, generated)
}

fn skipped_dependency() -> Value {
    let mut full = dependency(false, 0);
    let unresolved_codes = (0..50)
        .map(|index| format!("unresolved-{index}-{}", "x".repeat(80)))
        .collect::<Vec<_>>();
    full["dependency_decision"] = json!("missing_or_ambiguous_targets");
    full["ad_units"] = Value::Array(
        unresolved_codes
            .iter()
            .map(|code| {
                json!({
                    "ad_unit_code":code,
                    "proof_state":"missing",
                    "reason":"exact ad-unit code was not returned by GAM",
                    "matches":0
                })
            })
            .collect(),
    );
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
        unresolved_codes
            .iter()
            .map(|code| json!(format!("ad unit code {code} did not resolve exactly")))
            .collect(),
    );
    full["proof_flags"] = json!({
        "target_resolution_incomplete":true,"id_only_targets_have_unknown_ancestors":false,
        "placements_capped_or_shape_unknown":false,"line_items_capped_or_truncated":false,
        "soap_manage_scope_required":false,"line_items_blocked":false
    });
    reseal(full, ProbeKind::AdUnitDependency, false)
}

fn unresolved_code_variant_dependency() -> Value {
    let mut full = skipped_dependency();
    full["ad_units"] = json!([
        {
            "ad_unit_code":"missing-code",
            "proof_state":"missing",
            "reason":"exact ad-unit code was not returned by GAM",
            "matches":0
        },
        {
            "ad_unit_code":"ambiguous-code",
            "proof_state":"ambiguous",
            "reason":"GAM returned multiple rows for the exact ad-unit code",
            "matches":2
        },
        {
            "ad_unit_code":"invalid-resource-code",
            "ad_unit_id":null,
            "resource_name":"networks/7654321/adUnits/7",
            "display_name":null,
            "status":null,
            "ad_unit_sizes":null,
            "ancestor_ad_unit_ids":[],
            "ancestor_identity_complete":true,
            "proof_state":"invalid_resource_name",
            "reason":"exact ad-unit code resolved outside the requested canonical network/resource scope"
        }
    ]);
    full["target_resolution_issues"] = json!([
        "ad unit code missing-code did not resolve exactly",
        "ad unit code ambiguous-code did not resolve exactly",
        "ad unit code invalid-resource-code did not resolve exactly"
    ]);
    reseal(full, ProbeKind::AdUnitDependency, false)
}

fn permission_dependency() -> Value {
    let mut full = dependency(false, 0);
    full["dependency_decision"] = json!("blocked");
    for row in full["ad_units"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("ad_unit_code");
        row.as_object_mut()
            .unwrap()
            .remove("ancestor_identity_complete");
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
    let target_resolution_issues = full["ad_units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let id = row["ad_unit_id"].as_str().unwrap();
            json!(format!(
                "ad unit id {id} was supplied without a resolved code row; ancestor targeting proof is incomplete"
            ))
        })
        .collect::<Vec<_>>();
    full["target_resolution_issues"] = Value::Array(target_resolution_issues);
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
        "target_resolution_incomplete":true,"id_only_targets_have_unknown_ancestors":true,
        "placements_capped_or_shape_unknown":false,"line_items_capped_or_truncated":false,
        "soap_manage_scope_required":true,"line_items_blocked":true
    });
    reseal(full, ProbeKind::AdUnitDependency, false)
}

fn completed_dependency(sample_only: bool) -> Value {
    let mut full = dependency(true, 9_000);
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
        "surface":"line_items",
        "decision":if sample_only { "no_dependencies_in_sample" } else { "no_dependencies_observed" },
        "proof_state":if sample_only { "sample_only" } else { "complete" },
        "total_result_set_size":if sample_only { 2 } else { 1 },
        "inspected_results":1,
        "max_line_items":1000,
        "line_item_page_size":500,
        "response_truncated":false,
        "missing_total_result_set_size":false,
        "request_ids":["r"],
        "request_id_count":1,
        "request_ids_truncated":false,
        "response_times":["1"],
        "response_time_count":1,
        "response_times_truncated":false,
        "transport_metadata_sample_limit":50,
        "status_counts":{"PAUSED":1},
        "dependency_match_count":0,
        "dependency_matches_sample":[],
        "dependency_matches_truncated":false,
        "dependency_match_sample_limit":50,
        "mutation_performed":false
    });
    full["dependency_decision"] = json!(if sample_only {
        "incomplete_no_dependencies_observed"
    } else {
        "no_dependencies_observed"
    });
    full["proof_flags"] = json!({
        "target_resolution_incomplete":false,
        "id_only_targets_have_unknown_ancestors":false,
        "placements_capped_or_shape_unknown":false,
        "line_items_capped_or_truncated":sample_only,
        "soap_manage_scope_required":false,
        "line_items_blocked":false
    });
    reseal(full, ProbeKind::AdUnitDependency, true)
}

fn ancestor_incomplete_dependency() -> Value {
    let mut full = completed_dependency(false);
    full["ad_units"][0]["ancestor_identity_complete"] = json!(false);
    full["target_resolution_issues"] =
        json!(["ad unit code unit-1 returned malformed or foreign ancestor identities"]);
    full["proof_flags"]["target_resolution_incomplete"] = json!(true);
    full["dependency_decision"] = json!("missing_or_ambiguous_targets");
    reseal(full, ProbeKind::AdUnitDependency, false)
}

fn late_error_dependency() -> Value {
    let mut full = dependency(true, 9_000);
    let line_items = full["line_items"].as_object_mut().unwrap();
    for field in [
        "upstream_status",
        "request_id",
        "request_id_truncated",
        "response_time",
        "response_time_truncated",
        "soap_fault",
        "soap_fault_truncated",
        "message",
        "message_truncated",
    ] {
        line_items.remove(field);
    }
    line_items.insert("error".into(), json!("later page unavailable"));
    line_items.insert("error_truncated".into(), json!(false));
    line_items.insert("hint".into(), json!("retry the read"));
    line_items.insert("hint_truncated".into(), json!(false));
    reseal(full, ProbeKind::AdUnitDependency, true)
}

fn generic_blocked_dependency() -> Value {
    let mut full = permission_dependency();
    full["line_items"] = json!({
        "surface":"line_items",
        "proof_state":"blocked",
        "block_class":"upstream",
        "error":"line-item read could not start",
        "error_truncated":false,
        "hint":"retry the read",
        "hint_truncated":false
    });
    full["proof_flags"]["soap_manage_scope_required"] = json!(false);
    reseal(full, ProbeKind::AdUnitDependency, false)
}

fn blocked_placement_dependency() -> Value {
    let mut full = dependency(true, 9_000);
    full["placements"] = json!({
        "surface":"placements",
        "proof_state":"blocked",
        "block_class":"upstream",
        "error":"placement read failed",
        "error_truncated":false,
        "hint":"retry the read",
        "hint_truncated":false
    });
    full["proof_flags"]["placements_capped_or_shape_unknown"] = json!(true);
    reseal(full, ProbeKind::AdUnitDependency, true)
}

#[test]
fn maximal_exchange_and_dependency_are_bounded_and_keep_receipt_state() {
    for (kind, full, binds) in [
        (ProbeKind::ExchangeProtection, exchange(true, 9_000), true),
        (ProbeKind::ExchangeProtection, exchange(false, 9_000), false),
        (ProbeKind::AdUnitDependency, dependency(true, 9_000), true),
        (ProbeKind::AdUnitDependency, dependency(false, 9_000), false),
    ] {
        let meta = json!({"mutation_performed":false});
        let native_envelope =
            contract::success_envelope_with_meta(full.clone(), meta.clone(), Instant::now());
        assert!(serde_json::to_vec(&native_envelope).unwrap().len() > MAX_CONTRACT_ENVELOPE_BYTES);
        let result = bounded_probe_success(kind, full, meta, Instant::now(), "probe");
        assert!(serde_json::to_vec(&result).unwrap().len() < MAX_RMCP_TRANSPORT_BYTES);
        let envelope = structured(result);
        assert_eq!(envelope["ok"], true);
        assert!(serde_json::to_vec(&envelope).unwrap().len() < MAX_CONTRACT_ENVELOPE_BYTES);
        assert_eq!(validated_receipt_binding(&envelope["data"]), Some(binds));
        assert!(envelope["data"].get("ad_units").is_none());
        if kind == ProbeKind::AdUnitDependency {
            assert_eq!(
                envelope["data"]["dependency_decision"],
                "dependencies_found"
            );
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
            let omission = envelope["data"]["result_projection"]["omissions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| {
                    row["path"] == "/line_items/dependency_matches_sample/*/upstream_xml_sample"
                })
                .expect("XML omission");
            let omitted_samples = json!(["x".repeat(4_096)]);
            assert_eq!(omission["source_count"], 4_096);
            assert_eq!(omission["retained_count"], 0);
            assert_eq!(omission["omitted_count"], 4_096);
            assert_eq!(
                omission["source_value_fingerprint"],
                stable_fingerprint(&omitted_samples.to_string())
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
        (generic_blocked_dependency(), "blocked", "blocked"),
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
fn producer_shaped_late_line_item_blocks_compact() {
    for full in [dependency(true, 9_000), late_error_dependency()] {
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_ok()
        );
    }
}

#[test]
fn dependency_ad_unit_variants_and_ancestor_eligibility_match_the_producer() {
    let ancestor_incomplete = ancestor_incomplete_dependency();
    assert_eq!(validated_receipt_binding(&ancestor_incomplete), Some(false));
    for full in [
        dependency(true, 9_000),
        unresolved_code_variant_dependency(),
        permission_dependency(),
        ancestor_incomplete,
    ] {
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_ok()
        );
    }

    let mut resolved_with_notes = dependency(true, 9_000);
    resolved_with_notes["ad_units"][0]["proof_notes"] = json!([]);

    let mut incomplete_without_issue = dependency(true, 9_000);
    incomplete_without_issue["ad_units"][0]["ancestor_identity_complete"] = json!(false);

    let mut complete_with_issue = dependency(true, 9_000);
    complete_with_issue["target_resolution_issues"] =
        json!(["ad unit code unit-1 returned malformed or foreign ancestor identities"]);
    complete_with_issue["proof_flags"]["target_resolution_incomplete"] = json!(true);

    let mut missing_with_match = unresolved_code_variant_dependency();
    missing_with_match["ad_units"][0]["matches"] = json!(1);

    let mut id_only_without_note = permission_dependency();
    id_only_without_note["ad_units"][0]
        .as_object_mut()
        .unwrap()
        .remove("proof_notes");

    let mut empty_targets = dependency(true, 9_000);
    empty_targets["ad_units"] = json!([]);
    empty_targets["placements"]["target_placement_ids_by_ad_unit_id"] = json!({});
    empty_targets["placements"]["target_placement_match_count"] = json!(0);
    empty_targets["placements"]["target_placement_matches_sample"] = json!([]);
    empty_targets["line_items"] = skipped_dependency()["line_items"].clone();
    empty_targets["dependency_decision"] = json!("no_dependencies_observed");

    for (full, generated) in [
        (resolved_with_notes, true),
        (incomplete_without_issue, true),
        (complete_with_issue, false),
        (missing_with_match, false),
        (id_only_without_note, false),
        (empty_targets, false),
    ] {
        let full = reseal(full, ProbeKind::AdUnitDependency, generated);
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }
}

#[test]
fn placement_state_and_variant_shape_match_the_producer() {
    let mut capped = dependency(true, 9_000);
    capped["placements"]["page_size"] = json!(1);
    capped["placements"]["capped_or_possibly_more"] = json!(true);
    capped["placements"]["proof_state"] = json!("sample_or_shape_incomplete");
    capped["proof_flags"]["placements_capped_or_shape_unknown"] = json!(true);
    capped = reseal(capped, ProbeKind::AdUnitDependency, true);

    for full in [
        dependency(true, 9_000),
        capped,
        blocked_placement_dependency(),
    ] {
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_ok()
        );
    }

    let mut capped_but_complete = dependency(true, 9_000);
    capped_but_complete["placements"]["page_size"] = json!(1);
    capped_but_complete["placements"]["capped_or_possibly_more"] = json!(true);

    let mut next_page_without_cap = dependency(true, 9_000);
    next_page_without_cap["placements"]["next_page_token_present"] = json!(true);
    next_page_without_cap["placements"]["proof_state"] = json!("sample_or_shape_incomplete");
    next_page_without_cap["proof_flags"]["placements_capped_or_shape_unknown"] = json!(true);

    let mut unknown_but_complete = dependency(true, 9_000);
    unknown_but_complete["placements"]["membership_shape_unknown_count"] = json!(1);
    unknown_but_complete["placements"]["membership_shape_unknown_sample"] = json!([{
        "placement_id":null,
        "resource_name":"",
        "display_name":null,
        "reason":"placement membership shape was not exposed"
    }]);

    let mut truncated_but_complete = dependency(true, 9_000);
    truncated_but_complete["placements"]["target_placement_ids_truncated"] = json!(true);

    let mut normal_block_hybrid = dependency(true, 9_000);
    normal_block_hybrid["placements"]["error"] = json!("unexpected diagnostic");
    normal_block_hybrid["placements"]["error_truncated"] = json!(false);
    normal_block_hybrid["placements"]["hint"] = json!("unexpected hint");
    normal_block_hybrid["placements"]["hint_truncated"] = json!(false);

    let mut blocked_normal_hybrid = blocked_placement_dependency();
    blocked_normal_hybrid["placements"]["row_count_in_page"] = json!(0);

    let mut oversized_page = dependency(true, 9_000);
    oversized_page["placements"]["page_size"] = json!(1_001);

    let mut excessive_match_count = dependency(true, 9_000);
    excessive_match_count["placements"]["target_placement_match_count"] = json!(2);

    let mut changed_target_limit = dependency(true, 9_000);
    changed_target_limit["placements"]["target_placement_id_limit_per_ad_unit"] = json!(201);

    let mut unsupported_truncation = dependency(true, 9_000);
    unsupported_truncation["placements"]["target_placement_ids_truncated"] = json!(true);
    unsupported_truncation["placements"]["proof_state"] = json!("sample_or_shape_incomplete");
    unsupported_truncation["proof_flags"]["placements_capped_or_shape_unknown"] = json!(true);

    for full in [
        capped_but_complete,
        next_page_without_cap,
        unknown_but_complete,
        truncated_but_complete,
        normal_block_hybrid,
        blocked_normal_hybrid,
        oversized_page,
        excessive_match_count,
        changed_target_limit,
        unsupported_truncation,
    ] {
        let full = reseal(full, ProbeKind::AdUnitDependency, true);
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
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
fn contradictory_dependency_source_semantics_fail_closed() {
    let mut proof_flags = dependency(true, 9_000);
    proof_flags["proof_flags"]["line_items_blocked"] = json!(false);
    proof_flags = reseal(proof_flags, ProbeKind::AdUnitDependency, true);

    let mut cleanup = dependency(true, 9_000);
    cleanup["cleanup_decision"]["safe_to_archive_or_retire"] = json!(true);
    cleanup = reseal(cleanup, ProbeKind::AdUnitDependency, true);

    let mut incomplete_xml = dependency(true, 9_000);
    incomplete_xml["line_items"]["dependency_matches_sample"][0]
        .as_object_mut()
        .unwrap()
        .remove("upstream_xml_bytes");
    incomplete_xml = reseal(incomplete_xml, ProbeKind::AdUnitDependency, true);

    let mut impossible_xml = dependency(true, 9_000);
    impossible_xml["line_items"]["dependency_matches_sample"][0]["upstream_xml_bytes"] = json!(100);
    impossible_xml = reseal(impossible_xml, ProbeKind::AdUnitDependency, true);

    for full in [proof_flags, cleanup, incomplete_xml, impossible_xml] {
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }
}

#[test]
fn line_item_variant_hybrids_fail_closed() {
    let mut completed_with_error = completed_dependency(false);
    completed_with_error["line_items"]["error"] = json!("unexpected diagnostic");
    completed_with_error["line_items"]["error_truncated"] = json!(false);
    completed_with_error["line_items"]["hint"] = json!("unexpected hint");
    completed_with_error["line_items"]["hint_truncated"] = json!(false);

    let mut skipped_with_progress = skipped_dependency();
    skipped_with_progress["line_items"]["inspected_results"] = json!(0);

    let mut generic_permission_hybrid = generic_blocked_dependency();
    generic_permission_hybrid["line_items"]["decision"] = json!("blocked");
    generic_permission_hybrid["line_items"]["reason"] = json!("manage scope required");
    generic_permission_hybrid["line_items"]["required_scope"] = json!("scope");
    generic_permission_hybrid["line_items"]["current_scope"] = json!("readonly");
    generic_permission_hybrid["line_items"]["current_scope_truncated"] = json!(false);
    generic_permission_hybrid["line_items"]["mutation_performed"] = json!(false);
    generic_permission_hybrid["line_items"]["block_class"] = json!("permission");
    generic_permission_hybrid["proof_flags"]["soap_manage_scope_required"] = json!(true);

    let mut permission_with_soap_status = permission_dependency();
    permission_with_soap_status["line_items"]["upstream_status"] = json!(403);

    let mut error_with_soap_status = late_error_dependency();
    error_with_soap_status["line_items"]["upstream_status"] = json!(503);

    let mut soap_with_error = dependency(true, 9_000);
    soap_with_error["line_items"]["error"] = json!("duplicate diagnostic channel");
    soap_with_error["line_items"]["error_truncated"] = json!(false);
    soap_with_error["line_items"]["hint"] = json!("duplicate hint channel");
    soap_with_error["line_items"]["hint_truncated"] = json!(false);

    let mut permission_fault_with_upstream_class = dependency(true, 9_000);
    permission_fault_with_upstream_class["line_items"]["soap_fault"] =
        json!("PermissionError.PERMISSION_DENIED");

    let mut classification_drift = dependency(true, 9_000);
    classification_drift["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["classification"] =
        json!("placement_target");

    let mut exclusion_drift = dependency(true, 9_000);
    exclusion_drift["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["dependency_excluded"] =
        json!(true);

    let mut duplicate_target = dependency(true, 9_000);
    let repeated =
        duplicate_target["line_items"]["dependency_matches_sample"][0]["target_matches"][0].clone();
    duplicate_target["line_items"]["dependency_matches_sample"][0]["target_matches"] =
        json!([repeated.clone(), repeated]);

    let mut exact_coverage_drift = dependency(true, 9_000);
    exact_coverage_drift["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["targeting_match"]
        ["ad_unit_id"] = json!("999");

    let mut ancestor_coverage_drift = dependency(true, 9_000);
    ancestor_coverage_drift["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["targeting_match"] = json!({
        "ad_unit_id":"10","include_descendants":true,"match_type":"ancestor_descendant"
    });
    ancestor_coverage_drift["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["classification"] =
        json!("ancestor_descendant_target");

    let mut placement_coverage_drift = dependency(true, 9_000);
    placement_coverage_drift["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["targeting_match"] =
        Value::Null;
    placement_coverage_drift["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["matched_placement_ids"] =
        json!(["11"]);
    placement_coverage_drift["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["classification"] =
        json!("placement_target");

    for (full, generated) in [
        (completed_with_error, true),
        (skipped_with_progress, false),
        (generic_permission_hybrid, false),
        (permission_with_soap_status, false),
        (error_with_soap_status, true),
        (soap_with_error, true),
        (permission_fault_with_upstream_class, true),
        (classification_drift, true),
        (exclusion_drift, true),
        (duplicate_target, true),
        (exact_coverage_drift, true),
        (ancestor_coverage_drift, true),
        (placement_coverage_drift, true),
    ] {
        let full = reseal(full, ProbeKind::AdUnitDependency, generated);
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }
}

#[test]
fn completed_line_item_outcomes_are_rederived_from_scan_progress() {
    let mut complete_with_match = dependency(true, 9_000);
    complete_with_match["line_items"]["proof_state"] = json!("complete");
    complete_with_match["line_items"]["total_result_set_size"] = json!(1);
    for field in [
        "block_class",
        "upstream_status",
        "request_id",
        "request_id_truncated",
        "response_time",
        "response_time_truncated",
        "soap_fault",
        "soap_fault_truncated",
        "message",
        "message_truncated",
    ] {
        complete_with_match["line_items"]
            .as_object_mut()
            .unwrap()
            .remove(field);
    }
    complete_with_match["proof_flags"]["line_items_capped_or_truncated"] = json!(false);
    complete_with_match["proof_flags"]["line_items_blocked"] = json!(false);
    complete_with_match = reseal(complete_with_match, ProbeKind::AdUnitDependency, true);

    for full in [
        completed_dependency(false),
        completed_dependency(true),
        complete_with_match.clone(),
    ] {
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_ok()
        );
    }

    let mut complete_wrong_state = completed_dependency(false);
    complete_wrong_state["line_items"]["proof_state"] = json!("sample_only");
    complete_wrong_state["proof_flags"]["line_items_capped_or_truncated"] = json!(true);
    complete_wrong_state["dependency_decision"] = json!("incomplete_no_dependencies_observed");

    let mut sample_wrong_state = completed_dependency(true);
    sample_wrong_state["line_items"]["proof_state"] = json!("complete");
    sample_wrong_state["proof_flags"]["line_items_capped_or_truncated"] = json!(false);
    sample_wrong_state["dependency_decision"] = json!("no_dependencies_observed");

    let mut complete_wrong_decision = completed_dependency(false);
    complete_wrong_decision["line_items"]["decision"] = json!("no_dependencies_in_sample");

    let mut sample_wrong_decision = completed_dependency(true);
    sample_wrong_decision["line_items"]["decision"] = json!("no_dependencies_observed");

    let mut matched_wrong_decision = complete_with_match;
    matched_wrong_decision["line_items"]["decision"] = json!("no_dependencies_observed");

    let mut invalid_page_size = completed_dependency(false);
    invalid_page_size["line_items"]["line_item_page_size"] = json!(0);

    let mut invalid_scan_max = completed_dependency(false);
    invalid_scan_max["line_items"]["max_line_items"] = json!(5_001);

    let mut changed_transport_limit = completed_dependency(false);
    changed_transport_limit["line_items"]["transport_metadata_sample_limit"] = json!(49);

    let mut changed_match_limit = completed_dependency(false);
    changed_match_limit["line_items"]["dependency_match_sample_limit"] = json!(49);

    for full in [
        complete_wrong_state,
        sample_wrong_state,
        complete_wrong_decision,
        sample_wrong_decision,
        matched_wrong_decision,
        invalid_page_size,
        invalid_scan_max,
        changed_transport_limit,
        changed_match_limit,
    ] {
        let full = reseal(full, ProbeKind::AdUnitDependency, true);
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }

    let mut lower_total = completed_dependency(false);
    lower_total["line_items"]["total_result_set_size"] = json!(0);
    lower_total["line_items"]["proof_state"] = json!("sample_only");
    lower_total["line_items"]["decision"] = json!("no_dependencies_in_sample");
    lower_total["proof_flags"]["line_items_capped_or_truncated"] = json!(true);
    lower_total["dependency_decision"] = json!("incomplete_no_dependencies_observed");
    lower_total = reseal(lower_total, ProbeKind::AdUnitDependency, true);
    let compact = compact_success(
        ProbeKind::AdUnitDependency,
        &lower_total,
        &json!({"mutation_performed":false}),
    )
    .expect("lower-than-inspected total remains bounded incomplete evidence");
    assert_eq!(compact["line_items"]["proof_state"], "sample_only");
    assert_eq!(
        compact["proof_flags"]["line_items_capped_or_truncated"],
        true
    );
}

#[test]
fn placement_target_map_must_agree_with_match_count_and_sample() {
    let mut clear_with_hidden_reference = completed_dependency(false);
    clear_with_hidden_reference["placements"]["target_placement_ids_by_ad_unit_id"]["1"] =
        json!(["10"]);

    let mut match_without_reference = completed_dependency(false);
    match_without_reference["placements"]["target_placement_match_count"] = json!(1);
    match_without_reference["placements"]["target_placement_matches_sample"] = json!([{
        "placement_id":"10","status":"ACTIVE","matched_ad_unit_ids":["1"]
    }]);
    match_without_reference["dependency_decision"] = json!("dependencies_found");

    let mut mismatched_sample_reference = match_without_reference.clone();
    mismatched_sample_reference["placements"]["target_placement_ids_by_ad_unit_id"]["1"] =
        json!(["11"]);

    for full in [
        clear_with_hidden_reference,
        match_without_reference,
        mismatched_sample_reference,
    ] {
        let full = reseal(full, ProbeKind::AdUnitDependency, true);
        assert!(
            compact_success(
                ProbeKind::AdUnitDependency,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }
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
    dependency_sample["placements"]["target_placement_matches_sample"][0]["matched_ad_unit_ids"] =
        json!(["999"]);
    dependency_sample = reseal(dependency_sample, ProbeKind::AdUnitDependency, true);
    assert!(
        compact_success(
            ProbeKind::AdUnitDependency,
            &dependency_sample,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );

    let mut dependency_line_item = dependency(true, 9_000);
    dependency_line_item["line_items"]["dependency_matches_sample"][0]["target_matches"][0]["ad_unit_id"] =
        json!("999");
    dependency_line_item = reseal(dependency_line_item, ProbeKind::AdUnitDependency, true);
    assert!(
        compact_success(
            ProbeKind::AdUnitDependency,
            &dependency_line_item,
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

    let mut certainty = exchange(true, 9_000);
    certainty["certainty"]["can_prove_yield_group_targeting"] = json!(false);
    certainty = reseal(certainty, ProbeKind::ExchangeProtection, true);
    assert!(
        compact_success(
            ProbeKind::ExchangeProtection,
            &certainty,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );

    let mut exposed_without_attention =
        exchange_cases::exchange_with_yield_classification("targeted_exposed");
    exposed_without_attention["attention_reasons"] = json!([]);
    exposed_without_attention["overall_decision"] = json!("partial_api_proof");
    exposed_without_attention = reseal(
        exposed_without_attention,
        ProbeKind::ExchangeProtection,
        true,
    );
    assert!(
        compact_success(
            ProbeKind::ExchangeProtection,
            &exposed_without_attention,
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

    let mut full = dependency(true, 9_000);
    full["line_items"]["request_ids"] = Value::Array(
        (0..50)
            .map(|index| json!(format!("request-{index}")))
            .collect(),
    );
    full["line_items"]["request_id_count"] = json!(51);
    full["line_items"]["request_ids_truncated"] = json!(false);
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
        ProbeKind::ExchangeProtection,
        &full,
        &json!({"mutation_performed":false}),
    )
    .unwrap();
    compact["overall_decision"] = json!("attention_required");
    compact = rebind(compact);
    assert_eq!(validated_receipt_binding(&compact), Some(true));
    assert!(validate_projection(ProbeKind::ExchangeProtection, &full, &compact, &receipt).is_err());
    let mut compact = compact_success(
        ProbeKind::ExchangeProtection,
        &full,
        &json!({"mutation_performed":false}),
    )
    .unwrap();
    compact["result_projection"]["omissions"][0]["source_count"] = json!(2);
    compact = rebind(compact);
    assert_eq!(validated_receipt_binding(&compact), Some(true));
    assert!(validate_projection(ProbeKind::ExchangeProtection, &full, &compact, &receipt).is_err());

    let full = dependency(true, 9_000);
    let receipt = verify_receipt(ProbeKind::AdUnitDependency, &full).unwrap();
    let mut compact = compact_success(
        ProbeKind::AdUnitDependency,
        &full,
        &json!({"mutation_performed":false}),
    )
    .unwrap();
    compact["proof_flags"]["line_items_blocked"] = json!(false);
    compact["cleanup_decision"]["safe_to_archive_or_retire"] = json!(true);
    compact = rebind(compact);
    assert_eq!(validated_receipt_binding(&compact), Some(true));
    assert!(validate_projection(ProbeKind::AdUnitDependency, &full, &compact, &receipt).is_err());
}

#[test]
fn oversized_error_is_redacted_utf8_safe_and_byte_exact() {
    let error = AdManagerError::AuthBootstrap(format!(
        "access_token=secret {} {}",
        "€".repeat(3_000),
        "\\\"".repeat(2_000)
    ));
    let result = bounded_probe_error(
        ProbeKind::ExchangeProtection,
        error,
        Instant::now(),
        "probe",
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
    assert_eq!(
        row["source_count"].as_u64().unwrap(),
        row["retained_count"].as_u64().unwrap() + row["omitted_count"].as_u64().unwrap()
    );
    assert_eq!(row["retained_count"], json!(message.len()));
}

#[test]
fn compact_oversize_fails_closed() {
    let mut full = completed_dependency(false);
    full["line_items"]["status_counts"] = Value::Object(
        (0..200)
            .map(|index| (format!("STATUS_{index:03}_{}", "x".repeat(45)), json!(1)))
            .collect(),
    );
    full["line_items"]["inspected_results"] = json!(200);
    full["line_items"]["total_result_set_size"] = json!(200);
    full = reseal(full, ProbeKind::AdUnitDependency, true);
    let envelope = structured(bounded_probe_success(
        ProbeKind::AdUnitDependency,
        full,
        json!({"mutation_performed":false}),
        Instant::now(),
        "probe",
    ));
    assert_eq!(envelope["error"]["code"], "result_contract_error");
}
