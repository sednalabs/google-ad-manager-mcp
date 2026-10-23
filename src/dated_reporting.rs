//! Fixed-shape, bounded dated delivery report planning and CSV projection.

use std::collections::{BTreeMap, VecDeque};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::AdManagerError;

pub(crate) const MAX_LINE_ITEM_IDS: usize = 20;
pub(crate) const MAX_DATE_SPAN_DAYS: u32 = 62;
pub(crate) const MAX_REPORT_CSV_BYTES: usize = 5 * 1024 * 1024;
pub(crate) const MAX_REPORT_ROWS: usize = 10_000;
pub(crate) const MAX_REPORT_CSV_COLUMNS: usize = 6;
pub(crate) const MAX_BOUND_REPORT_JOBS: usize = 256;

const EXPECTED_HEADERS: [&str; MAX_REPORT_CSV_COLUMNS] = [
    "Dimension.DATE",
    "Dimension.LINE_ITEM_ID",
    "Dimension.CREATIVE_ID",
    "Dimension.CREATIVE_SIZE_DELIVERED",
    "Column.AD_SERVER_IMPRESSIONS",
    "Column.AD_SERVER_CLICKS",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DatedDeliveryReportRequest {
    pub network_code: String,
    pub line_item_ids: Vec<String>,
    pub start_date: String,
    pub end_date: String,
    /// Expected publisher IANA time zone, checked against the network before
    /// the report job is submitted and again when results are read.
    pub publisher_time_zone: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DatedDeliveryReportPlan {
    pub network_code: String,
    pub line_item_ids: Vec<String>,
    pub start_date: String,
    pub end_date: String,
    pub publisher_time_zone: String,
    pub statement: String,
    pub dimensions: [&'static str; 4],
    pub columns: [&'static str; 2],
    pub report_query_xml: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DatedDeliveryRow {
    pub date: String,
    pub line_item_id: String,
    pub creative_id: String,
    pub creative_size: String,
    pub impressions: u64,
    pub clicks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DatedDeliveryCsv {
    pub rows: Vec<DatedDeliveryRow>,
    pub complete: bool,
    pub truncation_reason: Option<&'static str>,
}

/// Process-local binding between a provider-assigned job id and the exact
/// canonical request that created it. The provider API exposes job status and
/// downloads by id, but no operation to read a job's query back.
#[derive(Debug, Default)]
pub(crate) struct DatedReportJobRegistry {
    by_job: BTreeMap<(String, String), DatedDeliveryReportPlan>,
    insertion_order: VecDeque<(String, String)>,
}

impl DatedReportJobRegistry {
    pub(crate) fn bind(
        &mut self,
        report_job_id: &str,
        plan: &DatedDeliveryReportPlan,
    ) -> Result<(), AdManagerError> {
        let key = (plan.network_code.clone(), report_job_id.to_string());
        if let Some(existing) = self.by_job.get(&key) {
            return if existing == plan {
                Ok(())
            } else {
                Err(AdManagerError::DatedReportJobBinding {
                    message: "provider repeated a report-job id with a conflicting request binding; refusing to replace the original association".to_string(),
                })
            };
        }
        if self.by_job.len() >= MAX_BOUND_REPORT_JOBS
            && let Some(oldest) = self.insertion_order.pop_front()
        {
            self.by_job.remove(&oldest);
        }
        self.insertion_order.push_back(key.clone());
        self.by_job.insert(key, plan.clone());
        Ok(())
    }

    pub(crate) fn require_match(
        &self,
        network_code: &str,
        report_job_id: &str,
        plan: &DatedDeliveryReportPlan,
    ) -> Result<(), AdManagerError> {
        if self
            .by_job
            .get(&(network_code.to_string(), report_job_id.to_string()))
            .is_some_and(|bound| bound == plan)
        {
            return Ok(());
        }
        Err(AdManagerError::DatedReportJobBinding {
            message: "no verified association for this network, report-job id and exact request is available in this MCP process (the id may be unknown, evicted, or from an earlier process); the query cannot be verified, so no provider read was attempted. This does not establish that a provider job is absent; do not resubmit automatically".to_string(),
        })
    }
}

pub(crate) fn build_plan(
    request: DatedDeliveryReportRequest,
) -> Result<DatedDeliveryReportPlan, AdManagerError> {
    let network_code = positive_numeric_id("network_code", &request.network_code)?;
    if request.line_item_ids.is_empty() || request.line_item_ids.len() > MAX_LINE_ITEM_IDS {
        return Err(AdManagerError::invalid(
            "line_item_ids",
            format!("must contain between 1 and {MAX_LINE_ITEM_IDS} exact line-item ids"),
        ));
    }
    let mut line_item_ids = request
        .line_item_ids
        .iter()
        .map(|id| positive_numeric_id("line_item_ids", id))
        .collect::<Result<Vec<_>, _>>()?;
    line_item_ids.sort_by(|left, right| {
        left.parse::<u64>()
            .unwrap_or_default()
            .cmp(&right.parse::<u64>().unwrap_or_default())
    });
    if line_item_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(AdManagerError::invalid(
            "line_item_ids",
            "must not contain duplicate ids",
        ));
    }

    let start = parse_date("start_date", &request.start_date)?;
    let end = parse_date("end_date", &request.end_date)?;
    if end < start {
        return Err(AdManagerError::invalid(
            "date_range",
            "end_date must be on or after start_date",
        ));
    }
    if inclusive_day_count_exceeds(start, end, MAX_DATE_SPAN_DAYS) {
        return Err(AdManagerError::invalid(
            "date_range",
            format!("inclusive range must not exceed {MAX_DATE_SPAN_DAYS} days"),
        ));
    }
    let publisher_time_zone = validate_timezone(&request.publisher_time_zone)?;
    let statement = format!("WHERE LINE_ITEM_ID IN ({})", line_item_ids.join(", "));
    let start_xml = date_xml(start);
    let end_xml = date_xml(end);
    let report_query_xml = format!(
        "<reportQuery>\n  <dimensions>DATE</dimensions>\n  <dimensions>LINE_ITEM_ID</dimensions>\n  <dimensions>CREATIVE_ID</dimensions>\n  <dimensions>CREATIVE_SIZE_DELIVERED</dimensions>\n  <columns>AD_SERVER_IMPRESSIONS</columns>\n  <columns>AD_SERVER_CLICKS</columns>\n  <dateRangeType>CUSTOM_DATE</dateRangeType>\n  <startDate>{start_xml}</startDate>\n  <endDate>{end_xml}</endDate>\n  <timeZoneType>PUBLISHER</timeZoneType>\n  <statement><query>{statement}</query></statement>\n</reportQuery>"
    );
    Ok(DatedDeliveryReportPlan {
        network_code,
        line_item_ids,
        start_date: start.to_string(),
        end_date: end.to_string(),
        publisher_time_zone,
        statement,
        dimensions: [
            "DATE",
            "LINE_ITEM_ID",
            "CREATIVE_ID",
            "CREATIVE_SIZE_DELIVERED",
        ],
        columns: ["AD_SERVER_IMPRESSIONS", "AD_SERVER_CLICKS"],
        report_query_xml,
    })
}

pub(crate) fn parse_report_csv(
    bytes: &[u8],
    plan: &DatedDeliveryReportPlan,
) -> Result<DatedDeliveryCsv, AdManagerError> {
    if bytes.len() > MAX_REPORT_CSV_BYTES {
        return Ok(DatedDeliveryCsv {
            rows: Vec::new(),
            complete: false,
            truncation_reason: Some("byte_cap"),
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| AdManagerError::UpstreamContract {
        field: "dated_delivery.csv",
        message: "report CSV is not valid UTF-8".to_string(),
    })?;
    let mut records = parse_csv_records(text)?;
    if records.is_empty() {
        return Err(csv_contract("report CSV has no header row"));
    }
    let headers = records.remove(0);
    if headers.len() != EXPECTED_HEADERS.len()
        || headers
            .iter()
            .zip(EXPECTED_HEADERS)
            .any(|(actual, expected)| actual.trim_start_matches('\u{feff}') != expected)
    {
        return Err(csv_contract(
            "report CSV headers do not match the fixed report schema",
        ));
    }

    let allowed_line_items = plan
        .line_item_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut rows = Vec::with_capacity(records.len().min(MAX_REPORT_ROWS));
    let mut complete = true;
    let mut truncation_reason = None;
    for (index, record) in records.into_iter().enumerate() {
        if record.iter().all(|field| field.is_empty()) {
            continue;
        }
        if index >= MAX_REPORT_ROWS {
            complete = false;
            truncation_reason = Some("row_cap");
            break;
        }
        if record.len() != EXPECTED_HEADERS.len() {
            return Err(csv_contract(
                "report CSV row has an unexpected column count",
            ));
        }
        let date = parse_date("report_date", &record[0])?.to_string();
        if date < plan.start_date || date > plan.end_date {
            return Err(csv_contract(
                "report CSV contains a date outside the requested range",
            ));
        }
        let line_item_id = positive_numeric_id("report_line_item_id", &record[1])?;
        if !allowed_line_items.contains(&line_item_id) {
            return Err(csv_contract(
                "report CSV contains an unrequested line-item id",
            ));
        }
        let creative_id = record[2].trim().to_string();
        if !creative_id.is_empty()
            && creative_id != "N/A"
            && !creative_id.chars().all(|ch| ch.is_ascii_digit())
        {
            return Err(csv_contract("report CSV contains an invalid creative id"));
        }
        rows.push(DatedDeliveryRow {
            date,
            line_item_id,
            creative_id,
            creative_size: record[3].trim().to_string(),
            impressions: nonnegative_integer("impressions", &record[4])?,
            clicks: nonnegative_integer("clicks", &record[5])?,
        });
    }
    Ok(DatedDeliveryCsv {
        rows,
        complete,
        truncation_reason,
    })
}

fn parse_csv_records(text: &str) -> Result<Vec<Vec<String>>, AdManagerError> {
    let mut records = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut closed_quote = false;
    while let Some(ch) = chars.next() {
        if quoted {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => {
                    quoted = false;
                    closed_quote = true;
                }
                _ => field.push(ch),
            }
            continue;
        }
        if closed_quote && !matches!(ch, ',' | '\r' | '\n') {
            return Err(csv_contract("report CSV has data after a closing quote"));
        }
        match ch {
            '"' if field.is_empty() && !closed_quote => quoted = true,
            '"' => return Err(csv_contract("report CSV has a quote in an unquoted field")),
            ',' => {
                if row.len() >= MAX_REPORT_CSV_COLUMNS {
                    return Err(csv_contract("report CSV exceeds the fixed column count"));
                }
                row.push(std::mem::take(&mut field));
                closed_quote = false;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut row));
                closed_quote = false;
            }
            _ => field.push(ch),
        }
    }
    if quoted {
        return Err(csv_contract("report CSV ends inside a quoted field"));
    }
    if !field.is_empty() || !row.is_empty() || closed_quote {
        row.push(field);
        records.push(row);
    }
    Ok(records)
}

fn numeric_id(field: &'static str, value: &str) -> Result<String, AdManagerError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 32 || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(AdManagerError::invalid(
            field,
            "must be a bounded numeric identifier",
        ));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| AdManagerError::invalid(field, "must be a bounded numeric identifier"))?;
    Ok(parsed.to_string())
}

fn positive_numeric_id(field: &'static str, value: &str) -> Result<String, AdManagerError> {
    let value = numeric_id(field, value)?;
    if value.parse::<u64>().ok().is_none_or(|number| number == 0) {
        return Err(AdManagerError::invalid(
            field,
            "must be a positive numeric identifier",
        ));
    }
    Ok(value)
}

fn validate_timezone(value: &str) -> Result<String, AdManagerError> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.contains('/')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '+'))
        && !value.contains("//")
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if !valid {
        return Err(AdManagerError::invalid(
            "publisher_time_zone",
            "must be an IANA time-zone identifier such as Australia/Sydney",
        ));
    }
    Ok(value.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CivilDate {
    year: i32,
    month: u32,
    day: u32,
}

impl std::fmt::Display for CivilDate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

fn parse_date(field: &'static str, value: &str) -> Result<CivilDate, AdManagerError> {
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return Err(AdManagerError::invalid(
            field,
            "must be a calendar date in YYYY-MM-DD form",
        ));
    }
    let year = parts[0].parse::<i32>().ok();
    let month = parts[1].parse::<u32>().ok();
    let day = parts[2].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return Err(AdManagerError::invalid(
            field,
            "must be a calendar date in YYYY-MM-DD form",
        ));
    };
    if !(1..=12).contains(&month) || !(1..=days_in_month_for_year(year, month)).contains(&day) {
        return Err(AdManagerError::invalid(
            field,
            "must be a valid calendar date",
        ));
    }
    Ok(CivilDate { year, month, day })
}

