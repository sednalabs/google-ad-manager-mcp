use serde_json::Map;

use super::*;

const PLACEMENT_PAGE_SIZE_MAX: usize = 1_000;
const TARGET_PLACEMENT_ID_LIMIT: usize = 200;
const LINE_ITEM_PAGE_SIZE_MAX: usize = 1_000;
const LINE_ITEM_SCAN_MAX: usize = 5_000;
const TRANSPORT_METADATA_SAMPLE_LIMIT: usize = 50;
const DEPENDENCY_MATCH_SAMPLE_LIMIT: usize = 50;

#[derive(Debug, Default)]
struct DependencyTargetContext {
    ancestor_ad_unit_ids: BTreeSet<String>,
    placement_ids: BTreeSet<String>,
}

#[derive(Debug)]
struct DependencyCoverage<'a> {
    ad_unit_id: &'a str,
    include_descendants: Option<bool>,
    match_type: &'a str,
}

pub(super) fn project_dependency(full: &Value) -> Result<(Value, Vec<Value>), String> {
    let root = object(full, "dependency probe")?;
    exact_keys(
        root,
        &[
            "network_code",
            "dependency_decision",
            "ad_units",
            "placements",
            "line_items",
            "target_resolution_issues",
            "proof_flags",
            "mutation_performed",
            "cleanup_decision",
            "result_fingerprint",
            "evidence_receipt_template",
        ],
        "dependency probe",
    )?;
    let network_code = text(root, "network_code", "dependency probe")?;
    text(root, "dependency_decision", "dependency probe")?;
    false_field(root, "mutation_performed", "dependency probe")?;
    let proof_flags = object(get(root, "proof_flags", "dependency probe")?, "proof flags")?;
    let cleanup = object(
        get(root, "cleanup_decision", "dependency probe")?,
        "cleanup",
    )?;
    exact_keys(
        cleanup,
        &["safe_to_archive_or_retire", "reason"],
        "cleanup decision",
    )?;
    text(cleanup, "reason", "cleanup decision")?;
    if cleanup
        .get("safe_to_archive_or_retire")
        .and_then(Value::as_bool)
        != Some(false)
    {
        return Err("cleanup decision did not prohibit mutation".into());
    }
    let ad_units = array(root, "ad_units", "dependency probe")?;
    let issues = array(root, "target_resolution_issues", "dependency probe")?;
    let issue_strings = issues
        .iter()
        .map(|issue| {
            issue
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "target resolution issue was not text".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_dependency_ad_units(network_code, ad_units, &issue_strings)?;
    let placement_source = get(root, "placements", "dependency probe")?;
    let line_item_source = get(root, "line_items", "dependency probe")?;
    validate_dependency_proof_flags(
        proof_flags,
        ad_units,
        issues,
        placement_source,
        line_item_source,
    )?;
    let expected_decision =
        dependency_probe_decision(&issue_strings, placement_source, line_item_source);
    if text(root, "dependency_decision", "dependency probe")? != expected_decision {
        return Err("dependency decision contradicted the retained proof surfaces".into());
    }
    let target_identity = target_identity_summary(ad_units)?;
    let target_ids = canonical_target_ids(ad_units)?;
    let target_contexts = dependency_target_contexts(ad_units, placement_source)?;
    let mut ledger = Ledger::default();
    ledger.omit(
        "/ad_units",
        get(root, "ad_units", "dependency probe")?,
        Class::Array,
    )?;
    ledger.omit(
        "/target_resolution_issues",
        get(root, "target_resolution_issues", "dependency probe")?,
        Class::Reason,
    )?;
    let placements =
        dependency_placements(placement_source, "/placements", &target_ids, &mut ledger)?;
    let line_items = dependency_line_items(
        line_item_source,
        "/line_items",
        &target_ids,
        &target_contexts,
        &mut ledger,
    )?;
    Ok((
        json!({
            "network_code": root["network_code"],
            "dependency_decision": root["dependency_decision"],
            "ad_units_summary": {
                "source_count": ad_units.len(),
                "identity": target_identity,
                "proof_state_counts": string_counts(ad_units, "proof_state")?,
            },
            "placements": placements,
            "line_items": line_items,
            "target_resolution_issue_count": issues.len(),
            "proof_flags": root["proof_flags"],
            "mutation_performed": false,
            "cleanup_decision": root["cleanup_decision"],
        }),
        ledger.0,
    ))
}

fn validate_dependency_ad_units(
    network_code: &str,
    rows: &[Value],
    issues: &[String],
) -> Result<(), String> {
    if !canonical_id(network_code) {
        return Err("dependency probe network was not canonical".into());
    }
    if !(1..=50).contains(&rows.len()) {
        return Err("dependency probe target count exceeded producer bounds".into());
    }
    let mut expected_issues = Vec::new();
    for row in rows {
        let row = object(row, "dependency ad unit")?;
        match text(row, "proof_state", "dependency ad unit")? {
            "resolved_exact" => {
                exact_keys(
                    row,
                    &[
                        "ad_unit_code",
                        "ad_unit_id",
                        "resource_name",
                        "display_name",
                        "status",
                        "ad_unit_sizes",
                        "ancestor_ad_unit_ids",
                        "ancestor_identity_complete",
                        "proof_state",
                    ],
                    "resolved dependency ad unit",
                )?;
                let code = text(row, "ad_unit_code", "resolved dependency ad unit")?;
                let ad_unit_id = text(row, "ad_unit_id", "resolved dependency ad unit")?;
                let expected_resource_name =
                    format!("networks/{network_code}/adUnits/{ad_unit_id}");
                if !canonical_id(ad_unit_id)
                    || text(row, "resource_name", "resolved dependency ad unit")?
                        != expected_resource_name.as_str()
                {
                    return Err("resolved dependency ad unit identity was not exact".into());
                }
                canonical_id_set(
                    array(row, "ancestor_ad_unit_ids", "resolved dependency ad unit")?,
                    "resolved dependency ancestors",
                )?;
                if !flag(
                    row,
                    "ancestor_identity_complete",
                    "resolved dependency ad unit",
                )? {
                    expected_issues.push(format!(
                        "ad unit code {code} returned malformed or foreign ancestor identities"
                    ));
                }
            }
            "invalid_resource_name" => {
                exact_keys(
                    row,
                    &[
                        "ad_unit_code",
                        "ad_unit_id",
                        "resource_name",
                        "display_name",
                        "status",
                        "ad_unit_sizes",
                        "ancestor_ad_unit_ids",
                        "ancestor_identity_complete",
                        "proof_state",
                        "reason",
                    ],
                    "invalid-resource dependency ad unit",
                )?;
                let code = text(row, "ad_unit_code", "invalid-resource dependency ad unit")?;
                if row.get("ad_unit_id") != Some(&Value::Null) {
                    return Err("invalid-resource dependency ad unit retained an id".into());
                }
                let resource_name =
                    text(row, "resource_name", "invalid-resource dependency ad unit")?;
                let canonical_prefix = format!("networks/{network_code}/adUnits/");
                if resource_name
                    .strip_prefix(canonical_prefix.as_str())
                    .is_some_and(canonical_id)
                {
                    return Err(
                        "invalid-resource dependency ad unit had a canonical resource".into(),
                    );
                }
                canonical_id_set(
                    array(
                        row,
                        "ancestor_ad_unit_ids",
                        "invalid-resource dependency ad unit",
                    )?,
                    "invalid-resource dependency ancestors",
                )?;
                flag(
                    row,
                    "ancestor_identity_complete",
                    "invalid-resource dependency ad unit",
                )?;
                if text(row, "reason", "invalid-resource dependency ad unit")?
                    != "exact ad-unit code resolved outside the requested canonical network/resource scope"
                {
                    return Err(
                        "invalid-resource dependency reason was not producer-defined".into(),
                    );
                }
                expected_issues.push(format!("ad unit code {code} did not resolve exactly"));
            }
            state @ ("missing" | "ambiguous") => {
                exact_keys(
                    row,
                    &["ad_unit_code", "proof_state", "reason", "matches"],
                    "unresolved dependency ad unit",
                )?;
                let code = text(row, "ad_unit_code", "unresolved dependency ad unit")?;
                let reason = text(row, "reason", "unresolved dependency ad unit")?;
                let matches = count(row, "matches", "unresolved dependency ad unit")?;
                if (state == "missing" && matches != 0)
                    || (state == "ambiguous" && matches <= 1)
                    || (state == "missing"
                        && reason != "exact ad-unit code was not returned by GAM")
                    || (state == "ambiguous"
                        && reason != "GAM returned multiple rows for the exact ad-unit code")
                {
                    return Err("unresolved dependency match count contradicted its state".into());
                }
                expected_issues.push(format!("ad unit code {code} did not resolve exactly"));
            }
            "id_only" => {
                exact_keys(
                    row,
                    &[
                        "ad_unit_id",
                        "ad_unit_codes",
                        "resource_name",
                        "display_name",
                        "status",
                        "ad_unit_sizes",
                        "ancestor_ad_unit_ids",
                        "proof_state",
                        "proof_notes",
                    ],
                    "id-only dependency ad unit",
                )?;
                let ad_unit_id = text(row, "ad_unit_id", "id-only dependency ad unit")?;
                if !canonical_id(ad_unit_id)
                    || !array(row, "ad_unit_codes", "id-only dependency ad unit")?.is_empty()
                    || !array(row, "ancestor_ad_unit_ids", "id-only dependency ad unit")?.is_empty()
                    || ["resource_name", "display_name", "status", "ad_unit_sizes"]
                        .iter()
                        .any(|field| row.get(*field) != Some(&Value::Null))
                {
                    return Err("id-only dependency ad unit retained resolved evidence".into());
                }
                let notes = array(row, "proof_notes", "id-only dependency ad unit")?;
                if notes.len() != 1
                    || notes[0].as_str()
                        != Some(
                            "ancestor targeting cannot be proven for an id-only target unless a code row is also resolved",
                        )
                {
                    return Err("id-only dependency proof notes did not match the producer".into());
                }
                expected_issues.push(format!(
                    "ad unit id {ad_unit_id} was supplied without a resolved code row; ancestor targeting proof is incomplete"
                ));
            }
            _ => return Err("dependency ad-unit proof state was not producer-defined".into()),
        }
    }
    if expected_issues.as_slice() != issues {
        return Err(
            "dependency target-resolution issues contradicted the ad-unit producer rows".into(),
        );
    }
    Ok(())
}

fn dependency_target_contexts(
    rows: &[Value],
    placements: &Value,
) -> Result<BTreeMap<String, DependencyTargetContext>, String> {
    let mut contexts = BTreeMap::<String, DependencyTargetContext>::new();
    for row in rows {
        let row = object(row, "dependency ad unit")?;
        let Some(ad_unit_id) = row.get("ad_unit_id").and_then(Value::as_str) else {
            continue;
        };
        let context = contexts.entry(ad_unit_id.to_string()).or_default();
        if let Some(ancestors) = row.get("ancestor_ad_unit_ids") {
            context.ancestor_ad_unit_ids.extend(canonical_id_set(
                ancestors
                    .as_array()
                    .ok_or("dependency ancestors were not an array")?,
                "dependency ancestors",
            )?);
        }
    }
    if let Some(target_map) = placements.get("target_placement_ids_by_ad_unit_id") {
        let target_map = target_map
            .as_object()
            .ok_or("placement target map was not an object")?;
        for (target_id, placement_ids) in target_map {
            let context = contexts
                .get_mut(target_id)
                .ok_or("placement target map escaped dependency target context")?;
            context.placement_ids = canonical_id_set(
                placement_ids
                    .as_array()
                    .ok_or("placement target ids were not an array")?,
                "dependency target placements",
            )?;
        }
    }
    Ok(contexts)
}

fn validate_dependency_proof_flags(
    flags: &Map<String, Value>,
    ad_units: &[Value],
    issues: &[Value],
    placements: &Value,
    line_items: &Value,
) -> Result<(), String> {
    const KEYS: &[&str] = &[
        "target_resolution_incomplete",
        "id_only_targets_have_unknown_ancestors",
        "placements_capped_or_shape_unknown",
        "line_items_capped_or_truncated",
        "soap_manage_scope_required",
        "line_items_blocked",
    ];
    exact_keys(flags, KEYS, "proof flags")?;
    let id_only_targets_have_unknown_ancestors =
        ad_units
            .iter()
            .try_fold(false, |found, row| -> Result<bool, String> {
                let row = object(row, "dependency ad unit")?;
                if row.get("proof_state").and_then(Value::as_str) != Some("id_only") {
                    return Ok(found);
                }
                let codes = array(row, "ad_unit_codes", "id-only dependency ad unit")?;
                let ancestors = array(row, "ancestor_ad_unit_ids", "id-only dependency ad unit")?;
                if !codes.is_empty() || !ancestors.is_empty() {
                    return Err(
                        "id-only dependency target had resolved code or ancestor evidence".into(),
                    );
                }
                Ok(true)
            })?;
    let placements = object(placements, "placements")?;
    let line_items = object(line_items, "line items")?;
    let placement_state = text(placements, "proof_state", "placements")?;
    let line_item_state = text(line_items, "proof_state", "line items")?;
    let response_truncated = optional_bool(line_items, "response_truncated", "line items")?;
    let missing_total = optional_bool(line_items, "missing_total_result_set_size", "line items")?;
    let total = optional_count(line_items, "total_result_set_size", "line items")?;
    let inspected = optional_count(line_items, "inspected_results", "line items")?;
    let progress_incomplete =
        matches!((total, inspected), (Some(total), Some(inspected)) if total > inspected);
    let expected = BTreeMap::from([
        ("target_resolution_incomplete", !issues.is_empty()),
        (
            "id_only_targets_have_unknown_ancestors",
            id_only_targets_have_unknown_ancestors,
        ),
        (
            "placements_capped_or_shape_unknown",
            matches!(placement_state, "sample_or_shape_incomplete" | "blocked"),
        ),
        (
            "line_items_capped_or_truncated",
            line_item_state == "sample_only"
                || response_truncated
                || missing_total
                || progress_incomplete,
        ),
        (
            "soap_manage_scope_required",
            line_items.contains_key("required_scope"),
        ),
        ("line_items_blocked", line_item_state == "blocked"),
    ]);
    for (key, expected_value) in expected {
        if flag(flags, key, "proof flags")? != expected_value {
            return Err("dependency proof flags contradicted the retained proof surfaces".into());
        }
    }
    Ok(())
}

fn optional_bool(source: &Map<String, Value>, field: &str, name: &str) -> Result<bool, String> {
    match source.get(field) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("{name}.{field} was not boolean")),
    }
}

