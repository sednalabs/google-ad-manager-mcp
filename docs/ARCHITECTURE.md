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
  - write runtime mode for preview/apply gating
- `src/auth_ux.rs`
  - operator-facing `auth login`, `auth command`, `auth status`, and
    `auth doctor` flows
  - ADC quota-project detection and verification reporting
- `src/client.rs`
  - authenticated Google Ad Manager REST adapter
  - authenticated Google Ad Manager SOAP envelope and request adapter
  - curated collection routing
  - saved report run and result polling helpers
  - allowlisted REST write planning and execution helpers
  - allowlisted SOAP trafficking planning and execution helpers
- `src/error.rs`
  - stable error categories and hints
- `src/contract.rs`
  - Contract V1 response envelopes
  - secret-text redaction
  - scratchpad error envelopes
- `src/tool_surface.rs`
  - `ToolInventory` metadata for `find_tools`
- `src/tools.rs`
  - MCP tool argument schemas, SOAP payload template rendering, and
    implementations
- `src/lib.rs`
  - server assembly, scratchpad manager setup, and exported tool snapshot
    helpers
- `src/main.rs`
  - stdio entrypoint and `--print-tools` / `--print-tool-schema`

## Upstream boundary

The public v1 uses curated official Ad Manager surfaces:

REST beta:

- `networks.list`
- `networks/<code>/adUnits.list`
- `networks/<code>/orders.list`
- `networks/<code>/lineItems.list`
- `networks/<code>/reports.list`
- `reports.run`
- `reports.results.fetchRows`
- `networks.operations.reports.runs.get`
- allowlisted REST write methods for ad units, placements, saved reports,
  labels, teams, contacts, custom fields, custom targeting keys, applications,
  sites, ad spots, and related batch state actions

SOAP v202605 by default:

- `OrderService`
- `LineItemService`
- `CreativeService`
- `LineItemCreativeAssociationService`
- `ForecastService`
- `YieldGroupService`

The SOAP adapter is intentionally typed by operation but thin on object
modeling. It accepts inner operation XML fragments, wraps them in a
server-owned SOAP envelope and `RequestHeader`, authenticates with the existing
Google credential chain, and returns bounded raw XML plus request metadata.
It does not expose arbitrary SOAP services, arbitrary SOAP methods, caller
supplied envelopes, caller supplied headers, or bearer-token handling.

Common SOAP payload fragments are rendered by `gam_soap_payload_build`. That
helper does not call upstream and does not broaden the SOAP boundary; it only
turns validated template inputs such as line item IDs, order IDs, creative IDs,
and safe name fragments into inner `payload_xml` for the allowlisted SOAP
operations.

## Tool design

The initial first-class tool set is:

1. `gam_get_started`
2. `gam_auth_status`
3. `gam_auth_login_command`
4. `gam_networks_list`
5. `gam_network_catalog_list`
6. `gam_report_run`
7. `gam_report_result_rows`
8. `gam_trafficking_tool_matrix`
9. `gam_rest_write_plan`
10. `gam_rest_write_apply`
11. `gam_soap_payload_build`
12. `gam_soap_trafficking_plan`
13. `gam_soap_trafficking_apply`
14. `gam_yield_group_exclusions_preview`
15. `gam_yield_group_exclusions_apply`
16. `gam_scratchpad_open_session`
17. `gam_scratchpad_close_session`
18. `gam_scratchpad_list_sessions`
19. `gam_scratchpad_list_tables`
20. `gam_scratchpad_drop_table`
21. `gam_scratchpad_query`
22. `gam_scratchpad_ingest_network_catalog`
23. `gam_scratchpad_ingest_report_result_rows`
24. `gam_scratchpad_ingest_soap_line_items`
25. `gam_scratchpad_export_evidence_bundle`

`find_tools` is also exposed for deferred-loading and `tool_search` clients.

The deliberately grouped tool is `gam_network_catalog_list`. It keeps the
surface compact while still covering the curated network collections that
matter most for a first useful release and the exchange-proof workflow:

- ad units
- orders
- line items
- private auctions
- private auction deals
- saved reports

`gam_exchange_protection_probe` layers a product-neutral proof workflow over
those catalog reads and SOAP YieldGroupService reads. It reports partial proof
states for capped, blocked, or unsupported protection surfaces instead of
turning missing API coverage into a clean result.

`gam_yield_group_exclusions_preview` and
`gam_yield_group_exclusions_apply` are the typed mutation path for descendant-safe
YieldGroupService ad-unit exclusions. They read the current yield group,
preserve the existing yield-group targeting object, add or repair only requested
`excludedAdUnits` entries with `includeDescendants=true`, and require post-apply
readback before reporting an applied state. They deliberately do not make
`updateYieldGroups` a generic SOAP operation.

The deliberately grouped write tools are `gam_rest_write_plan` and
`gam_rest_write_apply`. They cover the current REST beta write surface through
typed allowlists rather than exposing arbitrary HTTP. Planning is a no-mutation
preview; apply requires explicit runtime enablement, the manage OAuth scope, a
matching confirmation token, operator context, and post-apply readback where the
upstream response exposes a resource name.

The deliberately grouped SOAP tools are `gam_soap_trafficking_plan` and
`gam_soap_trafficking_apply`. They cover classic trafficking and forecast
workflows through an operation enum:

- orders
- line items
- creatives
- line-item creative associations
- preview URLs
- forecasts

SOAP plans are no-mutation previews. SOAP apply always requires the full Ad
Manager manage scope because the legacy SOAP API does not support the newer
read-only scope. Mutating SOAP apply also requires explicit write-mode
enablement and operator context.

`gam_soap_payload_build` is a no-mutation helper that sits before those tools.
It renders bounded, validated templates for common read, line-item action,
LICA, preview, and forecast-by-ID payloads. Full order, line item, and creative
object construction remains intentionally outside the first helper slice.

## Auth UX

The binary exposes auth subcommands before stdio startup:

- `auth login`
- `auth command`
- `auth status`
- `auth doctor`

These commands are deliberately wrappers around Google Application Default
Credentials rather than a custom token store. By default they run gcloud with a
Google-Ad-Manager-specific `CLOUDSDK_CONFIG` directory so this server's user ADC
file is separate from other Google MCP servers under the same OS account. The
login command requests both the `cloud-platform` ADC scope required by `gcloud`
and the configured Ad Manager scope. The `--manage-scope` flag switches the login command to
`https://www.googleapis.com/auth/admanager` for operator-approved write apply
testing without asking users to remember the raw scope string. Verification
uses `networks.list` with a small page size, which proves token minting, API
enablement, quota-project behavior, and Ad Manager network visibility without
exposing tokens.

## Scratchpad Boundary

Scratchpad support is provided by `mcp-toolkit-scratchpad`, not by local
server-specific DuckDB lifecycle code. The Ad Manager MCP maps upstream
catalog/report rows and parsed SOAP line-item delivery readbacks into stable
scratchpad columns, while keeping the full upstream row or result XML for
private local evidence so API field drift does not destroy evidence.

The scratchpad tools are analysis and evidence helpers. They do not mutate
Google Ad Manager and do not broaden the upstream API surface.
