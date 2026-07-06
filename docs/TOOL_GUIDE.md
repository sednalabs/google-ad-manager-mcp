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
| `gam_trafficking_tool_matrix` | Describe REST-supported writes, SOAP trafficking operations, and remaining ergonomics gaps. |
| `gam_rest_write_plan` | Create a dry-run plan and confirmation token for an allowlisted REST write. |
| `gam_rest_write_apply` | Apply an allowlisted REST write after runtime, scope, and confirmation gates. |
| `gam_soap_trafficking_plan` | Create a dry-run plan and confirmation token for an allowlisted SOAP trafficking or forecast operation. |
| `gam_soap_trafficking_apply` | Run an allowlisted SOAP trafficking or forecast operation after scope, runtime, and confirmation gates. |
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

Write and trafficking tools are available as guarded preview/apply pairs:

1. `gam_trafficking_tool_matrix`
2. `gam_rest_write_plan`
3. `gam_rest_write_apply`
4. `gam_soap_trafficking_plan`
5. `gam_soap_trafficking_apply`

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

Classic trafficking remains SOAP-shaped in Google Ad Manager. The SOAP tools
therefore cover the production trafficking surface through a typed operation
allowlist instead of pretending those workflows exist in REST.

SOAP operations currently exposed:

- `OrderService`: `create_orders`, `get_orders_by_statement`,
  `perform_order_action`, `update_orders`
- `LineItemService`: `create_line_items`, `get_line_items_by_statement`,
  `perform_line_item_action`, `update_line_items`
- `CreativeService`: `create_creatives`, `get_creatives_by_statement`,
  `perform_creative_action`, `update_creatives`
- `LineItemCreativeAssociationService`:
  `create_line_item_creative_associations`,
  `get_line_item_creative_associations_by_statement`,
  `get_line_item_creative_association_preview_url`,
  `get_line_item_creative_association_native_style_preview_urls`,
  `perform_line_item_creative_association_action`,
  `update_line_item_creative_associations`
- `ForecastService`: `get_availability_forecast`,
  `get_availability_forecast_by_id`, `get_delivery_forecast`,
  `get_delivery_forecast_by_ids`, `get_traffic_data`

The SOAP request shape is intentionally thin:

- choose an allowlisted `operation`;
- provide `payload_xml` as the inner operation XML only;
- do not include a SOAP envelope, SOAP header, OAuth token, request header,
  XML declaration, DTD, or entity declaration;
- review the generated envelope returned by `gam_soap_trafficking_plan`;
- call `gam_soap_trafficking_apply` with the exact matching confirmation
  token.

All live SOAP calls require
`GOOGLE_AD_MANAGER_MCP_SCOPE=https://www.googleapis.com/auth/admanager`.
This includes non-mutating forecast/read operations because Google Ad
Manager's legacy SOAP API does not accept the newer read-only Ad Manager
scope. Mutating SOAP operations additionally require
`GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`, `expected_impact`, and
`rollback_note`.

Example SOAP line-item lookup plan:

```json
{
  "request": {
    "network_code": "1234567",
    "operation": "get_line_items_by_statement",
    "payload_xml": "<filterStatement><query>WHERE id = 987654</query></filterStatement>",
    "reason": "Read the current line item before a trafficking change."
  }
}
```

Example SOAP line-item pause plan:

```json
{
  "request": {
    "network_code": "1234567",
    "operation": "perform_line_item_action",
    "payload_xml": "<lineItemAction xsi:type=\"PauseLineItems\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"/><filterStatement><query>WHERE id = 987654</query></filterStatement>",
    "reason": "Pause the line item while the campaign owner reviews delivery.",
    "expected_impact": "Stops serving for line item 987654 until resumed.",
    "rollback_note": "Run perform_line_item_action with ResumeLineItems for the same id.",
    "idempotency_key": "ticket-123"
  }
}
```

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
