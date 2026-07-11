use super::*;
use crate::evidence::exact_ad_unit_id_from_resource_name;

#[derive(Debug, Clone)]
struct ExchangeTarget {
    ad_unit_id: String,
    ancestor_ad_unit_ids: BTreeSet<String>,
}

#[derive(Debug)]
struct ExchangeAdUnitSemantics {
    proof_state: &'static str,
    decision: &'static str,
    proof_complete: bool,
    target: Option<ExchangeTarget>,
}

#[derive(Debug, Clone, Copy)]
struct ExchangeCollectionSemantics {
    proof_state: &'static str,
    has_observed_rows: bool,
}

#[derive(Debug)]
struct RestDiscoverySemantics {
    interesting_resources: Vec<String>,
}

pub(super) fn project_exchange(full: &Value) -> Result<(Value, Vec<Value>), String> {
    let root = object(full, "exchange probe")?;
    exact_keys(
        root,
        &[
            "network_code",
            "overall_decision",
            "ad_units",
            "private_auctions",
            "private_auction_deals",
            "yield_groups",
            "rest_discovery",
            "unsupported_or_unintegrated_surfaces",
            "attention_reasons",
            "partial_reasons",
            "certainty",
            "result_fingerprint",
            "evidence_receipt_template",
        ],
        "exchange probe",
    )?;
    let network_code = text(root, "network_code", "exchange probe")?;
    if !canonical_numeric_id(network_code) {
        return Err("exchange network code was not a canonical positive id".into());
    }
    text(root, "overall_decision", "exchange probe")?;
    let certainty = object(get(root, "certainty", "exchange probe")?, "certainty")?;
    let ad_units = array(root, "ad_units", "exchange probe")?;
    let unsupported = array(
        root,
        "unsupported_or_unintegrated_surfaces",
        "exchange probe",
    )?;
    let attention = array(root, "attention_reasons", "exchange probe")?;
    let partial = array(root, "partial_reasons", "exchange probe")?;
    let private_auctions = get(root, "private_auctions", "exchange probe")?;
    let private_auction_deals = get(root, "private_auction_deals", "exchange probe")?;
    let yield_group_source = get(root, "yield_groups", "exchange probe")?;
    let rest_discovery = get(root, "rest_discovery", "exchange probe")?;
    let ad_unit_semantics = validate_exchange_ad_units(network_code, ad_units)?;
    let auction_semantics = validate_exchange_collection(private_auctions, "private_auctions")?;
    let deal_semantics =
        validate_exchange_collection(private_auction_deals, "private_auction_deals")?;
    let rest_semantics = validate_rest_discovery(rest_discovery)?;
    validate_unsupported_exchange_surfaces(&rest_semantics, unsupported)?;
    for reason in attention.iter().chain(partial) {
        if reason.as_str().is_none() {
            return Err("exchange reason surface contained non-text evidence".into());
        }
    }
    validate_exchange_certainty(
        certainty,
        &ad_unit_semantics,
        auction_semantics,
        deal_semantics,
        yield_group_source,
    )?;
    let (surface_attention, surface_partial) = exchange_surface_severity(
        &ad_unit_semantics,
        auction_semantics,
        deal_semantics,
        yield_group_source,
        rest_discovery,
        unsupported,
    )?;
    if surface_attention != !attention.is_empty() || surface_partial != !partial.is_empty() {
        return Err("exchange reason categories contradicted the retained proof surfaces".into());
    }
    let expected_decision = if surface_attention {
        "attention_required"
    } else if surface_partial {
        "partial_api_proof"
    } else {
        "api_exposed_surfaces_clear"
    };
    if text(root, "overall_decision", "exchange probe")? != expected_decision {
        return Err("exchange decision contradicted the retained reason surfaces".into());
    }
    let target_identity = target_identity_summary(ad_units)?;
    let targets = exchange_targets(&ad_unit_semantics)?;
    let mut ledger = Ledger::default();
    ledger.omit(
        "/ad_units",
        get(root, "ad_units", "exchange probe")?,
        Class::Array,
    )?;
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
        private_auctions,
        "private_auctions",
        "/private_auctions",
        &mut ledger,
    )?;
    let deals = exchange_collection(
        private_auction_deals,
        "private_auction_deals",
        "/private_auction_deals",
        &mut ledger,
    )?;
    let yield_groups = exchange_yield(yield_group_source, "/yield_groups", &targets, &mut ledger)?;
    let discovery = exchange_discovery(rest_discovery, "/rest_discovery", &mut ledger)?;
    Ok((
        json!({
            "network_code": root["network_code"],
            "overall_decision": root["overall_decision"],
            "ad_units_summary": {
                "source_count": ad_units.len(),
                "identity": target_identity,
                "proof_state_counts": exchange_ad_unit_string_counts(&ad_unit_semantics, |row| row.proof_state),
                "decision_counts": exchange_ad_unit_string_counts(&ad_unit_semantics, |row| row.decision),
                "proof_complete_counts": exchange_ad_unit_bool_counts(&ad_unit_semantics),
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

fn canonical_numeric_id(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| ch.is_ascii_digit())
        && value
            .parse::<u64>()
            .is_ok_and(|id| id > 0 && id.to_string() == value)
}

fn nullable_bool(
    source: &serde_json::Map<String, Value>,
    field: &str,
    name: &str,
) -> Result<Option<bool>, String> {
    match get(source, field, name)? {
        Value::Bool(value) => Ok(Some(*value)),
        Value::Null => Ok(None),
        _ => Err(format!("{name}.{field} was not boolean or null")),
    }
}

fn nullable_text<'a>(
    source: &'a serde_json::Map<String, Value>,
    field: &str,
    name: &str,
) -> Result<Option<&'a str>, String> {
    match get(source, field, name)? {
        Value::String(value) => Ok(Some(value)),
        Value::Null => Ok(None),
        _ => Err(format!("{name}.{field} was not text or null")),
    }
}

fn nullable_text_with_truncation<'a>(
    source: &'a serde_json::Map<String, Value>,
    field: &str,
    truncated_field: &str,
    name: &str,
) -> Result<Option<&'a str>, String> {
    let value = nullable_text(source, field, name)?;
    let truncated = flag(source, truncated_field, name)?;
    if value.is_none() && truncated {
        return Err(format!(
            "{name}.{field} was null while {truncated_field} was true"
        ));
    }
    Ok(value)
}