fn days_in_month(month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28,
        _ => 0,
    }
}

fn days_in_month_for_year(year: i32, month: u32) -> u32 {
    if month == 2 && (year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)) {
        29
    } else {
        days_in_month(month)
    }
}

fn inclusive_day_count_exceeds(start: CivilDate, end: CivilDate, limit: u32) -> bool {
    let mut count = 1;
    let mut date = start;
    while date < end && count <= limit {
        date = next_date(date);
        count += 1;
    }
    count > limit
}

fn next_date(date: CivilDate) -> CivilDate {
    if date.day < days_in_month_for_year(date.year, date.month) {
        CivilDate {
            day: date.day + 1,
            ..date
        }
    } else if date.month < 12 {
        CivilDate {
            month: date.month + 1,
            day: 1,
            ..date
        }
    } else {
        CivilDate {
            year: date.year + 1,
            month: 1,
            day: 1,
        }
    }
}

fn date_xml(date: CivilDate) -> String {
    format!(
        "<year>{}</year><month>{}</month><day>{}</day>",
        date.year, date.month, date.day
    )
}

fn nonnegative_integer(field: &'static str, value: &str) -> Result<u64, AdManagerError> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| csv_contract_static(field))
}

fn csv_contract(message: &str) -> AdManagerError {
    AdManagerError::UpstreamContract {
        field: "dated_delivery.csv",
        message: message.to_string(),
    }
}

