use super::*;

fn producer_unsupported_surfaces() -> Value {
    json!([
        {
            "surface": "protections",
            "proof_state": "not_proven",
            "api_exposure": "not_seen_in_rest_discovery",
            "note": "GAM protection objects are not implemented as a current MCP read surface.",
        },
        {
            "surface": "inventory_rules",
            "proof_state": "not_proven",
            "api_exposure": "not_seen_in_rest_discovery",
            "note": "GAM inventory-rule objects are not implemented as a current MCP read surface.",
        },
        {
            "surface": "unified_pricing_rules",
            "proof_state": "not_proven",
            "api_exposure": "not_seen_in_rest_discovery",
            "note": "GAM unified pricing rules are not implemented as a current MCP read surface.",
        },
    ])
}

fn producer_exchange(generated: bool, raw: usize) -> Value {
    let mut full = super::exchange(generated, raw);
    for row in full["ad_units"].as_array_mut().unwrap() {
        let id = row["ad_unit_id"].as_str().unwrap().to_string();
        let row = row.as_object_mut().unwrap();
        row.insert("ad_unit_code".into(), json!(format!("unit-{id}")));
        row.insert("ancestor_ad_unit_ids".into(), json!([]));
        row.insert("ancestor_identity_complete".into(), json!(true));
        row.insert("display_name".into(), json!(format!("Inventory unit {id}")));
        row.insert("status".into(), json!("ACTIVE"));
        row.insert("ad_unit_sizes".into(), json!([]));
    }
    full["unsupported_or_unintegrated_surfaces"] = producer_unsupported_surfaces();
    reseal(full, ProbeKind::ExchangeProtection, generated)
}

fn expected_exchange_projection_body() -> Value {
    json!({
        "network_code": "1234567",
        "overall_decision": "partial_api_proof",
        "ad_units_summary": {
            "source_count": 1,
            "identity": {
                "canonical_ad_unit_ids": ["1"],
                "canonical_ad_unit_id_count": 1,
                "missing_ad_unit_id_count": 0,
                "duplicate_ad_unit_id_count": 0,
            },
            "proof_state_counts": {"resolved_exact": 1},
            "decision_counts": {"clear_on_exposed_flags": 1},
            "proof_complete_counts": {"false": 0, "true": 1, "unknown": 0},
            "applied_adsense_enabled_counts": {"false": 1, "true": 0, "unknown": 0},
            "effective_adsense_enabled_counts": {"false": 1, "true": 0, "unknown": 0},
            "explicitly_targeted_counts": {"false": 0, "true": 1, "unknown": 0},
        },
        "private_auctions": {
            "collection": "private_auctions",
            "proof_state": "complete_empty",
            "row_count_in_page": 0,
            "page_size": 100,
            "next_page_token_present": false,
            "capped_or_possibly_more": false,
            "sample_count": 0,
        },
        "private_auction_deals": {
            "collection": "private_auction_deals",
            "proof_state": "complete_empty",
            "row_count_in_page": 0,
            "page_size": 100,
            "next_page_token_present": false,
            "capped_or_possibly_more": false,
            "sample_count": 0,
        },
        "yield_groups": {
            "surface": "yield_groups",
            "decision": "no_target_matches",
            "proof_state": "complete",
            "total_result_set_size": 0,
            "inspected_results": 0,
            "response_truncated": false,
            "mutation_performed": false,
            "request_id_truncated": false,
            "response_time_truncated": false,
            "target_ad_unit_count": 1,
            "target_ad_unit_match_count": 0,
            "targeting_class_counts": {
                "targeted_exposed": 0,
                "targeted_and_excluded": 0,
                "targeted_inactive": 0,
                "targeted_activity_unknown": 0,
            },
        },
        "rest_discovery": {
            "proof_state": "metadata_read",
            "resource_count": 1,
            "interesting_resource_count": 1,
        },
        "unsupported_or_unintegrated_surfaces": producer_unsupported_surfaces(),
        "attention_reason_count": 0,
        "partial_reason_count": 1,
        "certainty": {
            "can_prove_requested_ad_unit_flags": true,
            "can_prove_private_auction_absence_or_presence": true,
            "can_prove_private_deal_absence_or_presence": true,
            "can_prove_yield_group_targeting": true,
            "cannot_prove_via_current_api": [
                "protections",
                "inventory_rules",
                "unified_pricing_rules"
            ],
        },
    })
}

