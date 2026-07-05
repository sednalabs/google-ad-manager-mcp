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
| `gam_trafficking_tool_matrix` | Describe REST-supported write operations and SOAP-only trafficking gaps. |
| `gam_rest_write_plan` | Create a dry-run plan and confirmation token for an allowlisted REST write. |
| `gam_rest_write_apply` | Apply an allowlisted REST write after runtime, scope, and confirmation gates. |
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

## Write And Trafficking Tools

Write tools are available as a guarded preview/apply pair:

1. `gam_trafficking_tool_matrix`
2. `gam_rest_write_plan`
3. `gam_rest_write_apply`

The default runtime mode is `preview_only`. In that mode, `gam_rest_write_plan`
can return the exact REST request shape and confirmation token, but
`gam_rest_write_apply` fails closed.

Live apply requires all of the following:

- `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`
- `GOOGLE_AD_MANAGER_MCP_SCOPE=https://www.googleapis.com/auth/admanager`
- the exact `confirmation_token` from `gam_rest_write_plan`
- a non-empty `reason`
- `expected_impact`
- `rollback_note`

For local ADC credentials, the easiest way to request the manage scope is:

```bash
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> --manage-scope
```

The REST write surface is allowlisted from the official Ad Manager REST beta
discovery document. It includes inventory/supporting resources such as
`ad_units`, `placements`, `reports`, `labels`, `teams`, `contacts`,
`custom_fields`, `custom_targeting_keys`, `applications`, `sites`, and related
batch state actions.

The current REST beta surface does not expose order or line-item create/update
actions. `gam_trafficking_tool_matrix` reports those as SOAP-only gaps rather
than presenting fake tools that would fail at apply time.

Example dry-run plan for a saved-report patch:

```json
{
  "request": {
    "network_code": "1234567",
    "resource": "reports",
    "operation": "patch",
    "resource_name": "networks/1234567/reports/987654",
    "update_mask": "displayName",
    "body": {
      "name": "networks/1234567/reports/987654",
      "displayName": "Campaign delivery proof"
    },
    "reason": "Rename a saved report used for delivery proof packs.",
    "expected_impact": "Report name only; no campaign delivery changes.",
    "rollback_note": "Patch displayName back to the previous value.",
    "idempotency_key": "ticket-123"
  }
}
```

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