fn validate_exchange_ad_units(
    network_code: &str,
    rows: &[Value],
) -> Result<Vec<ExchangeAdUnitSemantics>, String> {
    if !(1..=50).contains(&rows.len()) {
        return Err("exchange probe target count exceeded producer bounds".into());
    }
    rows.iter()
        .map(|row| validate_exchange_ad_unit(network_code, row))
        .collect()
}

fn validate_exchange_ad_unit(
    network_code: &str,
    row: &Value,
) -> Result<ExchangeAdUnitSemantics, String> {
    let source = object(row, "exchange ad unit")?;
    let proof_state = text(source, "proof_state", "exchange ad unit")?;
    match proof_state {
        "missing" | "ambiguous" => {
            exact_keys(
                source,
                &[
                    "ad_unit_code",
                    "decision",
                    "proof_state",
                    "proof_complete",
                    "reason",
                    "matches",
                ],
                "unresolved exchange ad unit",
            )?;
            text(source, "ad_unit_code", "exchange ad unit")?;
            text(source, "reason", "exchange ad unit")?;
            let matches = count(source, "matches", "exchange ad unit")?;
            if (proof_state == "missing" && matches != 0)
                || (proof_state == "ambiguous" && matches < 2)
            {
                return Err("exchange ad-unit resolution count contradicted its variant".into());
            }
            if text(source, "decision", "exchange ad unit")? != "attention_required"
                || flag(source, "proof_complete", "exchange ad unit")?
            {
                return Err("unresolved exchange ad unit contradicted producer semantics".into());
            }
            Ok(ExchangeAdUnitSemantics {
                proof_state: if proof_state == "missing" {
                    "missing"
                } else {
                    "ambiguous"
                },
                decision: "attention_required",
                proof_complete: false,
                target: None,
            })
        }
        "resolved_exact" | "invalid_resource_name" => {
            exact_keys(
                source,
                &[
                    "ad_unit_code",
                    "ad_unit_id",
                    "proof_state",
                    "ancestor_ad_unit_ids",
                    "ancestor_identity_complete",
                    "resource_name",
                    "display_name",
                    "status",
                    "ad_unit_sizes",
                    "applied_adsense_enabled",
                    "effective_adsense_enabled",
                    "explicitly_targeted",
                    "decision",
                    "proof_complete",
                ],
                "resolved exchange ad unit",
            )?;
            text(source, "ad_unit_code", "exchange ad unit")?;
            let resource_name = text(source, "resource_name", "exchange ad unit")?;
            let derived_id = exact_ad_unit_id_from_resource_name(network_code, resource_name);
            let expected_state = if derived_id.is_some() {
                "resolved_exact"
            } else {
                "invalid_resource_name"
            };
            let expected_id = derived_id
                .as_ref()
                .map_or(Value::Null, |id| Value::String(id.clone()));
            if proof_state != expected_state
                || get(source, "ad_unit_id", "exchange ad unit")? != &expected_id
            {
                return Err("exchange ad-unit identity contradicted its resource name".into());
            }

            let ancestor_values = array(source, "ancestor_ad_unit_ids", "exchange ad unit")?;
            let ancestor_ids = canonical_id_set(ancestor_values, "exchange ad-unit ancestors")?;
            let source_ancestor_order = ancestor_values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            if source_ancestor_order != ancestor_ids.iter().cloned().collect::<Vec<_>>() {
                return Err("exchange ad-unit ancestors were not in producer order".into());
            }
            let ancestor_identity_complete =
                flag(source, "ancestor_identity_complete", "exchange ad unit")?;
            let applied_adsense =
                nullable_bool(source, "applied_adsense_enabled", "exchange ad unit")?;
            let effective_adsense =
                nullable_bool(source, "effective_adsense_enabled", "exchange ad unit")?;
            nullable_bool(source, "explicitly_targeted", "exchange ad unit")?;

            let proof_complete = derived_id.is_some()
                && ancestor_identity_complete
                && applied_adsense.is_some()
                && effective_adsense.is_some();
            let decision = if derived_id.is_none() {
                "partial_api_proof"
            } else if applied_adsense == Some(true) || effective_adsense == Some(true) {
                "attention_required"
            } else if proof_complete {
                "clear_on_exposed_flags"
            } else {
                "partial_api_proof"
            };
            if flag(source, "proof_complete", "exchange ad unit")? != proof_complete
                || text(source, "decision", "exchange ad unit")? != decision
            {
                return Err(
                    "exchange ad-unit decision contradicted retained producer evidence".into(),
                );
            }
            Ok(ExchangeAdUnitSemantics {
                proof_state: expected_state,
                decision,
                proof_complete,
                target: derived_id.map(|ad_unit_id| ExchangeTarget {
                    ad_unit_id,
                    ancestor_ad_unit_ids: ancestor_ids,
                }),
            })
        }
        _ => Err("exchange ad unit did not match a producer row variant".into()),
    }
}