pub(super) fn exchange_with_yield_classification(classification: &str) -> Value {
    let mut full = producer_exchange(true, 9_000);
    let (summary_field, match_field, status, activity_state, excluded) = match classification {
        "targeted_exposed" => (
            "targeted_exposed",
            "targeted_exposed_ad_unit_ids",
            json!("ACTIVE"),
            "active",
            false,
        ),
        "targeted_and_excluded" => (
            "targeted_and_excluded",
            "targeted_and_excluded_ad_unit_ids",
            json!("ACTIVE"),
            "active",
            true,
        ),
        "targeted_inactive" => (
            "targeted_inactive",
            "targeted_inactive_ad_unit_ids",
            json!("INACTIVE"),
            "inactive",
            false,
        ),
        "targeted_activity_unknown" => (
            "targeted_activity_unknown",
            "targeted_activity_unknown_ad_unit_ids",
            Value::Null,
            "unknown",
            false,
        ),
        _ => panic!("unknown fixture classification"),
    };
    let coverage = json!({
        "ad_unit_id": "1",
        "include_descendants": false,
        "match_type": "exact",
    });
    let mut target_match = json!({
        "yield_group_id": "10",
        "yield_group_name": "Fixture yield group",
        "status": status.clone(),
        "activity_state": activity_state,
        "format": "NATIVE",
        "environment_type": "WEB",
        "matched_ad_unit_ids": ["1"],
        "targeted_exposed_ad_unit_ids": [],
        "targeted_and_excluded_ad_unit_ids": [],
        "targeted_inactive_ad_unit_ids": [],
        "targeted_activity_unknown_ad_unit_ids": [],
        "targeted_ad_units": [{"ad_unit_id": "1", "include_descendants": false}],
        "excluded_ad_units": (if excluded {
            json!([{"ad_unit_id": "1", "include_descendants": false}])
        } else {
            json!([])
        }),
    });
    target_match[match_field] = json!(["1"]);
    full["yield_groups"]["target_ad_unit_matches"] = json!([target_match]);
    full["yield_groups"][summary_field] = json!([{
        "yield_group_id": "10",
        "yield_group_name": "Fixture yield group",
        "status": status,
        "activity_state": activity_state,
        "format": "NATIVE",
        "environment_type": "WEB",
        "requested_ad_unit_id": "1",
        "classification": classification,
        "targeting_match": coverage.clone(),
        "exclusion_match": (if excluded { coverage } else { Value::Null }),
    }]);
    full["yield_groups"]["total_result_set_size"] = json!(1);
    full["yield_groups"]["inspected_results"] = json!(1);
    full["yield_groups"]["decision"] = json!(classification);
    if classification == "targeted_exposed" {
        full["attention_reasons"] = json!(["yield group targets requested inventory"]);
        full["overall_decision"] = json!("attention_required");
    }
    reseal(full, ProbeKind::ExchangeProtection, true)
}