fn optional_count(
    source: &Map<String, Value>,
    field: &str,
    name: &str,
) -> Result<Option<usize>, String> {
    match source.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| format!("{name}.{field} was not an unsigned count")),
        Some(_) => Err(format!("{name}.{field} was not an unsigned count")),
    }
}

fn validate_dependency_placement_variant(source: &Map<String, Value>) -> Result<(), String> {
    if text(source, "surface", "placements")? != "placements" {
        return Err("placement surface label was invalid".into());
    }
    let proof_state = text(source, "proof_state", "placements")?;
    if proof_state == "blocked" {
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
            "blocked placements",
        )?;
        if !matches!(
            text(source, "block_class", "blocked placements")?,
            "permission" | "upstream"
        ) {
            return Err("placement block class was not producer-defined".into());
        }
        text(source, "error", "blocked placements")?;
        text(source, "hint", "blocked placements")?;
        return Ok(());
    }

    exact_keys(
        source,
        &[
            "surface",
            "proof_state",
            "row_count_in_page",
            "page_size",
            "next_page_token_present",
            "capped_or_possibly_more",
            "membership_shape_unknown_count",
            "membership_shape_unknown_sample",
            "target_placement_match_count",
            "target_placement_matches_truncated",
            "target_placement_id_limit_per_ad_unit",
            "target_placement_ids_truncated",
            "target_placement_ids_by_ad_unit_id",
            "target_placement_matches_sample",
            "mutation_performed",
        ],
        "placement summary",
    )?;
    let row_count = count(source, "row_count_in_page", "placements")?;
    let page_size = count(source, "page_size", "placements")?;
    if !(1..=PLACEMENT_PAGE_SIZE_MAX).contains(&page_size) || row_count > page_size {
        return Err("placement page progress exceeded producer bounds".into());
    }
    let next_page = flag(source, "next_page_token_present", "placements")?;
    let capped = flag(source, "capped_or_possibly_more", "placements")?;
    let expected_capped = next_page || row_count >= page_size;
    if capped != expected_capped {
        return Err("placement cap flag contradicted page progress".into());
    }
    let unknown_membership = count(source, "membership_shape_unknown_count", "placements")?;
    let placement_matches = count(source, "target_placement_match_count", "placements")?;
    if unknown_membership > row_count || placement_matches > row_count {
        return Err("placement evidence counts exceeded the source page".into());
    }
    if count(
        source,
        "target_placement_id_limit_per_ad_unit",
        "placements",
    )? != TARGET_PLACEMENT_ID_LIMIT
    {
        return Err("placement target-id limit changed from the producer contract".into());
    }
    let target_ids_truncated = flag(source, "target_placement_ids_truncated", "placements")?;
    let expected_state = if capped || unknown_membership > 0 || target_ids_truncated {
        "sample_or_shape_incomplete"
    } else {
        "complete_for_page"
    };
    if proof_state != expected_state {
        return Err("placement proof state contradicted producer inputs".into());
    }
    Ok(())
}