fn exchange_targets(rows: &[ExchangeAdUnitSemantics]) -> Result<Vec<ExchangeTarget>, String> {
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    for target in rows.iter().filter_map(|row| row.target.as_ref()) {
        if !seen.insert(target.ad_unit_id.clone()) {
            return Err("exchange ad-unit identities contained a duplicate canonical id".into());
        }
        targets.push(target.clone());
    }
    Ok(targets)
}

fn exchange_ad_unit_string_counts(
    rows: &[ExchangeAdUnitSemantics],
    field: fn(&ExchangeAdUnitSemantics) -> &'static str,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(field(row).to_string()).or_default() += 1;
    }
    counts
}

fn exchange_ad_unit_bool_counts(rows: &[ExchangeAdUnitSemantics]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::from([("false", 0_usize), ("true", 0), ("unknown", 0)]);
    for row in rows {
        let key = if row.proof_complete { "true" } else { "false" };
        *counts.get_mut(key).expect("known bool key") += 1;
    }
    counts
}

fn exchange_surface_severity(
    ad_units: &[ExchangeAdUnitSemantics],
    private_auctions: ExchangeCollectionSemantics,
    private_auction_deals: ExchangeCollectionSemantics,
    yield_groups: &Value,
    rest_discovery: &Value,
    unsupported: &[Value],
) -> Result<(bool, bool), String> {
    let yield_groups = object(yield_groups, "yield groups")?;
    let yield_state = text(yield_groups, "proof_state", "yield groups")?;
    let yield_decision = yield_groups.get("decision").and_then(Value::as_str);
    let rest_state = text(
        object(rest_discovery, "REST discovery")?,
        "proof_state",
        "REST discovery",
    )?;
    let attention = ad_units
        .iter()
        .any(|row| row.decision == "attention_required")
        || private_auctions.has_observed_rows
        || private_auction_deals.has_observed_rows
        || yield_decision == Some("targeted_exposed");
    let partial = !unsupported.is_empty()
        || ad_units.iter().any(|row| !row.proof_complete)
        || matches!(private_auctions.proof_state, "sample_only" | "blocked")
        || matches!(private_auction_deals.proof_state, "sample_only" | "blocked")
        || matches!(yield_state, "sample_only" | "blocked" | "skipped")
        || yield_decision == Some("targeted_activity_unknown")
        || rest_state == "blocked";
    Ok((attention, partial))
}

