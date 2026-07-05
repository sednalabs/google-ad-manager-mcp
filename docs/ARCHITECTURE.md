# Architecture

## Intent

This repository is a small public stdio MCP server over a curated subset of the
Google Ad Manager API (Beta). The design goal is a useful, auditable operator
surface, not an SDK mirror or generic upstream proxy.

## Module map

- `src/config.rs`
  - CLI and runtime settings
  - scope, timeout, quota-project, service-account, auth subcommand, and
    scratchpad configuration
- `src/auth_ux.rs`
  - operator-facing `auth login`, `auth command`, `auth status`, and
    `auth doctor` flows
  - ADC quota-project detection and verification reporting
- `src/client.rs`
  - authenticated Google Ad Manager REST adapter
  - curated collection routing
  - saved report run and result polling helpers
- `src/error.rs`
  - stable error categories and hints
- `src/contract.rs`
  - Contract V1 response envelopes
  - secret-text redaction
  - scratchpad error envelopes
- `src/tool_surface.rs`
  - `ToolInventory` metadata for `find_tools`
- `src/tools.rs`
  - MCP tool argument schemas and implementations
- `src/lib.rs`
  - server assembly, scratchpad manager setup, and exported tool snapshot
    helpers
- `src/main.rs`
  - stdio entrypoint and `--print-tools` / `--print-tool-schema`

## Upstream boundary

The public v1 uses only the official Ad Manager Beta REST surface:

- `networks.list`
- `networks/<code>/adUnits.list`
- `networks/<code>/orders.list`
- `networks/<code>/lineItems.list`
- `networks/<code>/reports.list`
- `reports.run`
- `reports.results.fetchRows`
- `networks.operations.reports.runs.get`

No SOAP adapter is included in this release. If a future read-only gap forces a
SOAP fallback, it should land behind a separate adapter boundary instead of
leaking SOAP semantics into the current tool surface.

## Tool design

The initial first-class tool set is:

1. `gam_get_started`
2. `gam_auth_status`
3. `gam_auth_login_command`
4. `gam_networks_list`
5. `gam_network_catalog_list`
6. `gam_report_run`
7. `gam_report_result_rows`
8. `gam_scratchpad_open_session`
9. `gam_scratchpad_close_session`
10. `gam_scratchpad_list_sessions`
11. `gam_scratchpad_list_tables`
12. `gam_scratchpad_drop_table`
13. `gam_scratchpad_query`
14. `gam_scratchpad_ingest_network_catalog`
15. `gam_scratchpad_ingest_report_result_rows`
16. `gam_scratchpad_export_evidence_bundle`

`find_tools` is also exposed for deferred-loading and `tool_search` clients.

The deliberately grouped tool is `gam_network_catalog_list`. It keeps the
surface compact while still covering the four network collections that matter
most for a first useful release:

- ad units
- orders
- line items
- saved reports

## Auth UX

The binary exposes auth subcommands before stdio startup:

- `auth login`
- `auth command`
- `auth status`
- `auth doctor`

These commands are deliberately wrappers around Google Application Default
Credentials rather than a custom token store. The login command requests both
the `cloud-platform` ADC scope required by `gcloud` and the configured Ad
Manager scope. Verification uses `networks.list` with a small page size, which
proves token minting, API enablement, quota-project behavior, and Ad Manager
network visibility without exposing tokens.

## Scratchpad Boundary

Scratchpad support is provided by `mcp-toolkit-scratchpad`, not by local
server-specific DuckDB lifecycle code. The Ad Manager MCP only maps upstream
catalog/report rows into stable scratchpad columns and keeps the full upstream
row as `upstream_json` so beta API field drift does not destroy evidence.

The scratchpad tools are analysis and evidence helpers. They do not mutate
Google Ad Manager and do not broaden the upstream API surface.