fn dependency_placements(
    full: &Value,
    path: &str,
    target_ids: &BTreeSet<String>,
    ledger: &mut Ledger,
) -> Result<Value, String> {
    let source = object(full, "placements")?;
    validate_dependency_placement_variant(source)?;
    let mut result = select(
        source,
        path,
        &[
            "surface",
            "proof_state",
            "row_count_in_page",
            "page_size",
            "next_page_token_present",
            "capped_or_possibly_more",
            "membership_shape_unknown_count",
            "target_placement_match_count",
            "target_placement_matches_truncated",
            "target_placement_id_limit_per_ad_unit",
            "target_placement_ids_truncated",
            "mutation_performed",
            "block_class",
            "error_truncated",
            "hint_truncated",
        ],
        &[
            ("membership_shape_unknown_sample", Class::Array),
            ("target_placement_ids_by_ad_unit_id", Class::Map),
            ("target_placement_matches_sample", Class::Array),
            ("error", Class::Reason),
            ("hint", Class::Reason),
        ],
        ledger,
    )?;
    text(source, "proof_state", "placements")?;
    false_if_present(source, "mutation_performed", "placements")?;
    validate_truncation_pairs(
        source,
        &[("error", "error_truncated"), ("hint", "hint_truncated")],
        "placements",
    )?;
    if source.get("target_placement_matches_sample").is_some()
        != source.get("target_placement_ids_by_ad_unit_id").is_some()
    {
        return Err("placement match sample and target map shape were incomplete".into());
    }
    let mut sample_references = None;
    if let Some(sample) = source.get("target_placement_matches_sample") {
        let sample = sample
            .as_array()
            .ok_or("placement sample was not an array")?;
        let matches = count(source, "target_placement_match_count", "placements")?;
        if sample.len() != matches.min(50)
            || flag(source, "target_placement_matches_truncated", "placements")?
                != (matches > sample.len())
            || array(source, "membership_shape_unknown_sample", "placements")?.len()
                != count(source, "membership_shape_unknown_count", "placements")?.min(10)
        {
            return Err("placement counts or truncation ledger were inconsistent".into());
        }
        let mut references = BTreeSet::new();
        for row in sample {
            let row = object(row, "placement sample row")?;
            let placement_id = text(row, "placement_id", "placement sample row")?;
            if !canonical_id(placement_id) {
                return Err("placement sample contained a noncanonical placement id".into());
            }
            let matched = array(row, "matched_ad_unit_ids", "placement sample row")?;
            if matched.is_empty() {
                return Err("placement sample escaped the probe target scope".into());
            }
            let mut row_target_ids = BTreeSet::new();
            for target in matched {
                let target = target
                    .as_str()
                    .filter(|target| canonical_id(target) && target_ids.contains(*target))
                    .ok_or("placement sample escaped the probe target scope")?;
                if !row_target_ids.insert(target) {
                    return Err("placement sample repeated a matched target id".into());
                }
                references.insert((target.to_string(), placement_id.to_string()));
            }
        }
        sample_references = Some(references);
        result.insert("target_placement_sample_count".into(), json!(sample.len()));
        result.insert(
            "sample_status_counts".into(),
            json!(status_counts(sample, "status")?),
        );
    }
    if let Some(targets) = source.get("target_placement_ids_by_ad_unit_id") {
        let targets = targets
            .as_object()
            .ok_or("placement target map was not an object")?;
        let limit = count(
            source,
            "target_placement_id_limit_per_ad_unit",
            "placements",
        )?;
        let mut counts = BTreeMap::new();
        let mut map_target_ids = BTreeSet::new();
        let mut references = 0_usize;
        let mut map_references = BTreeSet::new();
        let mut referenced_placement_ids = BTreeSet::new();
        let mut target_list_reached_limit = false;
        for (target, ids) in targets {
            if !canonical_id(target) || !target_ids.contains(target) {
                return Err("placement target map escaped the probe target scope".into());
            }
            map_target_ids.insert(target.clone());
            let ids = ids
                .as_array()
                .filter(|ids| ids.len() <= limit)
                .ok_or("placement target ids were not bounded arrays")?;
            let mut target_placement_ids = BTreeSet::new();
            for placement_id in ids {
                let placement_id = placement_id
                    .as_str()
                    .filter(|placement_id| canonical_id(placement_id))
                    .ok_or("placement target map contained a noncanonical placement id")?;
                if !target_placement_ids.insert(placement_id) {
                    return Err("placement target map repeated a placement id".into());
                }
                referenced_placement_ids.insert(placement_id.to_string());
                map_references.insert((target.clone(), placement_id.to_string()));
            }
            target_list_reached_limit |= ids.len() == limit;
            references = references
                .checked_add(ids.len())
                .ok_or("placement target reference count overflowed")?;
            counts.insert(target.clone(), ids.len());
        }
        if map_target_ids != *target_ids {
            return Err("placement target map did not cover the exact probe target scope".into());
        }
        let matches = count(source, "target_placement_match_count", "placements")?;
        if referenced_placement_ids.len() > matches
            || (matches > 0 && referenced_placement_ids.is_empty())
        {
            return Err("placement target references contradicted the match count".into());
        }
        if let Some(sample_references) = &sample_references {
            if !sample_references.is_subset(&map_references) {
                return Err("placement target map contradicted the retained match sample".into());
            }
            let sample_truncated =
                flag(source, "target_placement_matches_truncated", "placements")?;
            let map_truncated = flag(source, "target_placement_ids_truncated", "placements")?;
            if map_truncated && !target_list_reached_limit {
                return Err("placement target-id truncation lacked a full producer list".into());
            }
            if !sample_truncated && !map_truncated && sample_references != &map_references {
                return Err("placement target map contradicted the complete match sample".into());
            }
        }
        result.insert("target_placement_target_count".into(), json!(targets.len()));
        result.insert(
            "target_placement_id_reference_count".into(),
            json!(references),
        );
        result.insert(
            "target_placement_id_counts_by_ad_unit_id".into(),
            json!(counts),
        );
    }
    Ok(Value::Object(result))
}

