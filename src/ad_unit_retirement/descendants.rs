use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::fingerprint::stable_fingerprint;
use crate::{AdManagerError, AdManagerServer, CatalogCollection};

use super::inventory::{bounded_string, numeric_id};

const DESCENDANT_SAMPLE_LIMIT: usize = 10;
const MAX_DESCENDANT_SCAN_PAGES: u32 = 100;

pub(super) struct DescendantScan {
    network_code: String,
    target_ids: BTreeSet<String>,
    expected_has_children: BTreeMap<String, bool>,
    listed_has_children: BTreeMap<String, bool>,
    listed_target_ids: BTreeSet<String>,
    parent_by_id: BTreeMap<String, Option<String>>,
    reported_ancestors_by_id: BTreeMap<String, BTreeSet<String>>,
    duplicate_catalog_id: bool,
    max_rows: u32,
    page_count: u32,
    rows_scanned: u64,
    capped: bool,
    zero_progress: bool,
    repeated_page_token: bool,
    hierarchy_shape_observed: bool,
    invalid_network_resource: bool,
    seen_page_tokens: BTreeSet<String>,
    target_has_children: BTreeSet<String>,
    target_ids_with_descendants: BTreeSet<String>,
    target_depths: BTreeMap<String, usize>,
    external_descendants: Vec<Value>,
    external_samples: Vec<Value>,
    intra_target_relationships: Vec<Value>,
    status_counts: BTreeMap<String, u64>,
    fingerprint_rows: Vec<Value>,
}

impl DescendantScan {
    pub(super) fn new(
        network_code: &str,
        target_ids: &[String],
        expected_has_children: &BTreeMap<String, bool>,
        max_rows: u32,
    ) -> Self {
        Self {
            network_code: network_code.to_string(),
            target_ids: target_ids.iter().cloned().collect(),
            expected_has_children: expected_has_children.clone(),
            listed_has_children: BTreeMap::new(),
            listed_target_ids: BTreeSet::new(),
            parent_by_id: BTreeMap::new(),
            reported_ancestors_by_id: BTreeMap::new(),
            duplicate_catalog_id: false,
            max_rows,
            page_count: 0,
            rows_scanned: 0,
            capped: false,
            zero_progress: false,
            repeated_page_token: false,
            hierarchy_shape_observed: false,
            invalid_network_resource: false,
            seen_page_tokens: BTreeSet::new(),
            target_has_children: BTreeSet::new(),
            target_ids_with_descendants: BTreeSet::new(),
            target_depths: BTreeMap::new(),
            external_descendants: Vec::new(),
            external_samples: Vec::new(),
            intra_target_relationships: Vec::new(),
            status_counts: BTreeMap::new(),
            fingerprint_rows: Vec::new(),
        }
    }

    pub(super) fn consume_page(&mut self, payload: &Value) -> Option<String> {
        self.page_count += 1;
        let rows = ad_unit_rows(payload);
        let remaining = u64::from(self.max_rows).saturating_sub(self.rows_scanned) as usize;
        if rows.len() > remaining {
            self.capped = true;
        }
        let process_count = rows.len().min(remaining);
        for row in rows.iter().take(process_count) {
            self.consume_row(row);
        }
        self.rows_scanned += process_count as u64;

        let next_token = payload
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if self.capped || next_token.is_none() {
            return None;
        }
        if process_count == 0 {
            self.zero_progress = true;
            self.capped = true;
            return None;
        }
        if self.page_count >= MAX_DESCENDANT_SCAN_PAGES {
            self.capped = true;
            return None;
        }
        let next_token = next_token.expect("checked above");
        if !self.seen_page_tokens.insert(next_token.clone()) {
            self.repeated_page_token = true;
            self.capped = true;
            return None;
        }
        Some(next_token)
    }

