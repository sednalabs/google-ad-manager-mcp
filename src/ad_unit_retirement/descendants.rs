use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use serde_json::{Value, json};

use crate::{AdManagerError, fingerprint::stable_fingerprint};

const DESCENDANT_SAMPLE_LIMIT: usize = 8;
const MAX_DESCENDANT_SCAN_PAGES: u32 = 100;
const MAX_DESCENDANT_SCAN_BYTES: usize = 16 * 1024 * 1024;
const MAX_PAGE_TOKEN_BYTES: usize = 2 * 1024;

pub(crate) const MAX_DESCENDANT_PAGE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
struct CatalogRow {
    status: Option<String>,
    parent_id: Option<String>,
    reported_ancestors: Vec<String>,
    has_children: Option<bool>,
    fingerprint: Value,
}

pub(super) struct DescendantScan {
    network_code: String,
    target_ids: BTreeSet<String>,
    expected_has_children: BTreeMap<String, bool>,
    rows_by_id: BTreeMap<String, CatalogRow>,
    listed_target_ids: BTreeSet<String>,
    seen_page_tokens: BTreeSet<String>,
    last_resource_name: Option<String>,
    max_rows: u32,
    page_count: u32,
    rows_scanned: u64,
    response_bytes_scanned: usize,
    issues: BTreeSet<&'static str>,
    provider_request_state: &'static str,
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
            rows_by_id: BTreeMap::new(),
            listed_target_ids: BTreeSet::new(),
            seen_page_tokens: BTreeSet::new(),
            last_resource_name: None,
            max_rows,
            page_count: 0,
            rows_scanned: 0,
            response_bytes_scanned: 0,
            issues: BTreeSet::new(),
            provider_request_state: "completed",
        }
    }

    pub(super) fn consume_page(
        &mut self,
        payload: &Value,
        response_bytes: usize,
    ) -> Option<String> {
        self.page_count += 1;
        self.response_bytes_scanned = self.response_bytes_scanned.saturating_add(response_bytes);
        if response_bytes > MAX_DESCENDANT_PAGE_BYTES {
            self.issues.insert("page_response_bytes_exceeded");
        }
        if self.response_bytes_scanned > MAX_DESCENDANT_SCAN_BYTES {
            self.issues.insert("scan_response_bytes_exceeded");
        }

        let rows = match payload.get("adUnits") {
            Some(Value::Array(rows)) => rows,
            _ => {
                self.issues.insert("ad_units_missing_or_invalid");
                return None;
            }
        };
        let remaining = u64::from(self.max_rows).saturating_sub(self.rows_scanned) as usize;
        if rows.len() > remaining {
            self.issues.insert("row_cap_reached");
        }
        let process_count = rows.len().min(remaining);
        for row in rows.iter().take(process_count) {
            self.consume_row(row);
        }
        self.rows_scanned += process_count as u64;

        let next_token = match payload.get("nextPageToken") {
            None | Some(Value::Null) => None,
            Some(Value::String(token))
                if !token.is_empty() && token.len() <= MAX_PAGE_TOKEN_BYTES =>
            {
                Some(token.clone())
            }
            Some(_) => {
                self.issues.insert("next_page_token_invalid");
                return None;
            }
        };

        if self.issues.contains("row_cap_reached")
            || self.issues.contains("page_response_bytes_exceeded")
            || self.issues.contains("scan_response_bytes_exceeded")
            || next_token.is_none()
        {
            return None;
        }
        if process_count == 0 {
            self.issues.insert("zero_progress_page");
            return None;
        }
        if self.page_count >= MAX_DESCENDANT_SCAN_PAGES {
            self.issues.insert("page_cap_reached");
            return None;
        }
        let next_token = next_token.expect("checked above");
        if !self.seen_page_tokens.insert(next_token.clone()) {
            self.issues.insert("repeated_page_token");
            return None;
        }
        Some(next_token)
    }

    pub(super) fn record_failure(&mut self, err: AdManagerError, request_attempted: bool) {
        self.provider_request_state = if request_attempted {
            "attempted_no_complete_response"
        } else {
            "not_sent"
        };
        match err {
            AdManagerError::AuthBootstrap(_) => {
                self.issues.insert("blocked_auth");
            }
            AdManagerError::UpstreamApi {
                status: 401 | 403, ..
            }
            | AdManagerError::WriteScopeRequired { .. } => {
                self.issues.insert("blocked_permission");
            }
            _ => {
                self.issues.insert("upstream_read_incomplete");
            }
        }
    }

    fn consume_row(&mut self, row: &Value) {
        let Some(row) = row.as_object() else {
            self.issues.insert("catalog_row_invalid");
            return;
        };
        let Some(name) = row.get("name").and_then(Value::as_str) else {
            self.issues.insert("catalog_row_name_missing_or_invalid");
            return;
        };
        let Some(ad_unit_id) = scoped_numeric_id(name, &self.network_code) else {
            self.issues
                .insert("catalog_resource_invalid_or_cross_network");
            return;
        };
        if self
            .last_resource_name
            .as_deref()
            .is_some_and(|previous| previous >= name)
        {
            self.issues.insert("catalog_order_invalid");
        }
        self.last_resource_name = Some(name.to_string());

        let parent_id = match row.get("parentAdUnit") {
            None | Some(Value::Null) => None,
            Some(Value::String(resource_name)) => {
                match scoped_numeric_id(resource_name, &self.network_code) {
                    Some(id) => Some(id),
                    None => {
                        self.issues
                            .insert("parent_resource_invalid_or_cross_network");
                        None
                    }
                }
            }
            Some(_) => {
                self.issues
                    .insert("parent_resource_invalid_or_cross_network");
                None
            }
        };
        let reported_ancestors = match ancestor_ids(
            row.get("parentPath"),
            &self.network_code,
            parent_id.as_deref(),
        ) {
            Ok(ancestors) => ancestors,
            Err(()) => {
                self.issues.insert("parent_path_invalid");
                Vec::new()
            }
        };
        let has_children = row.get("hasChildren").and_then(Value::as_bool);
        if has_children.is_none() {
            self.issues.insert("has_children_missing_or_invalid");
        }
        let status = match row.get("status").and_then(Value::as_str) {
            Some(status) if matches!(status, "ACTIVE" | "INACTIVE" | "ARCHIVED") => {
                Some(status.to_string())
            }
            _ => {
                self.issues.insert("status_missing_or_invalid");
                None
            }
        };
        if self.target_ids.contains(&ad_unit_id) {
            self.listed_target_ids.insert(ad_unit_id.clone());
        }
        let fingerprint = json!({
            "id": ad_unit_id,
            "status": status,
            "parent": parent_id,
            "ancestors": reported_ancestors,
            "has_children": has_children,
            "updated": bounded_string(row.get("updateTime"), 64),
        });
        let catalog_row = CatalogRow {
            status,
            parent_id,
            reported_ancestors,
            has_children,
            fingerprint,
        };
        if self.rows_by_id.insert(ad_unit_id, catalog_row).is_some() {
            self.issues.insert("duplicate_catalog_id");
        }
    }

    pub(super) fn finish(mut self, page_size: u32) -> Value {
        if self.listed_target_ids != self.target_ids {
            self.issues.insert("target_catalog_row_missing");
        }

        let actual_parent_ids = self
            .rows_by_id
            .values()
            .filter_map(|row| row.parent_id.clone())
            .collect::<BTreeSet<_>>();
        for (id, row) in &self.rows_by_id {
            let actual_has_children = actual_parent_ids.contains(id);
            if row.has_children != Some(actual_has_children) {
                self.issues.insert("catalog_child_flag_mismatch");
            }
            if self.target_ids.contains(id)
                && self.expected_has_children.get(id).copied() != row.has_children
            {
                self.issues.insert("identity_catalog_child_flag_mismatch");
            }
            match resolve_catalog_ancestors(id, &self.rows_by_id) {
                Some(resolved) if resolved == row.reported_ancestors => {}
                _ => {
                    self.issues.insert("catalog_ancestry_mismatch");
                }
            }
        }

        let mut external_descendants = Vec::new();
        let mut external_sample = Vec::new();
        let mut status_counts = BTreeMap::<String, u64>::new();
        let mut intra_target_hierarchy = Vec::new();
        let mut target_depths = BTreeMap::<String, usize>::new();
        for (id, row) in &self.rows_by_id {
            let known_ancestors =
                known_catalog_ancestors(id, &self.rows_by_id, &row.reported_ancestors);
            let matched_targets = known_ancestors
                .intersection(&self.target_ids)
                .cloned()
                .collect::<Vec<_>>();
            if matched_targets.is_empty() {
                continue;
            }
            if self.target_ids.contains(id) {
                target_depths.insert(id.clone(), matched_targets.len());
                if intra_target_hierarchy.len() < DESCENDANT_SAMPLE_LIMIT {
                    intra_target_hierarchy.push(json!({
                        "target_ad_unit_id": id,
                        "ancestor_target_ad_unit_ids": matched_targets,
                    }));
                }
                continue;
            }
            let status = row.status.as_deref().unwrap_or("UNKNOWN").to_string();
            *status_counts.entry(status.clone()).or_default() += 1;
            let summary = json!({
                "ad_unit_id": id,
                "status": status,
                "matched_target_ad_unit_ids": matched_targets,
            });
            if external_sample.len() < DESCENDANT_SAMPLE_LIMIT {
                external_sample.push(summary.clone());
            }
            external_descendants.push(summary);
        }
        let blocking_count = external_descendants
            .iter()
            .filter(|row| row.get("status").and_then(Value::as_str) != Some("ARCHIVED"))
            .count();
        let transport_complete = !self.issues.iter().any(|issue| {
            matches!(
                *issue,
                "ad_units_missing_or_invalid"
                    | "row_cap_reached"
                    | "page_response_bytes_exceeded"
                    | "scan_response_bytes_exceeded"
                    | "next_page_token_invalid"
                    | "zero_progress_page"
                    | "page_cap_reached"
                    | "repeated_page_token"
                    | "blocked_auth"
                    | "blocked_permission"
                    | "upstream_read_incomplete"
            )
        });
        let hierarchy_reconciled = self.issues.is_empty();
        let proof_state = if blocking_count > 0 && !hierarchy_reconciled {
            "partial_blocked"
        } else if blocking_count > 0 {
            "complete_blocked"
        } else if self.issues.contains("blocked_auth") {
            "blocked_auth"
        } else if self.issues.contains("blocked_permission") {
            "blocked_permission"
        } else if !hierarchy_reconciled {
            "partial_capped"
        } else {
            "complete_clear"
        };
        let mut child_first_order = self.target_ids.iter().cloned().collect::<Vec<_>>();
        child_first_order.sort_by(|left, right| {
            target_depths
                .get(right)
                .unwrap_or(&0)
                .cmp(target_depths.get(left).unwrap_or(&0))
                .then_with(|| left.cmp(right))
        });
        let fingerprint_rows = self
            .rows_by_id
            .values()
            .map(|row| row.fingerprint.clone())
            .collect::<Vec<_>>();
        json!({
            "proof_state": proof_state,
            "scan_complete": transport_complete,
            "hierarchy_reconciled": hierarchy_reconciled,
            "issues": self.issues,
            "page_count": self.page_count,
            "rows_scanned": self.rows_scanned,
            "response_bytes_scanned": self.response_bytes_scanned,
            "page_size": page_size,
            "max_pages": MAX_DESCENDANT_SCAN_PAGES,
            "max_ad_units": self.max_rows,
            "max_scan_response_bytes": MAX_DESCENDANT_SCAN_BYTES,
            "external_descendant_count": external_descendants.len(),
            "blocking_external_descendant_count": blocking_count,
            "external_descendant_status_counts": status_counts,
            "external_descendant_sample": external_sample,
            "external_descendant_sample_truncated": external_descendants.len() > DESCENDANT_SAMPLE_LIMIT,
            "intra_target_hierarchy": intra_target_hierarchy,
            "requires_child_first_sequence": !target_depths.is_empty(),
            "required_child_first_target_order": child_first_order,
            "catalog_fingerprint": stable_fingerprint(&Value::Array(fingerprint_rows).to_string()),
            "descendant_result_fingerprint": stable_fingerprint(&Value::Array(external_descendants).to_string()),
            "provider_request_state": self.provider_request_state,
        })
    }
}

