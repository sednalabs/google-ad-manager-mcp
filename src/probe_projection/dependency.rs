use super::*;

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
    text(root, "network_code", "dependency probe")?;
    text(root, "dependency_decision", "dependency probe")?;
    false_field(root, "mutation_performed", "dependency probe")?;
    object(get(root, "proof_flags", "dependency probe")?, "proof flags")?;
    let cleanup = object(
        get(root, "cleanup_decision", "dependency probe")?,
        "cleanup",
    )?;
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
    let placement_source = get(root, "placements", "dependency probe")?;
    let line_item_source = get(root, "line_items", "dependency probe")?;
    let expected_decision =
        dependency_probe_decision(&issue_strings, placement_source, line_item_source);
    if text(root, "dependency_decision", "dependency probe")? != expected_decision {
        return Err("dependency decision contradicted the retained proof surfaces".into());
    }
    let target_identity = target_identity_summary(ad_units)?;
    let target_ids = canonical_target_ids(ad_units)?;
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
    let line_items =
        dependency_line_items(line_item_source, "/line_items", &target_ids, &mut ledger)?;
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

fn dependency_placements(
    full: &Value,
    path: &str,
    target_ids: &BTreeSet<String>,
    ledger: &mut Ledger,
) -> Result<Value, String> {
    let source = object(full, "placements")?;
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
        for row in sample {
            let row = object(row, "placement sample row")?;
            let matched = array(row, "matched_ad_unit_ids", "placement sample row")?;
            if matched.is_empty()
                || matched.iter().any(|id| {
                    id.as_str()
                        .is_none_or(|id| !canonical_id(id) || !target_ids.contains(id))
                })
            {
                return Err("placement sample escaped the probe target scope".into());
            }
        }
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
        for (target, ids) in targets {
            if !canonical_id(target) || !target_ids.contains(target) {
                return Err("placement target map escaped the probe target scope".into());
            }
            map_target_ids.insert(target.clone());
            let ids = ids
                .as_array()
                .filter(|ids| ids.len() <= limit)
                .ok_or("placement target ids were not bounded arrays")?;
            if ids
                .iter()
                .any(|id| id.as_str().is_none_or(|id| !canonical_id(id)))
            {
                return Err("placement target map contained a noncanonical placement id".into());
            }
            references = references
                .checked_add(ids.len())
                .ok_or("placement target reference count overflowed")?;
            counts.insert(target.clone(), ids.len());
        }
        if map_target_ids != *target_ids {
            return Err("placement target map did not cover the exact probe target scope".into());
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

fn dependency_line_items(
    full: &Value,
    path: &str,
    target_ids: &BTreeSet<String>,
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
    text(source, "proof_state", "line items")?;
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
        if metadata_limit == 0 {
            return Err("line-item transport metadata limit was zero".into());
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
        if sample.len() != matches.min(sample_limit)
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
        let mut raw = Vec::new();
        let mut raw_source_bytes = 0_usize;
        let mut raw_retained_bytes = 0_usize;
        let mut raw_truncated_count = 0_usize;
        for row in sample {
            let row = object(row, "dependency match")?;
            for target in array(row, "target_matches", "dependency match")? {
                let target = object(target, "dependency target")?;
                require_target_member(
                    get(target, "ad_unit_id", "dependency target")?,
                    target_ids,
                    "line-item dependency target",
                )?;
                *classes
                    .entry(text(target, "classification", "dependency target")?.into())
                    .or_default() += 1;
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
                    raw.push(json!({
                        "sample": xml,
                        "source_bytes": source_bytes,
                        "truncated": truncated,
                    }));
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
        if !raw.is_empty() {
            result.insert(
                "upstream_xml_sample_summary".into(),
                json!({
                    "sample_count": raw.len(),
                    "source_bytes": raw_source_bytes,
                    "retained_bytes": raw_retained_bytes,
                    "truncated_count": raw_truncated_count,
                }),
            );
            ledger.aggregate(
                &format!("{path}/dependency_matches_sample/*/upstream_xml_sample"),
                &Value::Array(raw),
                Class::RawSoap,
                "utf8_bytes",
                raw_source_bytes,
            );
        }
    }
    Ok(Value::Object(result))
}
