use super::*;

pub(super) fn project_exchange(full: &Value) -> Result<(Value, Vec<Value>), String> {
    let root = object(full, "exchange probe")?;
    exact_keys(
        root,
        &[
            "network_code", "overall_decision", "ad_units", "private_auctions",
            "private_auction_deals", "yield_groups", "rest_discovery",
            "unsupported_or_unintegrated_surfaces", "attention_reasons", "partial_reasons",
            "certainty", "result_fingerprint", "evidence_receipt_template",
        ],
        "exchange probe",
    )?;
    text(root, "network_code", "exchange probe")?;
    text(root, "overall_decision", "exchange probe")?;
    object(get(root, "certainty", "exchange probe")?, "certainty")?;
    let ad_units = array(root, "ad_units", "exchange probe")?;
    let unsupported = array(root, "unsupported_or_unintegrated_surfaces", "exchange probe")?;
    let attention = array(root, "attention_reasons", "exchange probe")?;
    let partial = array(root, "partial_reasons", "exchange probe")?;
    let expected_decision = if !attention.is_empty() {
        "attention_required"
    } else if !partial.is_empty() {
        "partial_api_proof"
    } else {
        "api_exposed_surfaces_clear"
    };
    if text(root, "overall_decision", "exchange probe")? != expected_decision {
        return Err("exchange decision contradicted the retained reason surfaces".into());
    }
    let target_identity = target_identity_summary(ad_units)?;
    let target_ids = canonical_target_ids(ad_units)?;
    let mut ledger = Ledger::default();
    ledger.omit("/ad_units", get(root, "ad_units", "exchange probe")?, Class::Array)?;
    ledger.omit(
        "/attention_reasons",
        get(root, "attention_reasons", "exchange probe")?,
        Class::Reason,
    )?;
    ledger.omit(
        "/partial_reasons",
        get(root, "partial_reasons", "exchange probe")?,
        Class::Reason,
    )?;
    let auctions = exchange_collection(
        get(root, "private_auctions", "exchange probe")?,
        "/private_auctions",
        &mut ledger,
    )?;
    let deals = exchange_collection(
        get(root, "private_auction_deals", "exchange probe")?,
        "/private_auction_deals",
        &mut ledger,
    )?;
    let yield_groups = exchange_yield(
        get(root, "yield_groups", "exchange probe")?,
        "/yield_groups",
        &target_ids,
        &mut ledger,
    )?;
    let discovery = exchange_discovery(
        get(root, "rest_discovery", "exchange probe")?,
        "/rest_discovery",
        &mut ledger,
    )?;
    Ok((
        json!({
            "network_code": root["network_code"],
            "overall_decision": root["overall_decision"],
            "ad_units_summary": {
                "source_count": ad_units.len(),
                "identity": target_identity,
                "proof_state_counts": string_counts(ad_units, "proof_state")?,
                "decision_counts": string_counts(ad_units, "decision")?,
                "proof_complete_counts": bool_counts(ad_units, "proof_complete")?,
                "applied_adsense_enabled_counts": bool_counts(ad_units, "applied_adsense_enabled")?,
                "effective_adsense_enabled_counts": bool_counts(ad_units, "effective_adsense_enabled")?,
                "explicitly_targeted_counts": bool_counts(ad_units, "explicitly_targeted")?,
            },
            "private_auctions": auctions,
            "private_auction_deals": deals,
            "yield_groups": yield_groups,
            "rest_discovery": discovery,
            "unsupported_or_unintegrated_surfaces": unsupported,
            "attention_reason_count": attention.len(),
            "partial_reason_count": partial.len(),
            "certainty": root["certainty"],
        }),
        ledger.0,
    ))
}

fn exchange_collection(full: &Value, path: &str, ledger: &mut Ledger) -> Result<Value, String> {
    let source = object(full, "exchange collection")?;
    let mut result = select(
        source,
        path,
        &[
            "collection", "surface", "proof_state", "row_count_in_page", "page_size",
            "next_page_token_present", "capped_or_possibly_more", "block_class",
            "error_truncated", "hint_truncated",
        ],
        &[
            ("sample", Class::Array),
            ("error", Class::Reason),
            ("hint", Class::Reason),
        ],
        ledger,
    )?;
    text(source, "proof_state", "exchange collection")?;
    validate_truncation_pairs(
        source,
        &[("error", "error_truncated"), ("hint", "hint_truncated")],
        "exchange collection",
    )?;
    if let Some(sample) = source.get("sample") {
        let sample = sample.as_array().ok_or("collection sample was not an array")?;
        if sample.len() != count(source, "row_count_in_page", "exchange collection")?.min(5) {
            return Err("collection row count and omitted sample were inconsistent".into());
        }
        result.insert(
            "sample_count".into(),
            json!(sample.len()),
        );
    }
    Ok(Value::Object(result))
}

