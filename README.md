# google-ad-manager-mcp

`google-ad-manager-mcp` is a public Rust stdio MCP server for Google Ad
Manager workflows. It is built on `mcp-toolkit-rs`, the official Google Ad
Manager API (Beta), and the official Google Ad Manager SOAP API for classic
trafficking operations that are not yet available through REST.

The current alpha focuses on a small useful surface:

- inspect Google Ad Manager credential readiness without exposing secrets;
- discover accessible Ad Manager networks;
- list curated network collections:
  - ad units
  - orders
  - line items
  - saved reports
- run saved reports and fetch paginated result rows;
- plan allowlisted REST write operations with no upstream mutation;
- apply allowlisted REST writes only when an operator explicitly enables write
  mode, uses the manage scope, and passes the matching confirmation token;
- build safe inner SOAP `payload_xml` fragments for common trafficking
  templates without calling upstream;
- plan and run allowlisted SOAP trafficking operations for orders, line items,
  creatives, line-item creative associations, preview URLs, and forecasts;
- preview and apply descendant-safe ad-unit exclusions to a readback-proven yield group
  with confirmation-token and post-apply readback gates;
- load catalog/report pages and parsed SOAP line-item delivery readbacks into a
  bounded local DuckDB scratchpad for read-only analysis and evidence bundles.

The server intentionally does not expose a generic HTTP/SOAP proxy, arbitrary
query surface, or default live write operations.

## Documentation

- [Getting started](docs/GETTING_STARTED.md)
- [Tool guide](docs/TOOL_GUIDE.md)
- [Security model](docs/SECURITY_MODEL.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Decision 0001: Beta REST read-only first](docs/decision-0001-beta-rest-read-only.md)
- [Decision 0002: Guarded REST writes before SOAP trafficking](docs/decision-0002-guarded-rest-writes-before-soap-trafficking.md)
- [Decision 0003: Guarded SOAP trafficking adapter](docs/decision-0003-guarded-soap-trafficking-adapter.md)
- [Decision 0004: Exchange and protection proof surface](docs/decision-0004-exchange-protection-proof.md)
- [Decision 0005: Ad-unit dependency proof](docs/decision-0005-ad-unit-dependency-proof.md)
- [Releasing](docs/RELEASING.md)

## Install

The lowest-friction install path today is:

```bash
cargo install --locked --git https://github.com/sednalabs/google-ad-manager-mcp google-ad-manager-mcp
```

For a pinned tagged source install:

```bash
cargo install --locked --git https://github.com/sednalabs/google-ad-manager-mcp --tag v0.1.1-alpha.0 google-ad-manager-mcp
```

The repository also publishes GitHub-hosted binary bundles through the release
workflow and a Linux artifact on `main` through the `Linux Artifact` workflow.
Those hosted artifacts are useful when you want a pinned binary plus SHA256
manifests and a Sigstore verification bundle from hosted compute rather than a
local `cargo install`.

Pull-request `rust-baseline` runs also upload a `rustfmt-patch` diagnostic
artifact. This keeps formatting repairs reproducible when validation is
intentionally performed on hosted compute.

## First Run

The server exposes setup tools that do not return secrets:

- `gam_get_started`
- `gam_auth_status`
- `gam_auth_login_command`

For local use, the easiest path is:

```bash
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID>
```

The helper uses Google Application Default Credentials in a
Google-Ad-Manager-specific gcloud config directory, requests the required
`cloud-platform` ADC scope plus the read-only Ad Manager scope, sets the ADC
quota project when provided, and verifies access with `networks.list`. Keeping
a server-specific ADC file prevents a login for another Google MCP from
replacing this server's refresh token or scope grant.

You can inspect or script auth without starting an MCP session:

```bash
google-ad-manager-mcp auth command --headless
google-ad-manager-mcp auth status --verify-token
google-ad-manager-mcp auth doctor --verify-token --json
```

Then restart any stdio MCP client that keeps a long-lived child process and call:

```text
gam_auth_status { "verify_access": true }
```

After auth is proven:

1. `gam_networks_list`
2. `gam_network_catalog_list`
3. `gam_exchange_protection_probe` when you need exchange/yield/protection
   proof for exact ad units; yield-group exposure separates
   `targeted_exposed` from `targeted_and_excluded`