pub(super) async fn scan_descendants_with_reader<F, Fut>(
    network_code: &str,
    target_ids: &[String],
    identity_child_claims: &BTreeMap<String, bool>,
    page_size: u32,
    max_rows: u32,
    mut read_page: F,
) -> (Value, usize)
where
    F: FnMut(String, u32, Option<String>) -> Fut,
    Fut: Future<Output = (Result<(Value, usize), AdManagerError>, bool)>,
{
    let mut scan = DescendantScan::new(network_code, target_ids, identity_child_claims, max_rows);
    let mut page_token = None;
    let mut request_attempted_count = 0usize;
    loop {
        let (result, request_attempted) =
            read_page(network_code.to_string(), page_size, page_token).await;
        request_attempted_count += usize::from(request_attempted);
        let (payload, response_bytes) = match result {
            Ok(page) => page,
            Err(err) => {
                scan.record_failure(err, request_attempted);
                break;
            }
        };
        let Some(next_token) = scan.consume_page(&payload, response_bytes) else {
            break;
        };
        page_token = Some(next_token);
    }
    (scan.finish(page_size), request_attempted_count)
}

fn ancestor_ids(
    parent_path: Option<&Value>,
    network_code: &str,
    direct_parent_id: Option<&str>,
) -> Result<Vec<String>, ()> {
    let entries = parent_path.and_then(Value::as_array).ok_or(())?;
    let mut ids = Vec::with_capacity(entries.len());
    let mut seen = BTreeSet::new();
    for entry in entries {
        let resource_name = entry
            .as_object()
            .and_then(|object| object.get("parentAdUnit"))
            .and_then(Value::as_str)
            .ok_or(())?;
        let id = scoped_numeric_id(resource_name, network_code).ok_or(())?;
        if !seen.insert(id.clone()) {
            return Err(());
        }
        ids.push(id);
    }
    if ids.last().map(String::as_str) != direct_parent_id {
        return Err(());
    }
    Ok(ids)
}