fn exchange_yield(
    full: &Value,
    path: &str,
    target_ids: &BTreeSet<String>,
    ledger: &mut Ledger,
) -> Result<Value, String> {
    let source = object(full, "yield groups")?;
    let mut result = select(
        source,
        path,
        &[
            "surface", "decision", "proof_state", "total_result_set_size",
            "inspected_results", "response_truncated", "mutation_performed", "block_class",
            "upstream_status", "request_id_truncated", "response_time_truncated",
            "soap_fault_truncated", "message_truncated", "current_scope_truncated",
            "error_truncated", "hint_truncated",
        ],
        &[
            ("target_ad_unit_ids", Class::Array),
            ("target_ad_unit_matches", Class::Array),
            ("targeted_exposed", Class::Array),
            ("targeted_and_excluded", Class::Array),
            ("targeted_inactive", Class::Array),
            ("targeted_activity_unknown", Class::Array),
            ("upstream_response_xml", Class::RawSoap),
            ("request_id", Class::Transport),
            ("response_time", Class::Transport),
            ("soap_fault", Class::Reason),
            ("message", Class::Reason),
            ("reason", Class::Reason),
            ("required_scope", Class::Reason),
            ("current_scope", Class::Reason),
            ("error", Class::Reason),
            ("hint", Class::Reason),
        ],
        ledger,
    )?;
    text(source, "proof_state", "yield groups")?;
    false_if_present(source, "mutation_performed", "yield groups")?;
    validate_truncation_pairs(
        source,
        &[
            ("request_id", "request_id_truncated"),
            ("response_time", "response_time_truncated"),
            ("soap_fault", "soap_fault_truncated"),
            ("message", "message_truncated"),
            ("current_scope", "current_scope_truncated"),
            ("error", "error_truncated"),
            ("hint", "hint_truncated"),
        ],
        "yield groups",
    )?;
    if source.get("target_ad_unit_matches").is_some() {
        let declared_targets = canonical_id_set(
            array(source, "target_ad_unit_ids", "yield groups")?,
            "yield target ids",
        )?;
        if declared_targets != *target_ids {
            return Err("yield target ids disagreed with the probe target scope".into());
        }
        let fields = [
            ("targeted_exposed", "targeted_exposed"),
            ("targeted_and_excluded", "targeted_and_excluded"),
            ("targeted_inactive", "targeted_inactive"),
            ("targeted_activity_unknown", "targeted_activity_unknown"),
        ];
        let mut counts = BTreeMap::new();
        for (field, class) in fields {
            let rows = array(source, field, "yield groups")?;
            if rows.iter().any(|row| row.get("classification").and_then(Value::as_str) != Some(class)) {
                return Err("yield classification array was semantically inconsistent".into());
            }
            for row in rows {
                require_target_member(
                    get(object(row, "yield classification")?, "requested_ad_unit_id", "yield classification")?,
                    target_ids,
                    "yield classification target",
                )?;
            }
            counts.insert(class, rows.len());
        }
        let mut expanded = BTreeMap::from([
            ("targeted_exposed", 0_usize),
            ("targeted_and_excluded", 0_usize),
            ("targeted_inactive", 0_usize),
            ("targeted_activity_unknown", 0_usize),
        ]);
        for row in array(source, "target_ad_unit_matches", "yield groups")? {
            let row = object(row, "yield target match")?;
            for id in array(row, "matched_ad_unit_ids", "yield target match")? {
                require_target_member(id, target_ids, "yield matched target")?;
            }
            for (field, class) in [
                ("targeted_exposed_ad_unit_ids", "targeted_exposed"),
                ("targeted_and_excluded_ad_unit_ids", "targeted_and_excluded"),
                ("targeted_inactive_ad_unit_ids", "targeted_inactive"),
                ("targeted_activity_unknown_ad_unit_ids", "targeted_activity_unknown"),
            ] {
                let ids = array(row, field, "yield target match")?;
                for id in ids {
                    require_target_member(id, target_ids, "yield classified target")?;
                }
                *expanded.get_mut(class).expect("known yield class") += ids.len();
            }
        }
        if counts != expanded {
            return Err("yield class counts disagreed with expanded target matches".into());
        }
        result.insert(
            "target_ad_unit_count".into(),
            json!(array(source, "target_ad_unit_ids", "yield groups")?.len()),
        );
        result.insert(
            "target_ad_unit_match_count".into(),
            json!(array(source, "target_ad_unit_matches", "yield groups")?.len()),
        );
        result.insert("targeting_class_counts".into(), json!(counts));
    }
    Ok(Value::Object(result))
}

fn exchange_discovery(full: &Value, path: &str, ledger: &mut Ledger) -> Result<Value, String> {
    let source = object(full, "REST discovery")?;
    let mut result = select(
        source,
        path,
        &[
            "surface",
            "proof_state",
            "resource_count",
            "block_class",
            "error_truncated",
            "hint_truncated",
        ],
        &[
            ("interesting_resources", Class::Array),
            ("error", Class::Reason),
            ("hint", Class::Reason),
        ],
        ledger,
    )?;
    text(source, "proof_state", "REST discovery")?;
    validate_truncation_pairs(
        source,
        &[("error", "error_truncated"), ("hint", "hint_truncated")],
        "REST discovery",
    )?;
    if source.get("interesting_resources").is_some() {
        let interesting = array(source, "interesting_resources", "REST discovery")?;
        if interesting.len() > count(source, "resource_count", "REST discovery")? {
            return Err("interesting discovery resources exceeded total resources".into());
        }
        result.insert(
            "interesting_resource_count".into(),
            json!(interesting.len()),
        );
    }
    Ok(Value::Object(result))
}