4. `gam_ad_unit_dependency_probe` when you need read-only dependency proof
   before ad-unit cleanup, archive, or retargeting decisions
5. `gam_report_run`
6. `gam_report_result_rows` when a report result has more pages
7. `gam_trafficking_tool_matrix` before planning writes
8. `gam_rest_write_plan` for dry-run write previews
9. `gam_rest_write_apply` only in explicit operator mode
10. `gam_soap_payload_build` to generate common SOAP payload fragments
11. `gam_soap_trafficking_plan` for order, line-item, creative, LICA, preview,
   and forecast SOAP plans
12. `gam_soap_trafficking_apply` only after reviewing the matching SOAP plan
13. `gam_yield_group_exclusions_preview` when descendant-safe ad-unit exclusions should
   be added to an existing yield group without changing line-item targeting
14. `gam_yield_group_exclusions_apply` only with write mode enabled, the
   manage scope, the exact confirmation token, and post-apply readback proof
15. `gam_scratchpad_open_session` and the `gam_scratchpad_ingest_*` tools when
   you want local SQL analysis or a markdown evidence bundle

## Authentication

The server defaults to the read-only Ad Manager scope:

```text
https://www.googleapis.com/auth/admanager.readonly
```

REST live write apply and every live SOAP call require the manage scope:

```text
https://www.googleapis.com/auth/admanager
```

Dry-run write planning does not require the manage scope because it does not
call an upstream mutation endpoint. Applying a plan requires both the manage
scope and `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`. SOAP forecast/read calls
also require the manage scope because the legacy SOAP API does not accept the
newer Ad Manager read-only scope.

Supported credential sources:

- Server-specific Application Default Credentials from
  `google-ad-manager-mcp auth login`
- Conventional shared Application Default Credentials from
  `gcloud auth application-default login`
- Standard Google credential file via `GOOGLE_APPLICATION_CREDENTIALS`
- Server-specific service account file via
  `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH`
- Server-specific raw service account JSON via
  `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON`

For local user ADC, the runtime prefers the server-specific credential file and
falls back to conventional shared ADC only when that file has not been created
yet. Use `google-ad-manager-mcp auth login` for the low-friction isolated path.

If you already have a service account for the Google Ad Manager SOAP API, the
official Ad Manager Beta docs say you can reuse it after enabling the Ad
Manager API on the Google Cloud project tied to that credential.

For raw `gcloud` use with the same server-specific ADC file, set
`CLOUDSDK_CONFIG` to the server config directory. ADC user credentials need both
scopes:

```bash
CLOUDSDK_CONFIG="$HOME/.config/google-ad-manager-mcp/gcloud" \
  gcloud auth application-default login \
  --scopes=https://www.googleapis.com/auth/cloud-platform,https://www.googleapis.com/auth/admanager.readonly
CLOUDSDK_CONFIG="$HOME/.config/google-ad-manager-mcp/gcloud" \
  gcloud auth application-default set-quota-project <PROJECT_ID>
```

Use `google-ad-manager-mcp auth login --shared-adc` only when you deliberately
want the conventional shared gcloud ADC file for this OS user.

For operator write testing, replace the Ad Manager scope with:

```bash
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> --manage-scope
```

The equivalent raw `gcloud` command uses
`https://www.googleapis.com/auth/admanager` instead of the read-only scope.

Set `GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT=<PROJECT_ID>` in the MCP server
environment when you want the server to send the `x-goog-user-project` header.
The `gcloud auth application-default set-quota-project` command remains useful
for ADC-aware Google tooling. When you use `google-ad-manager-mcp auth login`,
the command is applied to the server-specific ADC file by default.

The server never returns raw access tokens, private keys, refresh tokens, or
whole credential files in tool responses.

## Configuration

