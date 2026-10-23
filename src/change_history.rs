//! Bounded, provider-attributed Change_History reads and operator-supplied
//! field-level history inspection.

use chrono::{DateTime, SecondsFormat, Utc};
use roxmltree::{Document, Node};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AdManagerError;

pub(crate) const CHANGE_HISTORY_PAGE_SIZE: usize = 50;
pub(crate) const CHANGE_HISTORY_MAX_EVENTS: usize = 500;
pub(crate) const CHANGE_HISTORY_MAX_PAGES: usize = 10;
pub(crate) const CHANGE_HISTORY_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const CHANGE_HISTORY_MAX_WINDOW_SECONDS: i64 = 93 * 24 * 60 * 60;
const INSPECT_MAX_INPUT_BYTES: usize = 1024 * 1024;
const INSPECT_MAX_ROWS: usize = 500;
const INSPECT_MAX_CHANGES_PER_ROW: usize = 100;
const MAX_TEXT_BYTES: usize = 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChangeHistoryArgs {
    /// Raw Ad Manager network code.
    pub network_code: String,
    /// Inclusive RFC3339 start bound. The offset is required.
    pub start_time: String,
    /// Inclusive RFC3339 end bound. The offset is required.
    pub end_time: String,
    /// Optional exact numeric entity ID.
    pub entity_id: Option<String>,
    /// Optional Ad Manager Change_History entity-type allowlist.
    #[serde(default)]
    pub entity_types: Vec<String>,
    /// Maximum events to return. Values must be between 1 and 500.
    pub max_events: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChangeHistoryInspectArgs {
    /// SHA-256 asserted by the operator for the supplied source export.
    pub source_sha256: String,
    /// Supplied row references and authored field-level changes. The adapter does not open files or fetch source data.
    pub rows: Vec<ChangeHistoryInputRow>,
    /// Return supplied previous/current values only when the server's sensitive-history gate is enabled and the per-call acknowledgment is true.
    #[serde(default)]
    pub include_values: bool,
    /// Explicit per-call acknowledgment required when include_values is true.
    #[serde(default)]
    pub acknowledge_sensitive_values: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChangeHistoryInputRow {
    pub row_ref: String,
    pub change_date_time: String,
    pub entity_type: String,
    pub entity_id: String,
    pub operation: String,
    pub user_id: Option<String>,
    pub changes: Vec<ChangeHistoryFieldChange>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChangeHistoryFieldChange {
    pub field: String,
    pub previous: Value,
    pub current: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct ChangeHistoryWindow {
    pub start_utc: DateTime<Utc>,
    pub end_utc: DateTime<Utc>,
    pub entity_id: Option<String>,
    pub entity_types: Vec<String>,
    pub max_events: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChangeHistoryEvent {
    pub change_date_time: Value,
    pub entity_id: String,
    pub entity_type: String,
    pub operation: String,
    pub user_id: String,
    pub paging_id: String,
}

impl ChangeHistoryArgs {
    pub(crate) fn validate(self) -> Result<(String, ChangeHistoryWindow), AdManagerError> {
        let network_code = validate_digits("network_code", &self.network_code)?;
        let start_utc = parse_rfc3339("start_time", &self.start_time)?;
        let end_utc = parse_rfc3339("end_time", &self.end_time)?;
        if start_utc.timestamp_subsec_nanos() != 0 || end_utc.timestamp_subsec_nanos() != 0 {
            return Err(AdManagerError::invalid(
                "time_window",
                "start_time and end_time must have whole-second precision",
            ));
        }
        let window_seconds = end_utc.signed_duration_since(start_utc).num_seconds();
        if window_seconds <= 0 || window_seconds > CHANGE_HISTORY_MAX_WINDOW_SECONDS {
            return Err(AdManagerError::invalid(
                "time_window",
                "end_time must be after start_time and the window may not exceed 93 days",
            ));
        }
        let entity_id = self
            .entity_id
            .as_deref()
            .map(|value| validate_digits("entity_id", value))
            .transpose()?;
        if self.entity_types.len() > 20 {
            return Err(AdManagerError::invalid(
                "entity_types",
                "may contain at most 20 values",
            ));
        }
        let mut entity_types = Vec::with_capacity(self.entity_types.len());
        for entity_type in self.entity_types {
            let normalized = entity_type.trim().to_ascii_uppercase();
            if !is_entity_type(&normalized) {
                return Err(AdManagerError::invalid(
                    "entity_types",
                    format!("unsupported Change_History entity type `{normalized}`"),
                ));
            }
            if !entity_types.contains(&normalized) {
                entity_types.push(normalized);
            }
        }
        let max_events = self.max_events.unwrap_or(100);
        if !(1..=CHANGE_HISTORY_MAX_EVENTS).contains(&max_events) {
            return Err(AdManagerError::invalid(
                "max_events",
                format!("must be between 1 and {CHANGE_HISTORY_MAX_EVENTS}"),
            ));
        }
        Ok((
            network_code,
            ChangeHistoryWindow {
                start_utc,
                end_utc,
                entity_id,
                entity_types,
                max_events,
            },
        ))
    }
}

impl ChangeHistoryWindow {
    /// Builds a fixed-schema PQL query; only validated scalar filters and an
    /// internal provider paging token can influence the statement.
    pub(crate) fn query(&self, before_id: Option<&str>, limit: usize) -> String {
        let mut clauses = vec![format!(
            "ChangeDateTime >= '{}'",
            self.start_utc.to_rfc3339_opts(SecondsFormat::Secs, true)
        )];
        clauses.push(format!(
            "ChangeDateTime <= '{}'",
            self.end_utc.to_rfc3339_opts(SecondsFormat::Secs, true)
        ));
        if let Some(entity_id) = &self.entity_id {
            clauses.push(format!("EntityId = {entity_id}"));
        }
        if !self.entity_types.is_empty() {
            let values = self
                .entity_types
                .iter()
                .map(|value| format!("'{value}'"))
                .collect::<Vec<_>>()
                .join(", ");
            clauses.push(format!("EntityType IN ({values})"));
        }
        if let Some(before_id) = before_id {
            // PQL IDs are provider-issued text cursors. They are validated as
            // printable short text and never escape the in-call page loop.
            clauses.push(format!("Id < '{}'", escape_pql_string(before_id)));
        }
        format!(
            "SELECT ChangeDateTime, EntityId, EntityType, Id, Operation, UserId FROM Change_History WHERE {} ORDER BY ChangeDateTime DESC LIMIT {}",
            clauses.join(" AND "),
            limit.clamp(1, CHANGE_HISTORY_PAGE_SIZE)
        )
    }
}

pub(crate) fn validate_paging_id(value: &str) -> Result<(), AdManagerError> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(AdManagerError::UpstreamContract {
            field: "change_history.id",
            message: "provider returned an invalid paging identifier".to_string(),
        });
    }
    Ok(())
}

pub(crate) fn parse_change_history_xml(
    xml: &str,
) -> Result<Vec<ChangeHistoryEvent>, AdManagerError> {
    let document = Document::parse(xml).map_err(|_| AdManagerError::UpstreamContract {
        field: "change_history.xml",
        message: "provider returned malformed XML".to_string(),
    })?;
    let expected_columns = [
        "ChangeDateTime",
        "EntityId",
        "EntityType",
        "Id",
        "Operation",
        "UserId",
    ];
    let columns = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "columnTypes")
        .filter_map(|node| {
            node.children()
                .find(|child| child.is_element() && child.tag_name().name() == "labelName")
                .and_then(|child| child.text())
                .map(str::trim)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    if columns.iter().map(String::as_str).collect::<Vec<_>>() != expected_columns {
        return Err(contract(
            "change_history.columns",
            "provider returned unexpected column order or shape",
        ));
    }
    let rows = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "rows")
        .flat_map(|node| {
            node.children()
                .filter(|child| child.is_element() && child.tag_name().name() == "values")
        })
        .collect::<Vec<_>>();
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let values = row
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "value")
            .collect::<Vec<_>>();
        if values.len() != 6 {
            return Err(AdManagerError::UpstreamContract {
                field: "change_history.row",
                message: "provider returned an unexpected Change_History row shape".to_string(),
            });
        }
        let timestamp = parse_datetime_value(values[0])?;
        let entity_id = simple_value(values[1])?;
        let entity_type = simple_value(values[2])?;
        let paging_id = simple_value(values[3])?;
        let operation = simple_value(values[4])?;
        let user_id = simple_value(values[5])?;
        validate_paging_id(&paging_id)?;
        if !validate_digits("change_history.entity_id", &entity_id).is_ok()
            || !validate_digits("change_history.user_id", &user_id).is_ok()
        {
            return Err(AdManagerError::UpstreamContract {
                field: "change_history.row",
                message: "provider returned a non-numeric entity or user identifier".to_string(),
            });
        }
        events.push(ChangeHistoryEvent {
            change_date_time: timestamp,
            entity_id,
            entity_type,
            operation,
            user_id,
            paging_id,
        });
    }
    Ok(events)
}

pub(crate) fn inspect_history(
    args: ChangeHistoryInspectArgs,
    unredacted_enabled: bool,
) -> Result<Value, AdManagerError> {
    let bytes = serde_json::to_vec(&args.rows)
        .map_err(|_| AdManagerError::invalid("rows", "could not encode input"))?;
    if bytes.len() > INSPECT_MAX_INPUT_BYTES {
        return Err(AdManagerError::invalid("rows", "input exceeds 1 MiB"));
    }
    if !is_sha256(&args.source_sha256) {
        return Err(AdManagerError::invalid(
            "source_sha256",
            "must be 64 hexadecimal characters",
        ));
    }
    if args.rows.is_empty() || args.rows.len() > INSPECT_MAX_ROWS {
        return Err(AdManagerError::invalid(
            "rows",
            format!("must contain 1 to {INSPECT_MAX_ROWS} rows"),
        ));
    }
    if args.include_values && (!unredacted_enabled || !args.acknowledge_sensitive_values) {
        return Err(AdManagerError::WriteActionDisabled {
            message: "unredacted change-history values require the server gate and per-call acknowledgment".to_string(),
        });
    }
    let mut seen_refs = std::collections::BTreeSet::new();
    let mut output = Vec::with_capacity(args.rows.len());
    for row in args.rows {
        let row_ref = bounded_text("row_ref", &row.row_ref)?;
        if !seen_refs.insert(row_ref.clone()) {
            return Err(AdManagerError::invalid(
                "rows.row_ref",
                "must be unique within the supplied source",
            ));
        }
        let _ = parse_rfc3339("rows.change_date_time", &row.change_date_time)?;
        let entity_type = bounded_text("rows.entity_type", &row.entity_type)?;
        let operation = bounded_text("rows.operation", &row.operation)?;
        let entity_id = bounded_text("rows.entity_id", &row.entity_id)?;
        let user_id = row
            .user_id
            .as_deref()
            .map(|value| bounded_text("rows.user_id", value))
            .transpose()?;
        if row.changes.is_empty() || row.changes.len() > INSPECT_MAX_CHANGES_PER_ROW {
            return Err(AdManagerError::invalid(
                "rows.changes",
                format!("must contain 1 to {INSPECT_MAX_CHANGES_PER_ROW} changes"),
            ));
        }
        let changes = row
            .changes
            .into_iter()
            .map(|change| {
                let field = bounded_text("rows.changes.field", &change.field)?;
                Ok(if args.include_values {
                    json!({"field": field, "previous": change.previous, "current": change.current})
                } else {
                    json!({"field": field, "previous": "[redacted]", "current": "[redacted]"})
                })
            })
            .collect::<Result<Vec<_>, AdManagerError>>()?;
        output.push(json!({
            "row_ref": row_ref,
            "change_date_time": row.change_date_time,
            "entity_type": entity_type,
            "entity_id": if args.include_values { entity_id } else { "[redacted]".to_string() },
            "operation": operation,
            "user_id": if args.include_values { user_id.unwrap_or_else(|| "[absent]".to_string()) } else { "[redacted]".to_string() },
            "changes": changes,
        }));
    }
    Ok(json!({
        "source_sha256": args.source_sha256.to_ascii_lowercase(),
        "source_identity": "operator_asserted_not_verified",
        "row_count": output.len(),
        "values_included": args.include_values,
        "rows": output,
    }))
}

fn parse_datetime_value(node: Node<'_, '_>) -> Result<Value, AdManagerError> {
    let value = node
        .descendants()
        .find(|child| child.is_element() && child.tag_name().name() == "date")
        .ok_or_else(|| {
            contract(
                "change_history.change_date_time",
                "provider timestamp omitted date",
            )
        })?;
    let component = |name: &str| -> Result<i64, AdManagerError> {
        let text = value
            .children()
            .find(|child| child.is_element() && child.tag_name().name() == name)
            .and_then(|child| child.text())
            .ok_or_else(|| {
                contract(
                    "change_history.change_date_time",
                    "provider timestamp omitted a component",
                )
            })?;
        text.parse::<i64>().map_err(|_| {
            contract(
                "change_history.change_date_time",
                "provider timestamp contained an invalid component",
            )
        })
    };
    let year = component("year")?;
    let month = component("month")?;
    let day = component("day")?;
    let hour = node_text_descendant(node, "hour")
        .and_then(|x| x.parse::<i64>().ok())
        .ok_or_else(|| {
            contract(
                "change_history.change_date_time",
                "provider timestamp omitted hour",
            )
        })?;
    let minute = node_text_descendant(node, "minute")
        .and_then(|x| x.parse::<i64>().ok())
        .ok_or_else(|| {
            contract(
                "change_history.change_date_time",
                "provider timestamp omitted minute",
            )
        })?;
    let second = node_text_descendant(node, "second")
        .and_then(|x| x.parse::<i64>().ok())
        .ok_or_else(|| {
            contract(
                "change_history.change_date_time",
                "provider timestamp omitted second",
            )
        })?;
    let timezone =
        node_text_descendant(node, "timeZoneId").unwrap_or_else(|| "[unresolved]".to_string());
    Ok(
        json!({"year": year, "month": month, "day": day, "hour": hour, "minute": minute, "second": second, "time_zone_id": timezone}),
    )
}

fn simple_value(node: Node<'_, '_>) -> Result<String, AdManagerError> {
    node.descendants()
        .find(|child| {
            child.is_element() && child.tag_name().name() == "value" && child.text().is_some()
        })
        .and_then(|child| child.text())
        .or_else(|| node.text())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| contract("change_history.value", "provider returned an empty value"))
}