fn early_exchange_variant(variant: &str) -> Value {
    let mut full = producer_exchange(false, 0);
    full["certainty"]["can_prove_yield_group_targeting"] = json!(false);
    match variant {
        "skipped" => {
            full["ad_units"] = Value::Array(
                (0..50)
                    .map(|index| {
                        json!({
                            "ad_unit_code": format!("missing-{index}"),
                            "decision": "attention_required",
                            "proof_state": "missing",
                            "proof_complete": false,
                            "reason": "exact ad-unit code was not returned",
                            "matches": 0,
                        })
                    })
                    .collect(),
            );
            full["attention_reasons"] = Value::Array(
                (0..50)
                    .map(|index| json!(format!("missing target {index} requires review")))
                    .collect(),
            );
            full["overall_decision"] = json!("attention_required");
            full["certainty"]["can_prove_requested_ad_unit_flags"] = json!(false);
            full["yield_groups"] = json!({
                "surface": "yield_groups",
                "decision": "skipped",
                "proof_state": "skipped",
                "reason": "no target ad-unit ids were available",
                "mutation_performed": false,
            });
        }
        "permission" => {
            full["yield_groups"] = json!({
                "surface": "yield_groups",
                "decision": "blocked",
                "proof_state": "blocked",
                "block_class": "permission",
                "reason": "manage scope required",
                "required_scope": "manage-scope",
                "current_scope": "readonly-scope",
                "current_scope_truncated": false,
                "mutation_performed": false,
            });
        }
        "upstream" => {
            full["yield_groups"] = json!({
                "surface": "yield_groups",
                "decision": "blocked",
                "proof_state": "blocked",
                "block_class": "upstream",
                "upstream_status": 500,
                "request_id": "request-1",
                "request_id_truncated": false,
                "response_time": "1",
                "response_time_truncated": false,
                "soap_fault": "upstream fault",
                "soap_fault_truncated": false,
                "message": "upstream read blocked",
                "message_truncated": false,
                "mutation_performed": false,
            });
        }
        "preflight" => {
            full["yield_groups"] = json!({
                "surface": "yield_groups",
                "proof_state": "blocked",
                "block_class": "upstream",
                "error": "preflight read blocked",
                "error_truncated": false,
                "hint": "retry after checking credentials",
                "hint_truncated": false,
            });
        }
        _ => panic!("unknown early exchange fixture"),
    }
    reseal(full, ProbeKind::ExchangeProtection, false)
}

fn blocked_soap_exchange(upstream_status: usize, block_class: &str, soap_fault: Value) -> Value {
    let mut full = early_exchange_variant("upstream");
    let message = soap_fault
        .as_str()
        .unwrap_or("upstream SOAP response failed")
        .to_string();
    full["yield_groups"]["upstream_status"] = json!(upstream_status);
    full["yield_groups"]["block_class"] = json!(block_class);
    full["yield_groups"]["soap_fault"] = soap_fault;
    full["yield_groups"]["soap_fault_truncated"] = json!(false);
    full["yield_groups"]["message"] = json!(message);
    reseal(full, ProbeKind::ExchangeProtection, false)
}

#[test]
fn exchange_projection_matches_whole_authoritative_body() {
    let envelope = structured(bounded_probe_success(
        ProbeKind::ExchangeProtection,
        producer_exchange(true, 9_000),
        json!({"mutation_performed":false}),
        Instant::now(),
        "probe",
    ));
    let mut body = envelope["data"].clone();
    let body = body.as_object_mut().unwrap();
    for field in [
        "source_result_fingerprint",
        "result_projection",
        "result_fingerprint",
        "evidence_receipt_template",
    ] {
        body.remove(field);
    }
    assert_eq!(
        Value::Object(body.clone()),
        expected_exchange_projection_body()
    );
}