const LINE_ITEM_SCAN_FIELDS: &[&str] = &[
    "surface",
    "decision",
    "proof_state",
    "total_result_set_size",
    "inspected_results",
    "max_line_items",
    "line_item_page_size",
    "response_truncated",
    "missing_total_result_set_size",
    "request_ids",
    "request_id_count",
    "request_ids_truncated",
    "response_times",
    "response_time_count",
    "response_times_truncated",
    "transport_metadata_sample_limit",
    "status_counts",
    "dependency_match_count",
    "dependency_matches_sample",
    "dependency_matches_truncated",
    "dependency_match_sample_limit",
    "mutation_performed",
];

fn exact_line_item_scan_fields(
    source: &Map<String, Value>,
    diagnostics: &[&str],
    name: &str,
) -> Result<(), String> {
    if source.len() != LINE_ITEM_SCAN_FIELDS.len() + diagnostics.len()
        || source.keys().any(|key| {
            !LINE_ITEM_SCAN_FIELDS.contains(&key.as_str()) && !diagnostics.contains(&key.as_str())
        })
    {
        return Err(format!("{name} fields did not match a producer variant"));
    }
    Ok(())
}

fn optional_text_present(
    source: &Map<String, Value>,
    field: &str,
    truncated_field: &str,
    name: &str,
) -> Result<bool, String> {
    let truncated = flag(source, truncated_field, name)?;
    match get(source, field, name)? {
        Value::Null if !truncated => Ok(false),
        Value::String(_) => Ok(true),
        Value::Null => Err(format!("{name}.{field} was absent but marked truncated")),
        _ => Err(format!("{name}.{field} was not nullable text")),
    }
}