fn node_text_descendant(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.descendants()
        .find(|child| child.is_element() && child.tag_name().name() == name)
        .and_then(|child| child.text())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_rfc3339(field: &'static str, value: &str) -> Result<DateTime<Utc>, AdManagerError> {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.with_timezone(&Utc))
        .map_err(|_| {
            AdManagerError::invalid(
                field,
                "must be an RFC3339 timestamp with an explicit offset",
            )
        })
}

fn validate_digits(field: &'static str, value: &str) -> Result<String, AdManagerError> {
    if value.is_empty() || value.len() > 24 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AdManagerError::invalid(
            field,
            "must contain only 1 to 24 decimal digits",
        ));
    }
    Ok(value.to_string())
}

fn bounded_text(field: &'static str, value: &str) -> Result<String, AdManagerError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(AdManagerError::invalid(
            field,
            "must be non-empty, control-free text of at most 1024 bytes",
        ));
    }
    Ok(value.to_string())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_entity_type(value: &str) -> bool {
    matches!(
        value,
        "BASE_RATE"
            | "COMPANY"
            | "CONTACT"
            | "CREATIVE"
            | "CREATIVE_SET"
            | "CUSTOM_FIELD"
            | "CUSTOM_KEY"
            | "CUSTOM_VALUE"
            | "PLACEMENT"
            | "AD_UNIT"
            | "LABEL"
            | "LINE_ITEM"
            | "NETWORK"
            | "ORDER"
            | "PROPOSAL"
            | "PROPOSAL_LINK"
            | "PROPOSAL_LINE_ITEM"
            | "PACKAGE"
            | "ROLE"
            | "TEAM"
            | "USER"
    )
}