fn validate_exchange_collection(
    value: &Value,
    expected_collection: &str,
) -> Result<ExchangeCollectionSemantics, String> {
    let source = object(value, "exchange collection")?;
    let state = text(source, "proof_state", "exchange collection")?;
    if state == "blocked" {
        exact_keys(
            source,
            &[
                "surface",
                "proof_state",
                "block_class",
                "error",
                "error_truncated",
                "hint",
                "hint_truncated",
            ],
            "blocked exchange collection",
        )?;
        if text(source, "surface", "exchange collection")? != expected_collection
            || !matches!(
                text(source, "block_class", "exchange collection")?,
                "permission" | "upstream"
            )
        {
            return Err("blocked exchange collection contradicted producer semantics".into());
        }
        text(source, "error", "exchange collection")?;
        text(source, "hint", "exchange collection")?;
        flag(source, "error_truncated", "exchange collection")?;
        flag(source, "hint_truncated", "exchange collection")?;
        return Ok(ExchangeCollectionSemantics {
            proof_state: "blocked",
            has_observed_rows: false,
        });
    }

    exact_keys(
        source,
        &[
            "collection",
            "proof_state",
            "row_count_in_page",
            "page_size",
            "next_page_token_present",
            "capped_or_possibly_more",
            "sample",
        ],
        "exchange collection",
    )?;
    if text(source, "collection", "exchange collection")? != expected_collection {
        return Err("exchange collection identity changed".into());
    }
    let row_count = count(source, "row_count_in_page", "exchange collection")?;
    let page_size = count(source, "page_size", "exchange collection")?;
    if !(1..=1_000).contains(&page_size) {
        return Err("exchange collection page size was outside producer bounds".into());
    }
    let next_page = flag(source, "next_page_token_present", "exchange collection")?;
    let capped = next_page || row_count >= page_size;
    if flag(source, "capped_or_possibly_more", "exchange collection")? != capped {
        return Err("exchange collection cap evidence contradicted producer progress".into());
    }
    let expected_state = if capped {
        "sample_only"
    } else if row_count == 0 {
        "complete_empty"
    } else {
        "complete_present"
    };
    if state != expected_state {
        return Err("exchange collection proof state contradicted producer progress".into());
    }
    let sample = array(source, "sample", "exchange collection")?;
    if sample.len() != row_count.min(5) {
        return Err("exchange collection sample cardinality contradicted its row count".into());
    }
    for row in sample {
        let row = object(row, "exchange collection sample")?;
        exact_keys(
            row,
            &["resource_name", "resource_id", "display_name", "status"],
            "exchange collection sample",
        )?;
        let resource_name = text(row, "resource_name", "exchange collection sample")?;
        let expected_resource_id = resource_name.rsplit('/').next().unwrap_or_default();
        if text(row, "resource_id", "exchange collection sample")? != expected_resource_id {
            return Err("exchange collection sample resource id was not producer-derived".into());
        }
    }
    Ok(ExchangeCollectionSemantics {
        proof_state: expected_state,
        has_observed_rows: row_count > 0,
    })
}

fn validate_exchange_certainty(
    certainty: &serde_json::Map<String, Value>,
    ad_units: &[ExchangeAdUnitSemantics],
    private_auctions: ExchangeCollectionSemantics,
    private_auction_deals: ExchangeCollectionSemantics,
    yield_groups: &Value,
) -> Result<(), String> {
    exact_keys(
        certainty,
        &[
            "can_prove_requested_ad_unit_flags",
            "can_prove_private_auction_absence_or_presence",
            "can_prove_private_deal_absence_or_presence",
            "can_prove_yield_group_targeting",
            "cannot_prove_via_current_api",
        ],
        "exchange certainty",
    )?;
    let expected = [
        (
            "can_prove_requested_ad_unit_flags",
            ad_units.iter().all(|row| row.proof_complete),
        ),
        (
            "can_prove_private_auction_absence_or_presence",
            matches!(
                private_auctions.proof_state,
                "complete_empty" | "complete_present"
            ),
        ),
        (
            "can_prove_private_deal_absence_or_presence",
            matches!(
                private_auction_deals.proof_state,
                "complete_empty" | "complete_present"
            ),
        ),
        (
            "can_prove_yield_group_targeting",
            text(
                object(yield_groups, "yield groups")?,
                "proof_state",
                "yield groups",
            )? == "complete",
        ),
    ];
    for (key, expected_value) in expected {
        if flag(certainty, key, "exchange certainty")? != expected_value {
            return Err("exchange certainty contradicted the retained proof surfaces".into());
        }
    }
    let unsupported = array(
        certainty,
        "cannot_prove_via_current_api",
        "exchange certainty",
    )?;
    let expected_unsupported = ["protections", "inventory_rules", "unified_pricing_rules"];
    if unsupported.len() != expected_unsupported.len()
        || unsupported
            .iter()
            .zip(expected_unsupported)
            .any(|(actual, expected)| actual.as_str() != Some(expected))
    {
        return Err("exchange certainty changed the unsupported proof surfaces".into());
    }
    Ok(())
}

