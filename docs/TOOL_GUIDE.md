# Tool Guide

All tools return Contract V1 envelopes: `ok/data/meta` on success and
`ok/error/meta` on failure.

## Tools

| Tool | Purpose |
| --- | --- |
| `find_tools` | Search tool metadata for deferred-loading or `tool_search` clients. |
| `gam_get_started` | Return the recommended first-run flow and supported credential sources. |
| `gam_auth_status` | Inspect configured auth inputs and optionally prove live Ad Manager access. |
| `gam_auth_login_command` | Build a copyable ADC login command without running it. |
| `gam_networks_list` | List Ad Manager networks visible to the authenticated principal. |
| `gam_network_catalog_list` | List one curated network collection: `ad_units`, `orders`, `line_items`, or `reports`. |
| `gam_report_run` | Run a saved Ad Manager report, optionally wait, and optionally fetch the first result page. |
| `gam_report_result_rows` | Fetch rows from a completed report result resource. |
| `gam_scratchpad_open_session` | Open or refresh a bounded local DuckDB scratchpad session. |
| `gam_scratchpad_close_session` | Close a scratchpad session and remove its local database. |
| `gam_scratchpad_list_sessions` | List active scratchpad sessions. |
| `gam_scratchpad_list_tables` | List tables in a scratchpad session. |
| `gam_scratchpad_drop_table` | Drop one scratchpad table. |
| `gam_scratchpad_query` | Run guarded read-only DuckDB SQL against a scratchpad session. |
| `gam_scratchpad_ingest_network_catalog` | Fetch one network catalog page and ingest it into a scratchpad table. |
| `gam_scratchpad_ingest_report_result_rows` | Fetch one report-result page and ingest it into a scratchpad table. |
| `gam_scratchpad_export_evidence_bundle` | Export bounded markdown evidence from scratchpad tables. |

## `gam_network_catalog_list`

`gam_network_catalog_list` is intentionally curated rather than generic. The
`collection` field is an allowlist:

- `ad_units`
- `orders`
- `line_items`
- `reports`

This keeps the tool useful without turning the MCP into an arbitrary upstream
endpoint browser.

## `gam_report_run`

`gam_report_run` is designed for saved reports that already exist in Ad
Manager. It accepts:

- `network_code`
- `report_id`
- optional wait controls
- optional first-page fetch controls

When `wait_for_completion=true`, the tool polls the Ad Manager long-running
operation and returns the `report_result` resource name once complete. If
`fetch_first_page=true`, it also returns the first page of rows so the first
successful report run is immediately useful.

Use `gam_report_result_rows` with the returned `result_name` when:

- the first page was truncated;
- you intentionally skipped fetching the first page during `gam_report_run`;
- you want to revisit a completed report result later.

## Scratchpad Tools

Scratchpad tools are local analysis helpers. They do not write to Google Ad
Manager. They create bounded DuckDB sessions inside the MCP server process so a
client can inspect larger catalog/report pages without repeatedly passing every
row through chat.

Typical flow:

1. `gam_scratchpad_open_session` with a stable `session_id`.
2. `gam_scratchpad_ingest_network_catalog` for ad units, orders, line items, or
   reports.
3. `gam_scratchpad_ingest_report_result_rows` for completed report result pages.
4. `gam_scratchpad_query` with read-only SQL.
5. `gam_scratchpad_export_evidence_bundle` for a bounded markdown summary.
6. `gam_scratchpad_close_session` when finished.

Example query:

```json
{
  "session_id": "gam_delivery_review",
  "sql": "SELECT collection, status, COUNT(*) AS rows FROM ad_units GROUP BY collection, status ORDER BY rows DESC",
  "page_size": 50
}
```

The scratchpad SQL policy allows read-only inspection patterns such as
`SELECT`, `WITH`, `EXPLAIN`, `DESCRIBE`, and `SUMMARIZE`. File access,
external scans, mutations, and long-running queries are rejected or timed out.