fn escape_pql_string(value: &str) -> String {
    value.replace('\'', "''")
}

fn contract(field: &'static str, message: &'static str) -> AdManagerError {
    AdManagerError::UpstreamContract {
        field,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChangeHistoryArgs, ChangeHistoryFieldChange, ChangeHistoryInputRow,
        ChangeHistoryInspectArgs,
    };
    use serde_json::json;

    #[test]
    fn bounded_change_history_query_keeps_provider_cursor_internal() {
        let (_, window) = ChangeHistoryArgs {
            network_code: "1234567".into(),
            start_time: "2026-10-01T00:00:00+11:00".into(),
            end_time: "2026-10-02T00:00:00+11:00".into(),
            entity_id: Some("9876".into()),
            entity_types: vec!["line_item".into()],
            max_events: Some(75),
        }
        .validate()
        .expect("valid history window");
        let query = window.query(Some("cursor_1"), 50);
        assert!(query.contains("ChangeDateTime >= '2026-09-30T13:00:00Z'"));
        assert!(query.contains("EntityId = 9876"));
        assert!(query.contains("EntityType IN ('LINE_ITEM')"));
        assert!(query.contains("Id < 'cursor_1'"));
        assert!(query.starts_with(
            "SELECT ChangeDateTime, EntityId, EntityType, Id, Operation, UserId FROM Change_History"
        ));
        assert!(query.ends_with("LIMIT 50"));
    }

    #[test]
    fn field_history_values_are_redacted_without_both_gates() {
        let args = ChangeHistoryInspectArgs {
            source_sha256: "a".repeat(64),
            rows: vec![ChangeHistoryInputRow {
                row_ref: "row-1".into(),
                change_date_time: "2026-10-01T00:00:00Z".into(),
                entity_type: "LINE_ITEM".into(),
                entity_id: "123".into(),
                operation: "UPDATE".into(),
                user_id: Some("456".into()),
                changes: vec![ChangeHistoryFieldChange {
                    field: "primaryGoal.units".into(),
                    previous: json!(50000),
                    current: json!(100000),
                }],
            }],
            include_values: false,
            acknowledge_sensitive_values: false,
        };
        let data = super::inspect_history(args, false).expect("redacted history");
        assert_eq!(data["rows"][0]["changes"][0]["previous"], "[redacted]");
        assert_eq!(data["rows"][0]["entity_id"], "[redacted]");
        assert_eq!(data["source_identity"], "operator_asserted_not_verified");
    }

    #[test]
    fn parses_documented_provider_change_history_shape() {
        let xml = r#"<selectResponse xmlns="https://www.google.com/apis/ads/publisher/v202605">
          <rval>
            <columnTypes><labelName>ChangeDateTime</labelName></columnTypes>
            <columnTypes><labelName>EntityId</labelName></columnTypes>
            <columnTypes><labelName>EntityType</labelName></columnTypes>
            <columnTypes><labelName>Id</labelName></columnTypes>
            <columnTypes><labelName>Operation</labelName></columnTypes>
            <columnTypes><labelName>UserId</labelName></columnTypes>
            <rows><values>
              <value xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="DateTimeValue"><value><date><year>2026</year><month>10</month><day>22</day></date><hour>12</hour><minute>5</minute><second>4</second><timeZoneId>Australia/Sydney</timeZoneId></value></value>
              <value xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="NumberValue"><value>123456</value></value>
              <value xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="TextValue"><value>LINE_ITEM</value></value>
              <value xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="TextValue"><value>cursor-1</value></value>
              <value xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="TextValue"><value>UPDATE</value></value>
              <value xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="NumberValue"><value>789</value></value>
            </values></rows>
          </rval>
        </selectResponse>"#;
        let rows = super::parse_change_history_xml(xml).expect("valid provider result");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entity_id, "123456");
        assert_eq!(rows[0].entity_type, "LINE_ITEM");
        assert_eq!(rows[0].operation, "UPDATE");
        assert_eq!(rows[0].user_id, "789");
        assert_eq!(rows[0].paging_id, "cursor-1");
        assert_eq!(rows[0].change_date_time["time_zone_id"], "Australia/Sydney");
    }

    #[test]
    fn rejects_subsecond_bounds_instead_of_truncating_them() {
        let result = ChangeHistoryArgs {
            network_code: "1234567".into(),
            start_time: "2026-10-01T00:00:00.500Z".into(),
            end_time: "2026-10-02T00:00:00Z".into(),
            entity_id: None,
            entity_types: vec![],
            max_events: Some(1),
        }
        .validate();
        assert!(result.is_err());
    }
}