fn exchange_collection(
    full: &Value,
    expected_collection: &str,
    path: &str,
    ledger: &mut Ledger,
) -> Result<Value, String> {
    validate_exchange_collection(full, expected_collection)?;
    let source = object(full, "exchange collection")?;
    let mut result = select(
        source,
        path,
        &[
            "collection",
            "surface",
            "proof_state",
            "row_count_in_page",
            "page_size",
            "next_page_token_present",
            "capped_or_possibly_more",
            "block_class",
            "error_truncated",
            "hint_truncated",
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
        let sample = sample
            .as_array()
            .ok_or("collection sample was not an array")?;
        if sample.len() != count(source, "row_count_in_page", "exchange collection")?.min(5) {
            return Err("collection row count and omitted sample were inconsistent".into());
        }
        result.insert("sample_count".into(), json!(sample.len()));
    }
    Ok(Value::Object(result))
}

fn exchange_yield(
    full: &Value,
    path: &str,
    targets: &[ExchangeTarget],
    ledger: &mut Ledger,
) -> Result<Value, String> {
    let source = object(full, "yield groups")?;
    let normal_shape = validate_exchange_yield_shape(source)?;
    let proof_state = text(source, "proof_state", "yield groups")?;
    if (targets.is_empty() && proof_state != "skipped")
        || (!targets.is_empty() && proof_state == "skipped")
    {
        return Err("yield result variant contradicted resolved producer targets".into());
    }
    let mut result = select(
        source,
        path,
        &[
            "surface",
            "decision",
            "proof_state",
            "total_result_set_size",
            "inspected_results",
            "response_truncated",
            "mutation_performed",
            "block_class",
            "upstream_status",
            "request_id_truncated",
            "response_time_truncated",
            "soap_fault_truncated",
            "message_truncated",
            "current_scope_truncated",
            "error_truncated",
            "hint_truncated",
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
    if normal_shape {
        false_field(source, "mutation_performed", "yield groups")?;
        nullable_text_with_truncation(
            source,
            "request_id",
            "request_id_truncated",
            "yield groups",
        )?;
        nullable_text_with_truncation(
            source,
            "response_time",
            "response_time_truncated",
            "yield groups",
        )?;
        if let Some(raw) = source.get("upstream_response_xml")
            && raw.as_str().is_none()
        {
            return Err("yield raw response was not text".into());
        }
        let expected_target_ids = targets
            .iter()
            .map(|target| Value::String(target.ad_unit_id.clone()))
            .collect::<Vec<_>>();
        if array(source, "target_ad_unit_ids", "yield groups")? != &expected_target_ids {
            return Err("yield target ids disagreed with the producer target order".into());
        }
        let inspected = count(source, "inspected_results", "yield groups")?;
        let target_matches = array(source, "target_ad_unit_matches", "yield groups")?;
        if target_matches.len() > inspected {
            return Err("yield target-match rows exceeded inspected producer results".into());
        }
        let derived_classifications = derive_yield_classifications(target_matches, targets)?;
        let mut counts = BTreeMap::new();
        for field in [
            "targeted_exposed",
            "targeted_and_excluded",
            "targeted_inactive",
            "targeted_activity_unknown",
        ] {
            let expected = derived_classifications
                .get(field)
                .expect("known yield classification");
            if array(source, field, "yield groups")? != expected {
                return Err(
                    "yield classification contradicted raw targeting and activity evidence".into(),
                );
            }
            counts.insert(field, expected.len());
        }
        let response_truncated = flag(source, "response_truncated", "yield groups")?;
        let total = match source.get("total_result_set_size") {
            None | Some(Value::Null) => None,
            Some(Value::Number(value)) => Some(
                value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or("yield total result count was invalid")?,
            ),
            Some(_) => return Err("yield total result count was invalid".into()),
        };
        if total.is_some_and(|total| inspected > total) {
            return Err("yield inspected results exceeded the reported total".into());
        }
        let sample_only = response_truncated
            || total.is_none()
            || total.is_some_and(|total| total > inspected);
        let expected_proof_state = if sample_only {
            "sample_only"
        } else {
            "complete"
        };
        let expected_decision = if counts["targeted_exposed"] > 0 {
            "targeted_exposed"
        } else if counts["targeted_and_excluded"] > 0 {
            "targeted_and_excluded"
        } else if counts["targeted_activity_unknown"] > 0 {
            "targeted_activity_unknown"
        } else if counts["targeted_inactive"] > 0 {
            "targeted_inactive"
        } else if sample_only {
            "sample_only"
        } else {
            "no_target_matches"
        };
        if text(source, "proof_state", "yield groups")? != expected_proof_state
            || text(source, "decision", "yield groups")? != expected_decision
        {
            return Err("yield decision contradicted the retained targeting evidence".into());
        }
        result.insert("target_ad_unit_count".into(), json!(targets.len()));
        result.insert(
            "target_ad_unit_match_count".into(),
            json!(target_matches.len()),
        );
        result.insert("targeting_class_counts".into(), json!(counts));
    }
    Ok(Value::Object(result))
}

#[derive(Debug)]
struct YieldTargetingValue {
    ad_unit_id: String,
    include_descendants: Option<bool>,
}

#[derive(Debug)]
struct YieldCoverage {
    ad_unit_id: String,
    include_descendants: Option<bool>,
    match_type: &'static str,
}

fn derive_yield_classifications(
    rows: &[Value],
    targets: &[ExchangeTarget],
) -> Result<BTreeMap<&'static str, Vec<Value>>, String> {
    let mut classifications = BTreeMap::from([
        ("targeted_exposed", Vec::new()),
        ("targeted_and_excluded", Vec::new()),
        ("targeted_inactive", Vec::new()),
        ("targeted_activity_unknown", Vec::new()),
    ]);
    for row in rows {
        let source = object(row, "yield target match")?;
        exact_keys(
            source,
            &[
                "yield_group_id",
                "yield_group_name",
                "status",
                "activity_state",
                "format",
                "environment_type",
                "matched_ad_unit_ids",
                "targeted_exposed_ad_unit_ids",
                "targeted_and_excluded_ad_unit_ids",
                "targeted_inactive_ad_unit_ids",
                "targeted_activity_unknown_ad_unit_ids",
                "targeted_ad_units",
                "excluded_ad_units",
            ],
            "yield target match",
        )?;
        nullable_text(source, "yield_group_id", "yield target match")?;
        nullable_text(source, "yield_group_name", "yield target match")?;
        let status = nullable_text(source, "status", "yield target match")?;
        nullable_text(source, "format", "yield target match")?;
        nullable_text(source, "environment_type", "yield target match")?;
        let activity_state = yield_activity_state(status);
        if text(source, "activity_state", "yield target match")? != activity_state {
            return Err("yield activity state contradicted raw status evidence".into());
        }
        let targeted = yield_targeting_values(source, "targeted_ad_units")?;
        let excluded = yield_targeting_values(source, "excluded_ad_units")?;
        let mut matched_ids = Vec::new();
        let mut classified_ids = BTreeMap::from([
            ("targeted_exposed", Vec::new()),
            ("targeted_and_excluded", Vec::new()),
            ("targeted_inactive", Vec::new()),
            ("targeted_activity_unknown", Vec::new()),
        ]);

        for target in targets {
            let direct_targeting = yield_coverage_for_target(&targeted, target);
            let exclusion = yield_coverage_for_target(&excluded, target);
            let broad_targeting = if direct_targeting.is_none() && exclusion.is_some() {
                yield_broad_descendant_coverage(&targeted)
            } else {
                None
            };
            let targeting = direct_targeting.or(broad_targeting);
            let Some(classification) =
                yield_classification(targeting.as_ref(), exclusion.as_ref(), activity_state)
            else {
                continue;
            };
            matched_ids.push(Value::String(target.ad_unit_id.clone()));
            classified_ids
                .get_mut(classification)
                .expect("known yield classification")
                .push(Value::String(target.ad_unit_id.clone()));
            classifications
                .get_mut(classification)
                .expect("known yield classification")
                .push(json!({
                    "yield_group_id": get(source, "yield_group_id", "yield target match")?,
                    "yield_group_name": get(source, "yield_group_name", "yield target match")?,
                    "status": get(source, "status", "yield target match")?,
                    "activity_state": activity_state,
                    "format": get(source, "format", "yield target match")?,
                    "environment_type": get(source, "environment_type", "yield target match")?,
                    "requested_ad_unit_id": target.ad_unit_id.clone(),
                    "classification": classification,
                    "targeting_match": targeting.as_ref().map(yield_coverage_json).unwrap_or(Value::Null),
                    "exclusion_match": exclusion.as_ref().map(yield_coverage_json).unwrap_or(Value::Null),
                }));
        }
        if matched_ids.is_empty() {
            return Err("yield target-match row had no producer-derived target match".into());
        }
        if get(source, "matched_ad_unit_ids", "yield target match")? != &Value::Array(matched_ids) {
            return Err("yield matched targets contradicted raw producer evidence".into());
        }
        for (field, classification) in [
            ("targeted_exposed_ad_unit_ids", "targeted_exposed"),
            ("targeted_and_excluded_ad_unit_ids", "targeted_and_excluded"),
            ("targeted_inactive_ad_unit_ids", "targeted_inactive"),
            (
                "targeted_activity_unknown_ad_unit_ids",
                "targeted_activity_unknown",
            ),
        ] {
            if get(source, field, "yield target match")?
                != &Value::Array(
                    classified_ids
                        .remove(classification)
                        .expect("known yield classification"),
                )
            {
                return Err("yield per-result classification contradicted raw evidence".into());
            }
        }
    }
    Ok(classifications)
}

fn yield_targeting_values(
    source: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Vec<YieldTargetingValue>, String> {
    array(source, field, "yield target match")?
        .iter()
        .map(|value| {
            let value = object(value, "yield targeting value")?;
            exact_keys(
                value,
                &["ad_unit_id", "include_descendants"],
                "yield targeting value",
            )?;
            Ok(YieldTargetingValue {
                ad_unit_id: text(value, "ad_unit_id", "yield targeting value")?.to_string(),
                include_descendants: nullable_bool(
                    value,
                    "include_descendants",
                    "yield targeting value",
                )?,
            })
        })
        .collect()
}

fn yield_coverage_for_target(
    values: &[YieldTargetingValue],
    target: &ExchangeTarget,
) -> Option<YieldCoverage> {
    values
        .iter()
        .find(|value| value.ad_unit_id == target.ad_unit_id)
        .map(|value| YieldCoverage {
            ad_unit_id: value.ad_unit_id.clone(),
            include_descendants: value.include_descendants,
            match_type: "exact",
        })
        .or_else(|| {
            values
                .iter()
                .find(|value| {
                    value.include_descendants == Some(true)
                        && target.ancestor_ad_unit_ids.contains(&value.ad_unit_id)
                })
                .map(|value| YieldCoverage {
                    ad_unit_id: value.ad_unit_id.clone(),
                    include_descendants: value.include_descendants,
                    match_type: "ancestor_descendant",
                })
        })
}

fn yield_broad_descendant_coverage(values: &[YieldTargetingValue]) -> Option<YieldCoverage> {
    values
        .iter()
        .find(|value| value.include_descendants == Some(true))
        .map(|value| YieldCoverage {
            ad_unit_id: value.ad_unit_id.clone(),
            include_descendants: value.include_descendants,
            match_type: "broad_descendant_target_unresolved_hierarchy",
        })
}

fn yield_activity_state(status: Option<&str>) -> &'static str {
    match status.map(|value| value.trim().to_ascii_uppercase()) {
        Some(value) if value == "ACTIVE" => "active",
        Some(value)
            if matches!(
                value.as_str(),
                "INACTIVE" | "ARCHIVED" | "DELETED" | "PAUSED" | "DRAFT"
            ) =>
        {
            "inactive"
        }
        _ => "unknown",
    }
}

fn yield_classification(
    targeting: Option<&YieldCoverage>,
    exclusion: Option<&YieldCoverage>,
    activity_state: &str,
) -> Option<&'static str> {
    if targeting.is_some() && exclusion.is_some() {
        return Some("targeted_and_excluded");
    }
    targeting?;
    match activity_state {
        "active" => Some("targeted_exposed"),
        "inactive" => Some("targeted_inactive"),
        _ => Some("targeted_activity_unknown"),
    }
}

fn yield_coverage_json(coverage: &YieldCoverage) -> Value {
    json!({
        "ad_unit_id": coverage.ad_unit_id.clone(),
        "include_descendants": coverage.include_descendants,
        "match_type": coverage.match_type,
    })
}

fn validate_exchange_yield_shape(source: &serde_json::Map<String, Value>) -> Result<bool, String> {
    if text(source, "surface", "yield groups")? != "yield_groups" {
        return Err("yield surface identity changed".into());
    }
    let proof_state = text(source, "proof_state", "yield groups")?;
    match proof_state {
        "complete" | "sample_only" => {
            exact_required_optional_keys(
                source,
                &[
                    "surface",
                    "decision",
                    "proof_state",
                    "request_id",
                    "request_id_truncated",
                    "response_time",
                    "response_time_truncated",
                    "total_result_set_size",
                    "inspected_results",
                    "response_truncated",
                    "target_ad_unit_ids",
                    "target_ad_unit_matches",
                    "targeted_exposed",
                    "targeted_and_excluded",
                    "targeted_inactive",
                    "targeted_activity_unknown",
                    "mutation_performed",
                ],
                &["upstream_response_xml"],
                "normal yield result",
            )?;
            Ok(true)
        }
        "skipped" => {
            exact_required_optional_keys(
                source,
                &[
                    "surface",
                    "decision",
                    "proof_state",
                    "reason",
                    "mutation_performed",
                ],
                &[],
                "skipped yield result",
            )?;
            if text(source, "decision", "yield groups")? != "skipped" {
                return Err("skipped yield result had a contradictory decision".into());
            }
            text(source, "reason", "yield groups")?;
            false_field(source, "mutation_performed", "yield groups")?;
            Ok(false)
        }
        "blocked" if source.contains_key("upstream_status") => {
            exact_required_optional_keys(
                source,
                &[
                    "surface",
                    "decision",
                    "proof_state",
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
                    "mutation_performed",
                ],
                &[],
                "blocked SOAP yield result",
            )?;
            if text(source, "decision", "yield groups")? != "blocked" {
                return Err("blocked SOAP yield result had a contradictory decision".into());
            }
            let block_class = text(source, "block_class", "yield groups")?;
            if !matches!(block_class, "permission" | "upstream") {
                return Err("blocked SOAP yield result had an invalid block class".into());
            }
            let upstream_status = count(source, "upstream_status", "yield groups")?;
            nullable_text_with_truncation(
                source,
                "request_id",
                "request_id_truncated",
                "yield groups",
            )?;
            nullable_text_with_truncation(
                source,
                "response_time",
                "response_time_truncated",
                "yield groups",
            )?;
            let soap_fault = nullable_text_with_truncation(
                source,
                "soap_fault",
                "soap_fault_truncated",
                "yield groups",
            )?;
            let message = text(source, "message", "yield groups")?;
            if upstream_status < 400 && soap_fault.is_none() {
                return Err("blocked SOAP yield result lacked an HTTP error or SOAP fault".into());
            }
            let permission_proven = matches!(upstream_status, 401 | 403)
                || retained_soap_permission_evidence(soap_fault, message);
            if permission_proven && block_class != "permission" {
                return Err(
                    "blocked SOAP yield result contradicted retained permission evidence".into(),
                );
            }
            false_field(source, "mutation_performed", "yield groups")?;
            Ok(false)
        }
        "blocked" if source.contains_key("required_scope") => {
            exact_required_optional_keys(
                source,
                &[
                    "surface",
                    "decision",
                    "proof_state",
                    "block_class",
                    "reason",
                    "required_scope",
                    "current_scope",
                    "current_scope_truncated",
                    "mutation_performed",
                ],
                &[],
                "permission-blocked yield result",
            )?;
            if text(source, "decision", "yield groups")? != "blocked"
                || text(source, "block_class", "yield groups")? != "permission"
            {
                return Err("permission-blocked yield result had contradictory state".into());
            }
            text(source, "reason", "yield groups")?;
            text(source, "required_scope", "yield groups")?;
            text(source, "current_scope", "yield groups")?;
            flag(source, "current_scope_truncated", "yield groups")?;
            false_field(source, "mutation_performed", "yield groups")?;
            Ok(false)
        }
        "blocked" => {
            exact_required_optional_keys(
                source,
                &[
                    "surface",
                    "proof_state",
                    "block_class",
                    "error",
                    "error_truncated",
                    "hint",
                    "hint_truncated",
                ],
                &[],
                "preflight-blocked yield result",
            )?;
            if !matches!(
                text(source, "block_class", "yield groups")?,
                "permission" | "upstream"
            ) {
                return Err("preflight-blocked yield result had an invalid block class".into());
            }
            text(source, "error", "yield groups")?;
            text(source, "hint", "yield groups")?;
            flag(source, "error_truncated", "yield groups")?;
            flag(source, "hint_truncated", "yield groups")?;
            Ok(false)
        }
        _ => Err("yield proof state did not match a producer result variant".into()),
    }
}

fn exact_required_optional_keys(
    source: &serde_json::Map<String, Value>,
    required: &[&str],
    optional: &[&str],
    name: &str,
) -> Result<(), String> {
    if required.iter().any(|key| !source.contains_key(*key))
        || source
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(format!("{name} fields did not match the producer contract"));
    }
    Ok(())
}