#[test]
fn exchange_ad_unit_projection_derives_decisions_from_identity_and_exposure() {
    let mut exposed = producer_exchange(true, 9_000);
    exposed["ad_units"][0]["applied_adsense_enabled"] = json!(true);
    exposed["ad_units"][0]["decision"] = json!("attention_required");
    exposed["attention_reasons"] = json!(["ad unit unit-1 needs review"]);
    exposed["overall_decision"] = json!("attention_required");
    exposed = reseal(exposed, ProbeKind::ExchangeProtection, true);
    let compact = compact_success(
        ProbeKind::ExchangeProtection,
        &exposed,
        &json!({"mutation_performed":false}),
    )
    .expect("producer-derived exposure decision is accepted");
    assert_eq!(
        compact["ad_units_summary"]["decision_counts"]["attention_required"],
        1
    );

    let mut partial = producer_exchange(true, 9_000);
    partial["ad_units"][0]["effective_adsense_enabled"] = Value::Null;
    partial["ad_units"][0]["proof_complete"] = json!(false);
    partial["ad_units"][0]["decision"] = json!("partial_api_proof");
    partial["certainty"]["can_prove_requested_ad_unit_flags"] = json!(false);
    partial = reseal(partial, ProbeKind::ExchangeProtection, true);
    let compact = compact_success(
        ProbeKind::ExchangeProtection,
        &partial,
        &json!({"mutation_performed":false}),
    )
    .expect("producer-derived incomplete exposure proof is accepted");
    assert_eq!(
        compact["ad_units_summary"]["proof_complete_counts"]["false"],
        1
    );

    let mut decision_drift = producer_exchange(true, 9_000);
    decision_drift["ad_units"][0]["decision"] = json!("attention_required");
    decision_drift["attention_reasons"] = json!(["coordinated decision drift"]);
    decision_drift["overall_decision"] = json!("attention_required");
    decision_drift = reseal(decision_drift, ProbeKind::ExchangeProtection, true);

    let mut proof_drift = producer_exchange(true, 9_000);
    proof_drift["ad_units"][0]["proof_complete"] = json!(false);
    proof_drift["certainty"]["can_prove_requested_ad_unit_flags"] = json!(false);
    proof_drift = reseal(proof_drift, ProbeKind::ExchangeProtection, true);

    let mut identity_drift = producer_exchange(true, 9_000);
    identity_drift["ad_units"][0]["resource_name"] = json!("networks/7654321/adUnits/1");
    identity_drift["ad_units"][0]["ad_unit_id"] = Value::Null;
    identity_drift["ad_units"][0]["proof_state"] = json!("invalid_resource_name");
    identity_drift["ad_units"][0]["proof_complete"] = json!(false);
    identity_drift["ad_units"][0]["decision"] = json!("partial_api_proof");
    identity_drift["certainty"]["can_prove_requested_ad_unit_flags"] = json!(false);
    identity_drift = reseal(identity_drift, ProbeKind::ExchangeProtection, false);

    for full in [decision_drift, proof_drift, identity_drift] {
        assert!(
            compact_success(
                ProbeKind::ExchangeProtection,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }
}

#[test]
fn exchange_ad_unit_resolution_variants_enforce_producer_cardinality() {
    let mut valid = early_exchange_variant("skipped");
    valid["ad_units"][0]["proof_state"] = json!("ambiguous");
    valid["ad_units"][0]["matches"] = json!(2);
    valid = reseal(valid, ProbeKind::ExchangeProtection, false);
    compact_success(
        ProbeKind::ExchangeProtection,
        &valid,
        &json!({"mutation_performed":false}),
    )
    .expect("producer-shaped ambiguous target is accepted");

    valid["ad_units"][0]["matches"] = json!(1);
    let invalid = reseal(valid, ProbeKind::ExchangeProtection, false);
    assert!(
        compact_success(
            ProbeKind::ExchangeProtection,
            &invalid,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );

    let mut too_many = early_exchange_variant("skipped");
    too_many["ad_units"].as_array_mut().unwrap().push(json!({
        "ad_unit_code":"missing-50",
        "decision":"attention_required",
        "proof_state":"missing",
        "proof_complete":false,
        "reason":"exact ad-unit code was not returned",
        "matches":0
    }));
    too_many = reseal(too_many, ProbeKind::ExchangeProtection, false);
    assert!(
        compact_success(
            ProbeKind::ExchangeProtection,
            &too_many,
            &json!({"mutation_performed":false})
        )
        .is_err()
    );
}

#[test]
fn exchange_yield_classifications_preserve_counts_and_fail_closed_on_drift() {
    for classification in [
        "targeted_exposed",
        "targeted_and_excluded",
        "targeted_inactive",
        "targeted_activity_unknown",
    ] {
        let full = exchange_with_yield_classification(classification);
        let compact = compact_success(
            ProbeKind::ExchangeProtection,
            &full,
            &json!({"mutation_performed":false}),
        )
        .unwrap();
        assert_eq!(
            compact["yield_groups"]["targeting_class_counts"][classification],
            1
        );
        assert_eq!(compact["yield_groups"]["decision"], classification);
    }

    let mut invalid_class = exchange_with_yield_classification("targeted_exposed");
    invalid_class["yield_groups"]["targeted_exposed"][0]["classification"] =
        json!("targeted_inactive");
    invalid_class = reseal(invalid_class, ProbeKind::ExchangeProtection, true);

    let mut invalid_scope = exchange_with_yield_classification("targeted_exposed");
    invalid_scope["yield_groups"]["target_ad_unit_matches"][0]["matched_ad_unit_ids"] =
        json!(["999"]);
    invalid_scope = reseal(invalid_scope, ProbeKind::ExchangeProtection, true);

    let mut invalid_counts = exchange_with_yield_classification("targeted_exposed");
    invalid_counts["yield_groups"]["target_ad_unit_matches"][0]["targeted_exposed_ad_unit_ids"] =
        json!([]);
    invalid_counts = reseal(invalid_counts, ProbeKind::ExchangeProtection, true);

    let mut invalid_decision = exchange_with_yield_classification("targeted_exposed");
    invalid_decision["yield_groups"]["decision"] = json!("no_target_matches");
    invalid_decision = reseal(invalid_decision, ProbeKind::ExchangeProtection, true);

    let mut incomplete_normal = exchange_with_yield_classification("targeted_exposed");
    incomplete_normal["yield_groups"]
        .as_object_mut()
        .unwrap()
        .remove("target_ad_unit_matches");
    incomplete_normal = reseal(incomplete_normal, ProbeKind::ExchangeProtection, true);

    let mut contradictory_skipped = early_exchange_variant("skipped");
    contradictory_skipped["yield_groups"]["decision"] = json!("targeted_exposed");
    contradictory_skipped = reseal(contradictory_skipped, ProbeKind::ExchangeProtection, false);

    let mut activity_drift = exchange_with_yield_classification("targeted_inactive");
    activity_drift["yield_groups"]["target_ad_unit_matches"][0]["status"] = json!("ACTIVE");
    activity_drift["yield_groups"]["target_ad_unit_matches"][0]["activity_state"] = json!("active");
    activity_drift["yield_groups"]["targeted_inactive"][0]["status"] = json!("ACTIVE");
    activity_drift["yield_groups"]["targeted_inactive"][0]["activity_state"] = json!("active");
    activity_drift = reseal(activity_drift, ProbeKind::ExchangeProtection, true);

    let mut exclusion_drift = exchange_with_yield_classification("targeted_and_excluded");
    exclusion_drift["yield_groups"]["target_ad_unit_matches"][0]["excluded_ad_units"] = json!([]);
    exclusion_drift["yield_groups"]["targeted_and_excluded"][0]["exclusion_match"] = Value::Null;
    exclusion_drift = reseal(exclusion_drift, ProbeKind::ExchangeProtection, true);

    let mut impossible_match_cardinality = exchange_with_yield_classification("targeted_exposed");
    impossible_match_cardinality["yield_groups"]["inspected_results"] = json!(0);
    impossible_match_cardinality["yield_groups"]["total_result_set_size"] = json!(0);
    impossible_match_cardinality = reseal(
        impossible_match_cardinality,
        ProbeKind::ExchangeProtection,
        true,
    );

    for full in [
        invalid_class,
        invalid_scope,
        invalid_counts,
        invalid_decision,
        incomplete_normal,
        contradictory_skipped,
        activity_drift,
        exclusion_drift,
        impossible_match_cardinality,
    ] {
        assert!(
            compact_success(
                ProbeKind::ExchangeProtection,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }
}

#[test]
fn exchange_projection_rejects_ambiguous_or_impossible_yield_progress() {
    let mut missing_total = producer_exchange(true, 9_000);
    missing_total["yield_groups"]["total_result_set_size"] = Value::Null;
    missing_total = reseal(missing_total, ProbeKind::ExchangeProtection, true);

    let mut invalid_total = producer_exchange(true, 9_000);
    invalid_total["yield_groups"]["total_result_set_size"] = json!(-1);
    invalid_total = reseal(invalid_total, ProbeKind::ExchangeProtection, true);

    let mut impossible_progress = producer_exchange(true, 9_000);
    impossible_progress["yield_groups"]["inspected_results"] = json!(1);
    impossible_progress = reseal(impossible_progress, ProbeKind::ExchangeProtection, true);

    let mut impossible_request_id = producer_exchange(true, 9_000);
    impossible_request_id["yield_groups"]["request_id"] = Value::Null;
    impossible_request_id["yield_groups"]["request_id_truncated"] = json!(true);
    impossible_request_id = reseal(impossible_request_id, ProbeKind::ExchangeProtection, true);

    let mut empty_targets = producer_exchange(true, 9_000);
    empty_targets["ad_units"] = json!([]);
    empty_targets = reseal(empty_targets, ProbeKind::ExchangeProtection, false);

    for full in [
        missing_total,
        invalid_total,
        impossible_progress,
        impossible_request_id,
        empty_targets,
    ] {
        assert!(
            compact_success(
                ProbeKind::ExchangeProtection,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }
}

#[test]
fn exchange_projection_accepts_explicitly_truncated_unknown_yield_total() {
    let mut full = producer_exchange(true, 9_000);
    full["yield_groups"]["total_result_set_size"] = Value::Null;
    full["yield_groups"]["response_truncated"] = json!(true);
    full["yield_groups"]["proof_state"] = json!("sample_only");
    full["yield_groups"]["decision"] = json!("sample_only");
    full["certainty"]["can_prove_yield_group_targeting"] = json!(false);
    full = reseal(full, ProbeKind::ExchangeProtection, true);

    let compact = compact_success(
        ProbeKind::ExchangeProtection,
        &full,
        &json!({"mutation_performed":false}),
    )
    .expect("explicit truncation preserves bounded partial proof");
    assert_eq!(compact["yield_groups"]["proof_state"], "sample_only");
    assert_eq!(compact["yield_groups"]["decision"], "sample_only");
    assert_eq!(
        compact["certainty"]["can_prove_yield_group_targeting"],
        false
    );
}

#[test]
fn exchange_private_market_proof_state_is_derived_from_page_evidence() {
    for field in ["private_auctions", "private_auction_deals"] {
        let sample = json!({
            "resource_name": format!("networks/1234567/{field}/1"),
            "resource_id": "1",
            "display_name": "Fixture private market row",
            "status": "ACTIVE",
        });
        let mut present = producer_exchange(true, 9_000);
        present[field] = json!({
            "collection": field,
            "proof_state": "complete_present",
            "row_count_in_page": 1,
            "page_size": 100,
            "next_page_token_present": false,
            "capped_or_possibly_more": false,
            "sample": [sample],
        });
        present["attention_reasons"] = json!(["private market inventory is present"]);
        present["overall_decision"] = json!("attention_required");
        present = reseal(present, ProbeKind::ExchangeProtection, true);
        let compact = compact_success(
            ProbeKind::ExchangeProtection,
            &present,
            &json!({"mutation_performed":false}),
        )
        .expect("uncapped positive collection is complete-present evidence");
        assert_eq!(compact[field]["proof_state"], "complete_present");

        let mut empty_page_with_next = producer_exchange(true, 9_000);
        empty_page_with_next[field] = json!({
            "collection": field,
            "proof_state": "sample_only",
            "row_count_in_page": 0,
            "page_size": 100,
            "next_page_token_present": true,
            "capped_or_possibly_more": true,
            "sample": [],
        });
        let certainty_field = if field == "private_auctions" {
            "can_prove_private_auction_absence_or_presence"
        } else {
            "can_prove_private_deal_absence_or_presence"
        };
        empty_page_with_next["certainty"][certainty_field] = json!(false);
        empty_page_with_next = reseal(empty_page_with_next, ProbeKind::ExchangeProtection, true);
        compact_success(
            ProbeKind::ExchangeProtection,
            &empty_page_with_next,
            &json!({"mutation_performed":false}),
        )
        .expect("empty paginated page remains bounded sample-only evidence");

        let mut proof_state_drift = present.clone();
        proof_state_drift[field]["proof_state"] = json!("complete_empty");
        proof_state_drift = reseal(proof_state_drift, ProbeKind::ExchangeProtection, true);

        let mut cap_drift = empty_page_with_next;
        cap_drift[field]["capped_or_possibly_more"] = json!(false);
        cap_drift = reseal(cap_drift, ProbeKind::ExchangeProtection, true);

        let mut sample_drift = present.clone();
        sample_drift[field]["sample"] = json!([]);
        sample_drift = reseal(sample_drift, ProbeKind::ExchangeProtection, true);

        for full in [proof_state_drift, cap_drift, sample_drift] {
            assert!(
                compact_success(
                    ProbeKind::ExchangeProtection,
                    &full,
                    &json!({"mutation_performed":false})
                )
                .is_err()
            );
        }
    }
}

#[test]
fn exchange_projection_rejects_capped_private_market_rows_without_attention_semantics() {
    for field in ["private_auctions", "private_auction_deals"] {
        let mut full = producer_exchange(true, 9_000);
        full[field] = json!({
            "collection": field,
            "proof_state": "sample_only",
            "row_count_in_page": 1,
            "page_size": 1,
            "next_page_token_present": false,
            "capped_or_possibly_more": true,
            "sample": [{
                "resource_name": format!("networks/1234567/{field}/1"),
                "resource_id": "1",
                "display_name": "Fixture private market row",
                "status": "ACTIVE",
            }],
        });
        if field == "private_auctions" {
            full["certainty"]["can_prove_private_auction_absence_or_presence"] = json!(false);
        } else {
            full["certainty"]["can_prove_private_deal_absence_or_presence"] = json!(false);
        }
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
}

#[test]
fn unsupported_exchange_surfaces_follow_fixed_rest_discovery_derivation() {
    let mut seen = producer_exchange(true, 9_000);
    seen["rest_discovery"]["interesting_resources"] = json!(["networks/protections"]);
    seen["unsupported_or_unintegrated_surfaces"][0]["api_exposure"] =
        json!("resource_seen_but_not_integrated");
    seen = reseal(seen, ProbeKind::ExchangeProtection, true);
    let compact = compact_success(
        ProbeKind::ExchangeProtection,
        &seen,
        &json!({"mutation_performed":false}),
    )
    .expect("fixed unsupported surface reflects discovered protection resource");
    assert_eq!(
        compact["unsupported_or_unintegrated_surfaces"][0]["api_exposure"],
        "resource_seen_but_not_integrated"
    );

    let mut discovery_drift = producer_exchange(true, 9_000);
    discovery_drift["rest_discovery"]["interesting_resources"] = json!(["networks/protections"]);
    discovery_drift = reseal(discovery_drift, ProbeKind::ExchangeProtection, true);

    let mut schema_drift = producer_exchange(true, 9_000);
    schema_drift["unsupported_or_unintegrated_surfaces"][1]["note"] =
        json!("fixture-only unsupported note");
    schema_drift = reseal(schema_drift, ProbeKind::ExchangeProtection, true);

    let mut missing_surface = producer_exchange(true, 9_000);
    missing_surface["unsupported_or_unintegrated_surfaces"]
        .as_array_mut()
        .unwrap()
        .pop();
    missing_surface = reseal(missing_surface, ProbeKind::ExchangeProtection, true);

    for full in [discovery_drift, schema_drift, missing_surface] {
        assert!(
            compact_success(
                ProbeKind::ExchangeProtection,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }

    let mut blocked = producer_exchange(true, 9_000);
    blocked["rest_discovery"] = json!({
        "surface": "rest_discovery",
        "proof_state": "blocked",
        "block_class": "upstream",
        "error": "discovery unavailable",
        "error_truncated": false,
        "hint": "retry later",
        "hint_truncated": false,
    });
    blocked = reseal(blocked, ProbeKind::ExchangeProtection, true);
    compact_success(
        ProbeKind::ExchangeProtection,
        &blocked,
        &json!({"mutation_performed":false}),
    )
    .expect("blocked discovery derives all fixed surfaces as unseen");
}

#[test]
fn blocked_soap_yield_matches_status_fault_and_permission_derivation() {
    let mut nullable_transport = blocked_soap_exchange(500, "upstream", Value::Null);
    nullable_transport["yield_groups"]["request_id"] = Value::Null;
    nullable_transport["yield_groups"]["request_id_truncated"] = json!(false);
    nullable_transport["yield_groups"]["response_time"] = Value::Null;
    nullable_transport["yield_groups"]["response_time_truncated"] = json!(false);
    nullable_transport = reseal(nullable_transport, ProbeKind::ExchangeProtection, false);

    for full in [
        nullable_transport,
        blocked_soap_exchange(200, "upstream", json!("ServerError.INTERNAL_ERROR")),
        blocked_soap_exchange(
            200,
            "permission",
            json!("PermissionError.PERMISSION_DENIED"),
        ),
        blocked_soap_exchange(403, "permission", Value::Null),
        blocked_soap_exchange(500, "permission", json!("opaque SOAP fault")),
    ] {
        compact_success(
            ProbeKind::ExchangeProtection,
            &full,
            &json!({"mutation_performed":false}),
        )
        .expect("producer-valid blocked SOAP result is accepted");
    }

    let no_error_or_fault = blocked_soap_exchange(200, "upstream", Value::Null);
    let wrong_http_permission_class = blocked_soap_exchange(401, "upstream", Value::Null);
    let wrong_fault_permission_class =
        blocked_soap_exchange(500, "upstream", json!("PermissionError.PERMISSION_DENIED"));
    let mut wrong_message_permission_class = blocked_soap_exchange(500, "upstream", Value::Null);
    wrong_message_permission_class["yield_groups"]["message"] =
        json!("AuthenticationError.NETWORK_API_ACCESS_DISABLED");
    wrong_message_permission_class = reseal(
        wrong_message_permission_class,
        ProbeKind::ExchangeProtection,
        false,
    );

    let mut invalid = vec![
        no_error_or_fault,
        wrong_http_permission_class,
        wrong_fault_permission_class,
        wrong_message_permission_class,
    ];
    for (field, truncated_field) in [
        ("request_id", "request_id_truncated"),
        ("response_time", "response_time_truncated"),
        ("soap_fault", "soap_fault_truncated"),
    ] {
        let mut null_marked_truncated = blocked_soap_exchange(500, "upstream", Value::Null);
        null_marked_truncated["yield_groups"][field] = Value::Null;
        null_marked_truncated["yield_groups"][truncated_field] = json!(true);
        invalid.push(reseal(
            null_marked_truncated,
            ProbeKind::ExchangeProtection,
            false,
        ));
    }
    for full in invalid {
        assert!(
            compact_success(
                ProbeKind::ExchangeProtection,
                &full,
                &json!({"mutation_performed":false})
            )
            .is_err()
        );
    }
}

#[test]
fn oversized_early_exchange_variants_keep_their_block_state() {
    for (variant, expected_state) in [
        ("skipped", "skipped"),
        ("permission", "blocked"),
        ("upstream", "blocked"),
        ("preflight", "blocked"),
    ] {
        let full = early_exchange_variant(variant);
        assert!(serde_json::to_vec(&full).unwrap().len() > MAX_CONTRACT_ENVELOPE_BYTES);
        let envelope = structured(bounded_probe_success(
            ProbeKind::ExchangeProtection,
            full,
            json!({"mutation_performed":false}),
            Instant::now(),
            "probe",
        ));
        assert_eq!(envelope["ok"], true);
        assert_eq!(
            envelope["data"]["yield_groups"]["proof_state"],
            expected_state
        );
        assert_eq!(
            envelope["data"]["evidence_receipt_template"]["state"],
            "not_generated"
        );
        assert!(envelope["data"].get("ad_units").is_none());
    }
}