| Setting | Default | Purpose |
| --- | --- | --- |
| `GOOGLE_AD_MANAGER_MCP_SCOPE` | `https://www.googleapis.com/auth/admanager.readonly` | OAuth scope requested from Google credentials |
| `GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT` | unset | Optional `x-goog-user-project` header |
| `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH` | unset | Server-specific service-account credential path |
| `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON` | unset | Server-specific raw service-account JSON |
| `GOOGLE_AD_MANAGER_MCP_HTTP_TIMEOUT_MS` | `15000` | Upstream request timeout |
| `GOOGLE_AD_MANAGER_MCP_API_BASE_URL` | `https://admanager.googleapis.com/v1` | Upstream API root |
| `GOOGLE_AD_MANAGER_MCP_SOAP_BASE_URL` | `https://ads.google.com/apis/ads/publisher` | Upstream SOAP API root before version/service |
| `GOOGLE_AD_MANAGER_MCP_WRITE_MODE` | `preview_only` | Write runtime gate: `read_only`, `preview_only`, or `enabled` |
| `GOOGLE_AD_MANAGER_MCP_REPORT_POLL_TIMEOUT_MS` | `300000` | Default report wait timeout |
| `GOOGLE_AD_MANAGER_MCP_REPORT_POLL_INITIAL_INTERVAL_MS` | `5000` | Initial report polling interval |
| `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_SESSION_TTL_SECS` | `900` | Scratchpad session idle TTL |
| `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_SESSIONS` | `64` | Maximum active scratchpad sessions |
| `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_TABLES_PER_SESSION` | `32` | Maximum tables per scratchpad session |
| `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_ROWS_PER_SESSION` | `1000000` | Maximum ingested rows per scratchpad session |
| `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_MEMORY_MB` | `256` | DuckDB memory limit per scratchpad connection |
| `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_QUERY_TIMEOUT_MS` | `15000` | Scratchpad query timeout |
| `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_SQL_BYTES` | `65536` | Maximum SQL payload accepted by scratchpad guardrails |
| `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_ROOT_DIR` | OS temp directory | Optional existing directory for scratchpad databases |

## Tools

- `find_tools`
- `gam_get_started`
- `gam_auth_status`
- `gam_auth_login_command`
- `gam_networks_list`
- `gam_network_catalog_list`
- `gam_exchange_protection_probe`
- `gam_ad_unit_dependency_probe`
- `gam_report_run`
- `gam_report_result_rows`
- `gam_trafficking_tool_matrix`
- `gam_rest_write_plan`
- `gam_rest_write_apply`
- `gam_soap_payload_build`
- `gam_soap_trafficking_plan`
- `gam_soap_trafficking_apply`
- `gam_yield_group_exclusions_preview`
- `gam_yield_group_exclusions_apply`
- `gam_scratchpad_open_session`
- `gam_scratchpad_close_session`
- `gam_scratchpad_list_sessions`
- `gam_scratchpad_list_tables`
- `gam_scratchpad_drop_table`
- `gam_scratchpad_query`
- `gam_scratchpad_ingest_network_catalog`
- `gam_scratchpad_ingest_report_result_rows`
- `gam_scratchpad_ingest_soap_line_items`
- `gam_scratchpad_export_evidence_bundle`

All tool responses use Contract V1 envelopes:

```json
{
  "ok": true,
  "data": {},
  "meta": {
    "elapsed_ms": 12
  }
}
```

## Upstream Scope

This server is intentionally shaped around curated official Google Ad Manager
surfaces rather than broad proxy access. REST beta is used for networks,
catalogs, saved reports, and supported REST writes. The guarded SOAP adapter is
used for classic trafficking workflows that remain SOAP-shaped:

- `OrderService`
- `LineItemService`
- `CreativeService`
- `LineItemCreativeAssociationService`
- `ForecastService`
- `YieldGroupService`

`gam_soap_trafficking_plan` wraps an allowlisted SOAP operation around an inner
payload XML fragment and returns the exact envelope plus a confirmation token.
`gam_soap_trafficking_apply` runs the reviewed envelope only after scope,
runtime, and confirmation checks. Mutating SOAP operations require
`GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`; non-mutating forecast/read SOAP
operations can run without write mode enabled but still need the manage scope
required by the legacy SOAP API.

`gam_yield_group_exclusions_preview` and
`gam_yield_group_exclusions_apply` provide the typed path for descendant-safe
YieldGroupService ad-unit exclusions. They preserve current yield-group
targeting, add or repair only requested `excludedAdUnits` entries with
`includeDescendants=true`, and require post-apply readback before reporting
success.