fn validate_blocked_line_item_variant(source: &Map<String, Value>) -> Result<(), String> {
    let block_class = text(source, "block_class", "blocked line items")?;
    if !matches!(block_class, "permission" | "upstream") {
        return Err("line-item block class was not producer-defined".into());
    }

    if source.contains_key("inspected_results") {
        if source.contains_key("upstream_status") {
            exact_line_item_scan_fields(
                source,
                &[
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
                ],
                "SOAP-blocked line items",
            )?;
            let upstream_status = count(source, "upstream_status", "SOAP-blocked line items")?;
            if upstream_status > u16::MAX as usize {
                return Err("SOAP-blocked line-item status exceeded u16".into());
            }
            optional_text_present(
                source,
                "request_id",
                "request_id_truncated",
                "SOAP-blocked line items",
            )?;
            optional_text_present(
                source,
                "response_time",
                "response_time_truncated",
                "SOAP-blocked line items",
            )?;
            let soap_fault_present = optional_text_present(
                source,
                "soap_fault",
                "soap_fault_truncated",
                "SOAP-blocked line items",
            )?;
            let soap_fault = source.get("soap_fault").and_then(Value::as_str);
            let message = text(source, "message", "SOAP-blocked line items")?;
            flag(source, "message_truncated", "SOAP-blocked line items")?;
            if upstream_status < 400 && !soap_fault_present {
                return Err("SOAP-blocked line items had no blocking status or fault".into());
            }
            if (matches!(upstream_status, 401 | 403)
                || retained_soap_permission_evidence(soap_fault, message))
                && block_class != "permission"
            {
                return Err("SOAP permission status had a contradictory block class".into());
            }
        } else {
            exact_line_item_scan_fields(
                source,
                &[
                    "block_class",
                    "error",
                    "error_truncated",
                    "hint",
                    "hint_truncated",
                ],
                "error-blocked line items",
            )?;
            text(source, "error", "error-blocked line items")?;
            text(source, "hint", "error-blocked line items")?;
        }
        return Ok(());
    }

    if source.contains_key("required_scope") {
        exact_keys(
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
            "permission-blocked line items",
        )?;
        if block_class != "permission"
            || source.get("decision").and_then(Value::as_str) != Some("blocked")
        {
            return Err("permission-blocked line items had contradictory semantics".into());
        }
        text(source, "reason", "permission-blocked line items")?;
        text(source, "required_scope", "permission-blocked line items")?;
        text(source, "current_scope", "permission-blocked line items")?;
        flag(
            source,
            "current_scope_truncated",
            "permission-blocked line items",
        )?;
        return Ok(());
    }

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
        "generic-blocked line items",
    )?;
    text(source, "error", "generic-blocked line items")?;
    text(source, "hint", "generic-blocked line items")?;
    Ok(())
}