fn validate_rest_discovery(full: &Value) -> Result<RestDiscoverySemantics, String> {
    let source = object(full, "REST discovery")?;
    match text(source, "proof_state", "REST discovery")? {
        "metadata_read" => {
            exact_keys(
                source,
                &["proof_state", "resource_count", "interesting_resources"],
                "REST discovery metadata",
            )?;
            let resource_count = count(source, "resource_count", "REST discovery")?;
            let interesting = array(source, "interesting_resources", "REST discovery")?;
            if interesting.len() > resource_count {
                return Err("interesting discovery resources exceeded total resources".into());
            }
            let interesting_resources = interesting
                .iter()
                .map(|resource| {
                    resource
                        .as_str()
                        .map(str::to_string)
                        .ok_or("interesting REST discovery resource was not text".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(RestDiscoverySemantics {
                interesting_resources,
            })
        }
        "blocked" => {
            exact_keys(
                source,
                &[
                    "surface",
                    "proof_state",
                    "block_class",
                    "error",
                    "error_truncated",
                    "hint",
                    "hint_truncated",
                ],
                "blocked REST discovery",
            )?;
            if text(source, "surface", "REST discovery")? != "rest_discovery"
                || !matches!(
                    text(source, "block_class", "REST discovery")?,
                    "permission" | "upstream"
                )
            {
                return Err("blocked REST discovery contradicted producer semantics".into());
            }
            text(source, "error", "REST discovery")?;
            text(source, "hint", "REST discovery")?;
            flag(source, "error_truncated", "REST discovery")?;
            flag(source, "hint_truncated", "REST discovery")?;
            Ok(RestDiscoverySemantics {
                interesting_resources: Vec::new(),
            })
        }
        _ => Err("REST discovery did not match a producer result variant".into()),
    }
}

fn validate_unsupported_exchange_surfaces(
    rest_discovery: &RestDiscoverySemantics,
    surfaces: &[Value],
) -> Result<(), String> {
    let resources = rest_discovery
        .interesting_resources
        .iter()
        .map(|resource| resource.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let expected = [
        (
            "protections",
            "protection",
            "GAM protection objects are not implemented as a current MCP read surface.",
        ),
        (
            "inventory_rules",
            "inventoryrule",
            "GAM inventory-rule objects are not implemented as a current MCP read surface.",
        ),
        (
            "unified_pricing_rules",
            "pricing",
            "GAM unified pricing rules are not implemented as a current MCP read surface.",
        ),
    ]
    .into_iter()
    .map(|(surface, needle, note)| {
        let api_exposure = if resources.iter().any(|resource| resource.contains(needle)) {
            "resource_seen_but_not_integrated"
        } else {
            "not_seen_in_rest_discovery"
        };
        json!({
            "surface": surface,
            "proof_state": "not_proven",
            "api_exposure": api_exposure,
            "note": note,
        })
    })
    .collect::<Vec<_>>();
    if surfaces != expected.as_slice() {
        return Err(
            "unsupported exchange surfaces contradicted fixed producer discovery derivation".into(),
        );
    }
    Ok(())
}

fn exchange_discovery(full: &Value, path: &str, ledger: &mut Ledger) -> Result<Value, String> {
    validate_rest_discovery(full)?;
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