fn csv_contract_static(field: &'static str) -> AdManagerError {
    AdManagerError::UpstreamContract {
        field: "dated_delivery.csv",
        message: format!("report CSV contains an invalid {field} value"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DatedDeliveryReportRequest, DatedReportJobRegistry, MAX_BOUND_REPORT_JOBS,
        MAX_DATE_SPAN_DAYS, MAX_REPORT_ROWS, build_plan, parse_report_csv,
    };

    fn request() -> DatedDeliveryReportRequest {
        DatedDeliveryReportRequest {
            network_code: "1234567".into(),
            line_item_ids: vec!["900".into(), "42".into()],
            start_date: "2026-10-01".into(),
            end_date: "2026-10-31".into(),
            publisher_time_zone: "Australia/Sydney".into(),
        }
    }

    #[test]
    fn plan_is_fixed_shape_and_sorts_exact_positive_line_items() {
        let plan = build_plan(request()).expect("valid bounded plan");
        assert_eq!(plan.line_item_ids, ["42", "900"]);
        assert_eq!(plan.statement, "WHERE LINE_ITEM_ID IN (42, 900)");
        assert!(
            plan.report_query_xml
                .contains("<timeZoneType>PUBLISHER</timeZoneType>")
        );
        assert!(
            plan.report_query_xml
                .contains("<year>2026</year><month>10</month><day>1</day>")
        );
        assert!(plan.report_query_xml.contains("<day>31</day>"));
        assert!(!plan.report_query_xml.contains("ORDER BY"));
    }

    #[test]
    fn report_job_registry_accepts_only_the_exact_bound_query() {
        let accepted = build_plan(request()).unwrap();
        let mut registry = DatedReportJobRegistry::default();
        registry.bind("7654321", &accepted).unwrap();
        assert!(
            registry
                .require_match(&accepted.network_code, "7654321", &accepted)
                .is_ok()
        );

        let mut changed_requests = vec![
            DatedDeliveryReportRequest {
                network_code: "7654322".into(),
                ..request()
            },
            DatedDeliveryReportRequest {
                line_item_ids: vec!["42".into()],
                ..request()
            },
            DatedDeliveryReportRequest {
                start_date: "2026-10-02".into(),
                ..request()
            },
            DatedDeliveryReportRequest {
                end_date: "2026-10-30".into(),
                ..request()
            },
            DatedDeliveryReportRequest {
                publisher_time_zone: "America/Los_Angeles".into(),
                ..request()
            },
        ];
        // The provider schema is fixed by the adapter rather than caller input;
        // explicitly exercise exact equality if that shape changes internally.
        let mut changed_metrics = accepted.clone();
        changed_metrics.columns = ["AD_SERVER_CLICKS", "AD_SERVER_IMPRESSIONS"];
        assert!(
            registry
                .require_match(&accepted.network_code, "7654321", &changed_metrics)
                .is_err()
        );

        for changed in changed_requests.drain(..) {
            let changed_plan = build_plan(changed).unwrap();
            assert!(
                registry
                    .require_match(&changed_plan.network_code, "7654321", &changed_plan)
                    .is_err()
            );
        }
        let narrower = build_plan(DatedDeliveryReportRequest {
            line_item_ids: vec!["42".into()],
            start_date: "2026-10-02".into(),
            end_date: "2026-10-02".into(),
            ..request()
        })
        .unwrap();
        let mut fresh_registry = DatedReportJobRegistry::default();
        fresh_registry.bind("7654322", &narrower).unwrap();
        // A narrower provider result cannot be presented as complete for the
        // broader caller-supplied plan, even when its rows are in scope.
        assert!(
            fresh_registry
                .require_match(&accepted.network_code, "7654322", &accepted)
                .is_err()
        );
        assert!(
            registry
                .require_match(&accepted.network_code, "9999999", &accepted)
                .is_err()
        );
    }

    #[test]
    fn report_job_registry_fails_closed_on_restart_eviction_and_conflict() {
        let plan = build_plan(request()).unwrap();
        let mut registry = DatedReportJobRegistry::default();
        registry.bind("1", &plan).unwrap();
        let conflicting = build_plan(DatedDeliveryReportRequest {
            end_date: "2026-10-30".into(),
            ..request()
        })
        .unwrap();
        assert!(registry.bind("1", &conflicting).is_err());
        assert!(
            registry
                .require_match(&plan.network_code, "1", &plan)
                .is_ok()
        );

        let restarted = DatedReportJobRegistry::default();
        assert!(
            restarted
                .require_match(&plan.network_code, "1", &plan)
                .is_err()
        );

        let mut bounded = DatedReportJobRegistry::default();
        for id in 1..=(MAX_BOUND_REPORT_JOBS + 1) {
            bounded.bind(&id.to_string(), &plan).unwrap();
        }
        assert!(
            bounded
                .require_match(&plan.network_code, "1", &plan)
                .is_err()
        );
        assert!(
            bounded
                .require_match(
                    &plan.network_code,
                    &(MAX_BOUND_REPORT_JOBS + 1).to_string(),
                    &plan
                )
                .is_ok()
        );
    }

    #[test]
    fn rejects_empty_zero_duplicate_or_oversized_line_ids() {
        for ids in [vec![], vec!["0".into()], vec!["4".into(), "4".into()]] {
            let mut input = request();
            input.line_item_ids = ids;
            assert!(build_plan(input).is_err());
        }
        let mut input = request();
        input.line_item_ids = (1..=21).map(|value| value.to_string()).collect();
        assert!(build_plan(input).is_err());
    }

    #[test]
    fn rejects_bad_or_too_wide_inclusive_date_ranges() {
        let mut input = request();
        input.start_date = "2026-02-30".into();
        assert!(build_plan(input).is_err());
        assert_eq!(MAX_DATE_SPAN_DAYS, 62);
        let mut boundary = request();
        boundary.start_date = "2026-10-01".into();
        boundary.end_date = "2026-12-01".into();
        assert!(build_plan(boundary).is_ok());
        let mut over_boundary = request();
        over_boundary.start_date = "2026-10-01".into();
        over_boundary.end_date = "2026-12-02".into();
        assert!(build_plan(over_boundary).is_err());
    }

    #[test]
    fn parses_fixed_schema_and_rejects_rows_outside_plan() {
        let plan = build_plan(request()).unwrap();
        let csv = concat!(
            "Dimension.DATE,Dimension.LINE_ITEM_ID,Dimension.CREATIVE_ID,Dimension.CREATIVE_SIZE_DELIVERED,Column.AD_SERVER_IMPRESSIONS,Column.AD_SERVER_CLICKS\r\n",
            "2026-10-02,42,77,300x250,123,4\r\n",
        );
        let parsed = parse_report_csv(csv.as_bytes(), &plan).unwrap();
        assert!(parsed.complete);
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].impressions, 123);
        let out_of_scope = csv.replace(",42,", ",99,");
        assert!(parse_report_csv(out_of_scope.as_bytes(), &plan).is_err());
    }

    #[test]
    fn csv_parser_handles_quoted_commas_and_embedded_newlines() {
        let plan = build_plan(request()).unwrap();
        let csv = concat!(
            "Dimension.DATE,Dimension.LINE_ITEM_ID,Dimension.CREATIVE_ID,Dimension.CREATIVE_SIZE_DELIVERED,Column.AD_SERVER_IMPRESSIONS,Column.AD_SERVER_CLICKS\n",
            "2026-10-02,42,77,\"300x250,\nresponsive\",123,4\n",
        );
        let parsed = parse_report_csv(csv.as_bytes(), &plan).unwrap();
        assert_eq!(parsed.rows[0].creative_size, "300x250,\nresponsive");
    }

    #[test]
    fn row_cap_returns_explicit_incomplete_status() {
        let plan = build_plan(request()).unwrap();
        let mut csv = String::from(
            "Dimension.DATE,Dimension.LINE_ITEM_ID,Dimension.CREATIVE_ID,Dimension.CREATIVE_SIZE_DELIVERED,Column.AD_SERVER_IMPRESSIONS,Column.AD_SERVER_CLICKS\n",
        );
        for _ in 0..=MAX_REPORT_ROWS {
            csv.push_str("2026-10-02,42,77,300x250,1,0\n");
        }
        let parsed = parse_report_csv(csv.as_bytes(), &plan).unwrap();
        assert!(!parsed.complete);
        assert_eq!(parsed.truncation_reason, Some("row_cap"));
        assert_eq!(parsed.rows.len(), MAX_REPORT_ROWS);
    }

    #[test]
    fn rejects_header_drift_and_malformed_csv() {
        let plan = build_plan(request()).unwrap();
        assert!(parse_report_csv(b"bad,headers\n", &plan).is_err());
        let mut csv = b"Dimension.DATE,Dimension.LINE_ITEM_ID,Dimension.CREATIVE_ID,Dimension.CREATIVE_SIZE_DELIVERED,Column.AD_SERVER_IMPRESSIONS,Column.AD_SERVER_CLICKS\n2026-10-02,42,77,300x250,1,0\n".to_vec();
        csv.extend_from_slice(b"\"unterminated");
        assert!(parse_report_csv(&csv, &plan).is_err());
    }

    #[test]
    fn byte_cap_returns_explicit_incomplete_status_without_parsing_raw_export() {
        let plan = build_plan(request()).unwrap();
        let bytes = vec![b'x'; super::MAX_REPORT_CSV_BYTES + 1];
        let parsed = parse_report_csv(&bytes, &plan).unwrap();
        assert!(!parsed.complete);
        assert_eq!(parsed.truncation_reason, Some("byte_cap"));
        assert!(parsed.rows.is_empty());
    }
}