fn validate_line_item_outcome(source: &Map<String, Value>) -> Result<(), String> {
    if text(source, "surface", "line items")? != "line_items" {
        return Err("line-item surface label was invalid".into());
    }
    let proof_state = text(source, "proof_state", "line items")?;
    match proof_state {
        "skipped" => {
            exact_keys(
                source,
                &[
                    "surface",
                    "decision",
                    "proof_state",
                    "reason",
                    "mutation_performed",
                ],
                "skipped line items",
            )?;
            if source.get("decision").and_then(Value::as_str) != Some("skipped") {
                return Err("skipped line-item proof had a contradictory decision".into());
            }
            text(source, "reason", "skipped line items")?;
            return Ok(());
        }
        "blocked" => {
            validate_blocked_line_item_variant(source)?;
            if source.contains_key("inspected_results") {
                validate_line_item_scan_controls(source)?;
            }
            let matches = optional_count(source, "dependency_match_count", "line items")?;
            let expected = matches.map(|matches| {
                if matches > 0 {
                    "dependencies_found"
                } else {
                    "blocked"
                }
            });
            match (source.get("decision").and_then(Value::as_str), expected) {
                (Some(decision), Some(expected)) if decision == expected => {}
                (Some("blocked"), None) | (None, None) => {}
                _ => {
                    return Err("blocked line-item proof lost observed dependency semantics".into());
                }
            }
            return Ok(());
        }
        "complete" | "sample_only" => {}
        _ => return Err("line-item proof state was not producer-defined".into()),
    }

    exact_line_item_scan_fields(source, &[], "completed line items")?;
    validate_line_item_scan_controls(source)?;

    if !source.contains_key("total_result_set_size") {
        return Err("completed line-item proof omitted total progress".into());
    }
    let total = optional_count(source, "total_result_set_size", "line items")?;
    let inspected = count(source, "inspected_results", "line items")?;
    let response_truncated = flag(source, "response_truncated", "line items")?;
    let missing_total = flag(source, "missing_total_result_set_size", "line items")?;
    let matches = count(source, "dependency_match_count", "line items")?;
    if total.is_none() && !missing_total {
        return Err("completed line-item proof had impossible missing-total state".into());
    }
    if matches > inspected {
        return Err("completed line-item proof had impossible scan progress".into());
    }

    let capped =
        response_truncated || missing_total || total.is_some_and(|total| total != inspected);
    let expected_state = if capped { "sample_only" } else { "complete" };
    let expected_decision = if matches > 0 {
        "dependencies_found"
    } else if capped {
        "no_dependencies_in_sample"
    } else {
        "no_dependencies_observed"
    };
    if proof_state != expected_state
        || source.get("decision").and_then(Value::as_str) != Some(expected_decision)
    {
        return Err("completed line-item outcome contradicted scan progress".into());
    }
    Ok(())
}

fn validate_line_item_scan_controls(source: &Map<String, Value>) -> Result<(), String> {
    let max_line_items = count(source, "max_line_items", "line items")?;
    let page_size = count(source, "line_item_page_size", "line items")?;
    let inspected = count(source, "inspected_results", "line items")?;
    if !(1..=LINE_ITEM_SCAN_MAX).contains(&max_line_items)
        || !(1..=LINE_ITEM_PAGE_SIZE_MAX).contains(&page_size)
        || inspected > max_line_items
    {
        return Err("line-item scan controls exceeded producer bounds".into());
    }
    if count(source, "transport_metadata_sample_limit", "line items")?
        != TRANSPORT_METADATA_SAMPLE_LIMIT
        || count(source, "dependency_match_sample_limit", "line items")?
            != DEPENDENCY_MATCH_SAMPLE_LIMIT
    {
        return Err("line-item sample limits changed from the producer contract".into());
    }
    Ok(())
}

fn dependency_coverage<'a>(
    value: &'a Value,
    name: &str,
) -> Result<Option<DependencyCoverage<'a>>, String> {
    let Some(value) = value.as_object() else {
        if value.is_null() {
            return Ok(None);
        }
        return Err(format!("{name} was not an object or null"));
    };
    exact_keys(
        value,
        &["ad_unit_id", "include_descendants", "match_type"],
        name,
    )?;
    let ad_unit_id = text(value, "ad_unit_id", name)?;
    if !canonical_id(ad_unit_id) {
        return Err(format!("{name} contained a noncanonical ad-unit id"));
    }
    match value.get("include_descendants") {
        Some(Value::Bool(_)) | Some(Value::Null) => {}
        _ => {
            return Err(format!(
                "{name}.include_descendants was not boolean or null"
            ));
        }
    }
    let match_type = text(value, "match_type", name)?;
    if !matches!(match_type, "exact" | "ancestor_descendant") {
        return Err(format!("{name} contained an unknown match type"));
    }
    Ok(Some(DependencyCoverage {
        ad_unit_id,
        include_descendants: value.get("include_descendants").and_then(Value::as_bool),
        match_type,
    }))
}