    fn consume_row(&mut self, row: &Value) {
        let name = row.get("name").and_then(Value::as_str).unwrap_or_default();
        let ad_unit_id = scoped_numeric_id(name, &self.network_code);
        if !name.is_empty() && ad_unit_id.is_none() {
            self.invalid_network_resource = true;
        }
        let parent_id = row
            .get("parentAdUnit")
            .and_then(Value::as_str)
            .and_then(|value| scoped_numeric_id(value, &self.network_code));
        if row.get("parentAdUnit").is_some() && parent_id.is_none() {
            self.invalid_network_resource = true;
        }
        let reported_ancestors = ancestor_ids(row, &self.network_code)
            .into_iter()
            .collect::<BTreeSet<_>>();
        if row.get("parentAdUnit").is_some() || row.get("parentPath").is_some() {
            self.hierarchy_shape_observed = true;
        }
        if let Some(id) = &ad_unit_id
            && self.target_ids.contains(id)
        {
            self.listed_target_ids.insert(id.clone());
            if let Some(has_children) = row.get("hasChildren").and_then(Value::as_bool) {
                self.listed_has_children.insert(id.clone(), has_children);
            }
            if row.get("hasChildren").and_then(Value::as_bool) == Some(true) {
                self.target_has_children.insert(id.clone());
            }
        }
        if let Some(id) = &ad_unit_id
            && (self
                .parent_by_id
                .insert(id.clone(), parent_id.clone())
                .is_some()
                || self
                    .reported_ancestors_by_id
                    .insert(id.clone(), reported_ancestors.clone())
                    .is_some())
        {
            self.duplicate_catalog_id = true;
        }
        self.fingerprint_rows.push(json!({
            "id": ad_unit_id,
            "status": bounded_string(row.get("status"), 32),
            "parent": parent_id,
            "ancestors": reported_ancestors,
            "has_children": row.get("hasChildren").and_then(Value::as_bool),
            "updated": bounded_string(row.get("updateTime"), 64),
        }));

        let ancestors = reported_ancestors;
        let matched_targets = ancestors
            .intersection(&self.target_ids)
            .cloned()
            .collect::<Vec<_>>();
        if matched_targets.is_empty() {
            return;
        }
        self.target_ids_with_descendants
            .extend(matched_targets.iter().cloned());

        if let Some(id) = &ad_unit_id
            && self.target_ids.contains(id)
        {
            self.target_depths.insert(id.clone(), matched_targets.len());
            if self.intra_target_relationships.len() < DESCENDANT_SAMPLE_LIMIT {
                self.intra_target_relationships.push(json!({
                    "target_ad_unit_id": id,
                    "ancestor_target_ad_unit_ids": matched_targets,
                }));
            }
            return;
        }

        let status = row
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
            .to_string();
        *self.status_counts.entry(status.clone()).or_default() += 1;
        let summary = json!({
            "ad_unit_id": ad_unit_id,
            "ad_unit_code": bounded_string(row.get("adUnitCode"), 128),
            "status": status,
            "matched_target_ad_unit_ids": matched_targets,
        });
        if self.external_samples.len() < DESCENDANT_SAMPLE_LIMIT {
            self.external_samples.push(summary.clone());
        }
        self.external_descendants.push(summary);
    }

    pub(super) fn finish(self, page_size: u32) -> Value {
        let blocking_count = self
            .external_descendants
            .iter()
            .filter(|value| value.get("status").and_then(Value::as_str) != Some("ARCHIVED"))
            .count();
        let child_flag_mismatch = self
            .expected_has_children
            .iter()
            .any(|(id, expected)| self.listed_has_children.get(id).copied() != Some(*expected));
        let missing_target_list_row = self.listed_target_ids != self.target_ids;
        let ancestry_mismatch = self.reported_ancestors_by_id.iter().any(|(id, reported)| {
            resolve_catalog_ancestors(id, &self.parent_by_id)
                .is_none_or(|resolved| &resolved != reported)
        });
        let hierarchy_inconsistent = self.invalid_network_resource
            || self.duplicate_catalog_id
            || missing_target_list_row
            || ancestry_mismatch
            || child_flag_mismatch
            || self
                .target_has_children
                .difference(&self.target_ids_with_descendants)
                .next()
                .is_some();
        let state = if self.capped
            || self.zero_progress
            || self.repeated_page_token
            || hierarchy_inconsistent
        {
            "partial_capped"
        } else if !self.hierarchy_shape_observed {
            "unsupported_surface"
        } else if blocking_count > 0 {
            "complete_blocked"
        } else {
            "complete_clear"
        };
        let mut child_first_order = self.target_ids.iter().cloned().collect::<Vec<_>>();
        child_first_order.sort_by(|left, right| {
            self.target_depths
                .get(right)
                .unwrap_or(&0)
                .cmp(self.target_depths.get(left).unwrap_or(&0))
                .then_with(|| left.cmp(right))
        });
        json!({
            "proof_state": state,
            "rows_scanned": self.rows_scanned,
            "page_count": self.page_count,
            "max_pages": MAX_DESCENDANT_SCAN_PAGES,
            "max_ad_units": self.max_rows,
            "page_size": page_size,
            "scan_complete": !self.capped && !self.zero_progress && !self.repeated_page_token,
            "catalog_capped": self.capped,
            "zero_progress_page": self.zero_progress,
            "repeated_page_token": self.repeated_page_token,
            "hierarchy_shape_observed": self.hierarchy_shape_observed,
            "hierarchy_inconsistent": hierarchy_inconsistent,
            "identity_list_child_flag_mismatch": child_flag_mismatch,
            "invalid_network_resource": self.invalid_network_resource,
            "duplicate_catalog_id": self.duplicate_catalog_id,
            "missing_target_list_row": missing_target_list_row,
            "ancestry_mismatch": ancestry_mismatch,
            "external_descendant_count": self.external_descendants.len(),
            "blocking_external_descendant_count": blocking_count,
            "external_descendant_status_counts": self.status_counts,
            "external_descendant_sample": self.external_samples,
            "external_descendant_sample_truncated": self.external_descendants.len() > DESCENDANT_SAMPLE_LIMIT,
            "intra_target_hierarchy": self.intra_target_relationships,
            "requires_child_first_sequence": !self.target_depths.is_empty(),
            "required_child_first_target_order": child_first_order,
            "catalog_fingerprint": stable_fingerprint(&Value::Array(self.fingerprint_rows).to_string()),
            "descendant_result_fingerprint": stable_fingerprint(&Value::Array(self.external_descendants).to_string()),
        })
    }
}