fn scoped_numeric_id(value: &str, network_code: &str) -> Option<String> {
    let prefix = format!("networks/{network_code}/adUnits/");
    let raw_id = value.strip_prefix(&prefix)?;
    if raw_id.is_empty()
        || raw_id.len() > 19
        || raw_id.starts_with('0')
        || raw_id.contains('/')
        || !raw_id.chars().all(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    raw_id
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value.to_string())
        .filter(|canonical| canonical == raw_id)
}

fn resolve_catalog_ancestors(
    ad_unit_id: &str,
    rows_by_id: &BTreeMap<String, CatalogRow>,
) -> Option<Vec<String>> {
    let mut resolved = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = rows_by_id.get(ad_unit_id)?.parent_id.clone();
    while let Some(parent_id) = current {
        if !seen.insert(parent_id.clone()) {
            return None;
        }
        resolved.push(parent_id.clone());
        current = rows_by_id.get(&parent_id)?.parent_id.clone();
    }
    resolved.reverse();
    Some(resolved)
}

fn known_catalog_ancestors(
    ad_unit_id: &str,
    rows_by_id: &BTreeMap<String, CatalogRow>,
    reported: &[String],
) -> BTreeSet<String> {
    let mut known = reported.iter().cloned().collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut current = rows_by_id
        .get(ad_unit_id)
        .and_then(|row| row.parent_id.clone());
    while let Some(parent_id) = current {
        if !seen.insert(parent_id.clone()) {
            break;
        }
        known.insert(parent_id.clone());
        current = rows_by_id
            .get(&parent_id)
            .and_then(|row| row.parent_id.clone());
    }
    known
}

fn bounded_string(value: Option<&Value>, max_chars: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|value| value.chars().take(max_chars).collect())
}