fn validate_dependency_target_matches(
    values: &[Value],
    target_ids: &BTreeSet<String>,
    target_contexts: &BTreeMap<String, DependencyTargetContext>,
) -> Result<BTreeMap<String, usize>, String> {
    if values.is_empty() {
        return Err("dependency match contained no target evidence".into());
    }
    let mut seen = BTreeSet::new();
    let mut classes = BTreeMap::<String, usize>::new();
    for target in values {
        let target = object(target, "dependency target")?;
        exact_keys(
            target,
            &[
                "ad_unit_id",
                "ad_unit_codes",
                "classification",
                "targeting_match",
                "exclusion_match",
                "matched_placement_ids",
                "root_or_network_targeting",
                "dependency_excluded",
            ],
            "dependency target",
        )?;
        let ad_unit_id = text(target, "ad_unit_id", "dependency target")?;
        require_target_member(
            get(target, "ad_unit_id", "dependency target")?,
            target_ids,
            "line-item dependency target",
        )?;
        if !seen.insert(ad_unit_id.to_string()) {
            return Err("dependency match repeated a target ad-unit id".into());
        }
        let context = target_contexts
            .get(ad_unit_id)
            .ok_or("dependency target context was unavailable")?;
        let matched_placements = canonical_id_set(
            array(target, "matched_placement_ids", "dependency target")?,
            "dependency matched placements",
        )?;
        if !matched_placements.is_subset(&context.placement_ids) {
            return Err("dependency placement evidence escaped the target placement map".into());
        }
        let placement_match = !matched_placements.is_empty();
        let mut codes = BTreeSet::new();
        for code in array(target, "ad_unit_codes", "dependency target")? {
            let code = code
                .as_str()
                .filter(|code| !code.is_empty())
                .ok_or("dependency target code was not nonempty text")?;
            if !codes.insert(code) {
                return Err("dependency target repeated an ad-unit code".into());
            }
        }
        let targeting = dependency_coverage(
            get(target, "targeting_match", "dependency target")?,
            "dependency targeting match",
        )?;
        let exclusion = dependency_coverage(
            get(target, "exclusion_match", "dependency target")?,
            "dependency exclusion match",
        )?;
        for (coverage, name) in [
            (targeting.as_ref(), "dependency targeting match"),
            (exclusion.as_ref(), "dependency exclusion match"),
        ] {
            let Some(coverage) = coverage else {
                continue;
            };
            let valid = match coverage.match_type {
                "exact" => coverage.ad_unit_id == ad_unit_id,
                "ancestor_descendant" => {
                    coverage.include_descendants == Some(true)
                        && context.ancestor_ad_unit_ids.contains(coverage.ad_unit_id)
                }
                _ => false,
            };
            if !valid {
                return Err(format!("{name} escaped the retained target hierarchy"));
            }
        }
        let root = flag(target, "root_or_network_targeting", "dependency target")?;
        let excluded = flag(target, "dependency_excluded", "dependency target")?;
        if excluded != exclusion.is_some() || (targeting.is_none() && !placement_match && !root) {
            return Err("dependency target evidence contradicted its producer shape".into());
        }
        let expected = if exclusion.is_some() {
            if targeting.is_some() {
                "targeted_but_excluded"
            } else if placement_match {
                "placement_targeted_but_excluded"
            } else {
                "root_or_network_targeted_but_excluded"
            }
        } else if let Some(coverage) = targeting {
            if coverage.match_type == "exact" {
                "exact_target"
            } else {
                "ancestor_descendant_target"
            }
        } else if placement_match {
            "placement_target"
        } else {
            "root_or_network_target"
        };
        let classification = text(target, "classification", "dependency target")?;
        if classification != expected {
            return Err("dependency target classification contradicted retained evidence".into());
        }
        *classes.entry(classification.to_string()).or_default() += 1;
    }
    Ok(classes)
}