pub(super) async fn scan_descendants(
    server: &AdManagerServer,
    network_code: &str,
    target_ids: &[String],
    identity_child_claims: &BTreeMap<String, bool>,
    page_size: u32,
    max_rows: u32,
) -> Result<Value, AdManagerError> {
    let mut scan = DescendantScan::new(network_code, target_ids, identity_child_claims, max_rows);
    let mut page_token = None;
    loop {
        let payload = server
            .client()
            .list_network_catalog(
                network_code,
                CatalogCollection::AdUnits,
                Some(page_size),
                page_token,
                None,
                Some("name".to_string()),
            )
            .await?;
        let Some(next_token) = scan.consume_page(&payload) else {
            break;
        };
        page_token = Some(next_token);
    }
    Ok(scan.finish(page_size))
}

pub(super) fn blocked_descendants(err: AdManagerError) -> Value {
    let state = match err {
        AdManagerError::UpstreamApi {
            status: 401 | 403, ..
        }
        | AdManagerError::WriteScopeRequired { .. } => "blocked_permission",
        _ => "not_run",
    };
    json!({
        "proof_state": state,
        "scan_complete": false,
        "reason": "the current ad-unit descendant scan did not complete",
    })
}

fn ad_unit_rows(payload: &Value) -> &[Value] {
    payload
        .get("adUnits")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn ancestor_ids(row: &Value, network_code: &str) -> Vec<String> {
    let mut ids = BTreeSet::new();
    collect_ids(row.get("parentAdUnit"), network_code, &mut ids);
    collect_ids(row.get("parentPath"), network_code, &mut ids);
    ids.into_iter().collect()
}

fn collect_ids(value: Option<&Value>, network_code: &str, ids: &mut BTreeSet<String>) {
    match value {
        Some(Value::String(value)) => {
            if let Some(id) = scoped_numeric_id(value, network_code) {
                ids.insert(id);
            }
        }
        Some(Value::Array(values)) => {
            for value in values {
                collect_ids(Some(value), network_code, ids);
            }
        }
        Some(Value::Object(object)) => {
            for key in ["adUnitId", "adUnit", "id", "name", "parentAdUnit"] {
                collect_ids(object.get(key), network_code, ids);
            }
        }
        _ => {}
    }
}

fn scoped_numeric_id(value: &str, network_code: &str) -> Option<String> {
    if value.contains('/') {
        let prefix = format!("networks/{network_code}/adUnits/");
        let raw_id = value.strip_prefix(&prefix)?;
        if raw_id.contains('/') {
            return None;
        }
        return numeric_id(raw_id);
    }
    numeric_id(value)
}

fn resolve_catalog_ancestors(
    ad_unit_id: &str,
    parent_by_id: &BTreeMap<String, Option<String>>,
) -> Option<BTreeSet<String>> {
    let mut resolved = BTreeSet::new();
    let mut current = parent_by_id.get(ad_unit_id)?.clone();
    while let Some(parent_id) = current {
        if !resolved.insert(parent_id.clone()) {
            return None;
        }
        current = parent_by_id.get(&parent_id)?.clone();
    }
    Some(resolved)
}
