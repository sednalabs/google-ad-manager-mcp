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