fn dependency_line_items(
    full: &Value,
    path: &str,
    target_ids: &BTreeSet<String>,
    target_contexts: &BTreeMap<String, DependencyTargetContext>,
    ledger: &mut Ledger,
) -> Result<Value, String> {
    let source = object(full, "line items")?;
    let mut result = select(
        source,
        path,
        &[
            "surface",
            "decision",
            "proof_state",
            "total_result_set_size",
            "inspected_results",
            "max_line_items",
            "line_item_page_size",
            "response_truncated",
            "missing_total_result_set_size",
            "status_counts",
            "request_id_count",
            "request_ids_truncated",
            "response_time_count",
            "response_times_truncated",
            "transport_metadata_sample_limit",
            "dependency_match_count",
            "dependency_matches_truncated",
            "dependency_match_sample_limit",
            "mutation_performed",
            "block_class",
            "upstream_status",
            "error_truncated",
            "hint_truncated",
            "request_id_truncated",
            "response_time_truncated",
            "soap_fault_truncated",
            "message_truncated",
            "current_scope_truncated",
        ],
        &[
            ("request_ids", Class::Transport),
            ("response_times", Class::Transport),
            ("dependency_matches_sample", Class::Array),
            ("error", Class::Reason),
            ("hint", Class::Reason),
            ("request_id", Class::Transport),
            ("response_time", Class::Transport),
            ("soap_fault", Class::Reason),
            ("message", Class::Reason),
            ("reason", Class::Reason),
            ("required_scope", Class::Reason),
            ("current_scope", Class::Reason),
        ],
        ledger,
    )?;
    validate_line_item_outcome(source)?;
    false_if_present(source, "mutation_performed", "line items")?;
    validate_truncation_pairs(
        source,
        &[
            ("error", "error_truncated"),
            ("hint", "hint_truncated"),
            ("request_id", "request_id_truncated"),
            ("response_time", "response_time_truncated"),
            ("soap_fault", "soap_fault_truncated"),
            ("message", "message_truncated"),
            ("current_scope", "current_scope_truncated"),
        ],
        "line items",
    )?;
    let transport_fields = [
        "transport_metadata_sample_limit",
        "request_ids",
        "request_id_count",
        "request_ids_truncated",
        "response_times",
        "response_time_count",
        "response_times_truncated",
    ];
    if source.get("transport_metadata_sample_limit").is_some() {
        if transport_fields
            .iter()
            .any(|field| source.get(*field).is_none())
        {
            return Err("line-item transport metadata shape was incomplete".into());
        }
        let metadata_limit = count(source, "transport_metadata_sample_limit", "line items")?;
        if metadata_limit != TRANSPORT_METADATA_SAMPLE_LIMIT {
            return Err("line-item transport metadata limit changed".into());
        }
        for (sample_field, count_field, truncated_field) in [
            ("request_ids", "request_id_count", "request_ids_truncated"),
            (
                "response_times",
                "response_time_count",
                "response_times_truncated",
            ),
        ] {
            let sample = array(source, sample_field, "line items")?;
            let observed = count(source, count_field, "line items")?;
            let truncated = flag(source, truncated_field, "line items")?;
            if sample.len() != observed.min(metadata_limit)
                || (!truncated && observed != sample.len())
            {
                return Err(
                    "line-item transport metadata counts or cap flags were inconsistent".into(),
                );
            }
        }
    } else if transport_fields
        .iter()
        .any(|field| source.get(*field).is_some())
    {
        return Err("line-item transport metadata shape was incomplete".into());
    }
    if let Some(sample) = source.get("dependency_matches_sample") {
        let sample = sample
            .as_array()
            .ok_or("dependency sample was not an array")?;
        let matches = count(source, "dependency_match_count", "line items")?;
        let sample_limit = count(source, "dependency_match_sample_limit", "line items")?;
        if sample_limit != DEPENDENCY_MATCH_SAMPLE_LIMIT
            || sample.len() != matches.min(sample_limit)
            || flag(source, "dependency_matches_truncated", "line items")?
                != (matches > sample.len())
        {
            return Err("line-item dependency counts or truncation were inconsistent".into());
        }
        let statuses = object(get(source, "status_counts", "line items")?, "status counts")?;
        let status_total = statuses.values().try_fold(0_u64, |total, value| {
            value.as_u64().and_then(|value| total.checked_add(value))
        });
        if status_total != Some(count(source, "inspected_results", "line items")? as u64) {
            return Err("line-item status counts disagreed with inspected progress".into());
        }
        if text(source, "proof_state", "line items")? == "blocked"
            && source.get("decision").and_then(Value::as_str)
                != Some(if matches > 0 {
                    "dependencies_found"
                } else {
                    "blocked"
                })
        {
            return Err("late line-item block lost observed dependency semantics".into());
        }
        let mut classes = BTreeMap::<String, usize>::new();
        let mut excluded = BTreeMap::from([("false", 0_usize), ("true", 0_usize)]);
        let mut raw_samples = Vec::new();
        let mut raw_source_bytes = 0_usize;
        let mut raw_retained_bytes = 0_usize;
        let mut raw_truncated_count = 0_usize;
        for row in sample {
            let row = object(row, "dependency match")?;
            let target_matches = array(row, "target_matches", "dependency match")?;
            for (classification, count) in
                validate_dependency_target_matches(target_matches, target_ids, target_contexts)?
            {
                *classes.entry(classification).or_default() += count;
            }
            for target in target_matches {
                let target = object(target, "dependency target")?;
                let value = flag(target, "dependency_excluded", "dependency target")?;
                *excluded
                    .get_mut(if value { "true" } else { "false" })
                    .expect("bool key") += 1;
            }
            match (
                row.get("upstream_xml_sample"),
                row.get("upstream_xml_truncated"),
                row.get("upstream_xml_bytes"),
            ) {
                (None, None, None) => {}
                (
                    Some(Value::String(xml)),
                    Some(Value::Bool(truncated)),
                    Some(Value::Number(source_bytes)),
                ) => {
                    let source_bytes = source_bytes
                        .as_u64()
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or("dependency XML source bytes were invalid")?;
                    if source_bytes < xml.len() || *truncated != (source_bytes > xml.len()) {
                        return Err("dependency XML truncation metadata was inconsistent".into());
                    }
                    raw_source_bytes = raw_source_bytes
                        .checked_add(source_bytes)
                        .ok_or("dependency XML source byte count overflowed")?;
                    raw_retained_bytes = raw_retained_bytes
                        .checked_add(xml.len())
                        .ok_or("dependency XML retained byte count overflowed")?;
                    if *truncated {
                        raw_truncated_count += 1;
                    }
                    raw_samples.push(json!(xml));
                }
                _ => return Err("dependency XML sample metadata was incomplete".into()),
            }
        }
        result.insert("dependency_sample_count".into(), json!(sample.len()));
        result.insert(
            "sample_status_counts".into(),
            json!(status_counts(sample, "status")?),
        );
        result.insert(
            "sample_activity_state_counts".into(),
            json!(status_counts(sample, "activity_state")?),
        );
        result.insert("sample_target_classification_counts".into(), json!(classes));
        result.insert("sample_dependency_excluded_counts".into(), json!(excluded));
        if !raw_samples.is_empty() {
            result.insert(
                "upstream_xml_sample_summary".into(),
                json!({
                    "sample_count": raw_samples.len(),
                    "source_bytes": raw_source_bytes,
                    "retained_bytes": raw_retained_bytes,
                    "truncated_count": raw_truncated_count,
                }),
            );
            ledger.aggregate(
                &format!("{path}/dependency_matches_sample/*/upstream_xml_sample"),
                &Value::Array(raw_samples),
                Class::RawSoap,
                "utf8_bytes",
                raw_retained_bytes,
            );
        }
    }
    Ok(Value::Object(result))
}
