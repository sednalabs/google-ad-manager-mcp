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
- bind one to ten exact canonical ad-unit ids to current REST identity and
  hierarchy, then grade exact-target, freshness-bound external evidence through
  a read-only retirement assessment that cannot recommend or apply a mutation;
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
- [Decision 0006: Evidence-graded ad-unit retirement assessment](docs/decision-0006-staged-ad-unit-retirement-assessment.md)
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
5. `gam_ad_unit_retirement_assessment` to prove exact current identity and a
   bounded, numeric-id-ordered hierarchy/descendant scan, grade exact-target
   evidence receipts, and return a conservative recommendation that still
   requires explicit operator review and never authorizes a mutation
6. `gam_network_catalog_list` with `collection="reports"` to obtain the saved
   report id
7. `gam_report_run`
8. `gam_report_operation_poll` only when the run returned an asynchronous
   operation
9. `gam_report_result_rows` when a completed report result has more pages
10. `gam_trafficking_tool_matrix` before planning writes
11. `gam_rest_write_plan` for dry-run write previews
12. `gam_rest_write_apply` only in explicit operator mode
13. `gam_soap_payload_build` to generate common SOAP payload fragments
14. `gam_soap_trafficking_plan` for order, line-item, creative, LICA, preview,
   and forecast SOAP plans
15. `gam_soap_trafficking_apply` only after reviewing the matching SOAP plan
16. `gam_yield_group_exclusions_preview` when descendant-safe ad-unit exclusions should
   be added to an existing yield group without changing line-item targeting
17. `gam_yield_group_exclusions_apply` only with write mode enabled, the
   manage scope, the exact confirmation token, and post-apply readback proof
18. `gam_scratchpad_open_session` and the `gam_scratchpad_ingest_*` tools when
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
| `GOOGLE_AD_MANAGER_MCP_REPORT_POLL_TIMEOUT_MS` | `300000` | Default report wait timeout; startup rejects values outside 1-86400000 ms |
| `GOOGLE_AD_MANAGER_MCP_REPORT_POLL_INITIAL_INTERVAL_MS` | `5000` | Initial report polling interval, clamped to 5000-30000 ms |
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
- `gam_ad_unit_retirement_assessment`
- `gam_report_run`
- `gam_report_operation_poll`
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

Failed tool executions keep the same `ok:false` structured envelope and set the
MCP `isError` signal to `true`, so clients do not have to infer failure solely
from nested JSON.

`find_tools` uses deterministic ranked natural-language discovery. Its default
response is compact, schema-free, capped by the toolkit's 32 KiB selection
budget, and includes match-completeness metadata. It adds guided dependency
edges without changing semantic match counts: REST plan before apply, optional
SOAP payload builder before SOAP plan, SOAP plan before SOAP apply, and yield
preview before yield apply. SOAP prerequisites are transitive, so direct SOAP
apply discovery adds the builder followed by the plan. Empty searches return
available groups and bounded retry examples without relaxing `read_only`. Group,
example-query, and local-state tool lists report total, returned, and truncated
counts so catalogue growth cannot silently erase recovery. Each
example is retained only when the expected tool ranks first at `limit=1` under
the same strict inventory, toolkit-normalized exact `group`, and `read_only`
filter. Recovery keeps the compatible string array at `retry.example_queries`,
reports `active_filter`, and marks
`retry.example_queries_validated_under_active_filter=true`; an invalid group can
therefore return no examples while still listing alternatives from the complete
strict list-visible inventory under the active `read_only` filter.
Ambiguous, truncated, negative, or exclusion-bearing intent marks provider
recovery fail-closed and emits no canned positive workflow or local-state
guidance. Mutating examples are available only when the caller explicitly sets
`read_only=false`. Stateful scratchpad recovery begins with session opening
before ingestion. Starting a report creates an upstream job, so
`gam_report_run` is registered truthfully as non-read-only and is callable from
discovery only when the bounded toolkit query places an explicit start, run,
launch, or execute action before a report object and the caller sets
`read_only=false`. The action must follow a bounded directive prefix. Only
recognized report-object modifiers may occur between that action and object,
and any tail must be empty or a bounded continuation of the newly started run;
planning, explanatory, negative, or unrelated language fails closed. Bare,
latest, current, and reverse `run of/for ... report` noun phrases expose only
the GET-based existing-operation continuation. An explicit report operation or
run identity takes precedence only when it is locally bound to the report
clause; an identity label without a value cannot override a clear new start.
Generic non-report operation references cannot inject the report poll tool.
Report start is not a canned retry example.

Report runs send the provider-required empty POST body. Definitive 4xx run
rejections are normal upstream API failures. Once the POST may otherwise have
been dispatched, transport, response-read, size-limit, plausible server-status,
JSON, and handoff failures are non-replay-safe uncertain handoffs; agents must
not start a replacement run automatically. Successful runs and subsequent polls
bind the requested report, returned operation name, optional `metadata.report`,
and final `reportResult` to one network/report identity. A known expected report
can supply omitted metadata, but inconsistent present metadata is rejected. The
validated POST observation seeds polling, so an already-complete handoff needs no
extra GET. Poll continuations preserve the optional expected report name and use
GET only. Invalid long-running-operation result unions fail closed.
Operation bodies are read through a 64 KiB cap and projected to documented
fields; result pages are read through a 512 KiB cap, use a maximum page size of
1,000, validate the documented object/row/page-token/count shapes, and pass
complete-result size guards. Direct row-fetch failures preserve the exact
bounded result and page-size handles. Opaque page tokens are not duplicated into
error output; the receipt records bounded token context and requires the caller
to retain the original request for an exact retry. Only transport, 408, 429,
and 5xx failures expose a GET continuation; permanent or malformed failures
require remediation without unchanged replay. Poll timeouts are between 1 ms and 24 hours;
initial intervals are between 5 and 30 seconds with bounded exponential backoff.
The absolute deadline covers in-flight GETs as well as sleeps. A timed-out
continuation increases the bounded timeout instead of replaying the expired
value. Contract-invalid poll observations preserve the last valid observation
and a safely resumable GET continuation. A definitive poll-time 4xx other than
408 or 429 instead returns remediation-required detail without an executable
continuation and without claiming that the operation itself is terminal.
A deterministic result-size failure
returns bounded operation, report, result, and page context plus a non-executable
smaller-page adjustment with `automatic_replay_safe=false`; it never repeats the
same oversized page request. If
page size 1 still exceeds the bound, reduce the saved report dimensions or
filters before starting a new run.

`limit` defaults to 20 and must be at least 1. Values above 100 are passed to
the toolkit, which clamps `match_summary.result_limit` to 100 and reports
`result_limit_clamped`. Set `include_schema=true` only after narrowing the result
set when full tool schemas are required. Schema expansion is limited to five
selected direct-plus-companion tools and fails closed above that limit.
Free-form query text, query terms, and unrecognized or truncated group text are never
returned. `request_summary` keeps only presence, recognition, and term-count
diagnostics. The compact selection remains within the toolkit's 32 KiB data
budget; the complete RMCP result includes a bounded actionable JSON text
projection for content-only clients, is capped at 64 KiB, and caps its
structured Contract V1 envelope at 48 KiB. Requested schemas remain in
`structuredContent.data`.
Omit `read_only`, set it to `null`, or set `read_only=true` to search only non-mutating execution
paths, including plans, previews, and no-mutation proof
reads. Every current scratchpad tool is excluded because the pinned scratchpad
runtime may create, refresh, or prune local session state even during queries,
listings, and evidence export. Set `read_only=false` to search only write-like
or local-state-mutating tools. Use two explicit searches when both mutation
classes are needed. Guided predecessors may still be
added to an apply result's allowed-tool list.
When an explicit `group="scratchpad", read_only=true` search has no matches,
recovery returns `local_state_alternatives` rather than silently relaxing the
filter. A query with strong scratchpad intent under `read_only=true` also emits
the same bounded `filter_alternative` alongside any weak read-only matches, so
unrelated ranking results cannot hide the deliberate local-state continuation.
Fail-closed searches never emit either form. The record and content-only
projection include a bounded executable rediscovery call,
eligible/destructive counts, and access classes. They make the
`read_only=false` retry explicit, limit its scope
to bounded MCP-local scratchpad state, state that it cannot mutate GAM, and
separately identifies destructive local close/drop tools. Its access classes
also distinguish local-only calls, normal GAM REST reads, and the SOAP line-item
ingest that requires the manage scope.
Common operator phrases such as pausing or archiving a line item and
deactivating or archiving an ad unit rank the corresponding non-mutating plan;
they do not opt the caller into apply discovery. Ad-unit archive, deactivate,
retire, and retirement intent also adds the evidence-first network, catalogue,
dependency-probe, and retirement-assessment chain before the REST plan.

Plan and non-noop preview receipts include an `apply_rediscovery` continuation.
It names the exact second `find_tools` call with `read_only=false`, points back
to the reviewed request and confirmation token, and does not authorize a
mutation or bypass existing runtime, scope, context, confirmation, or readback
gates.

Each `workflow_companion` record reports `callable_as_tool`,
`required_for_guided_sequence`, and `server_call_enforced:false`. An existing
report operation keeps `gam_report_run` only as non-callable cold-start
guidance, so it is never injected into `openai_allowed_tools` or schemas and
cannot prompt a duplicate run. Continuation, status, resume, check, poll,
monitor, operation-name, operation-handle, and wait-without-new-run language
under `read_only=false` adds `gam_report_operation_poll` as a callable GET-only
safe alternative only when the bounded query also establishes report context.
Generic operation or waiting language does not cross that boundary. Outside
that continuation context, an explicit
`read_only=false` report-start search returns the tool and schema normally with
its toolkit risk posture. Optional SOAP builder guidance remains callable.
The legacy `required` field remains as an equal
compatibility alias and is labelled by
`required_semantics:"guided_sequence_compatibility_alias"`; clients should use
the new fields. Every reachable dependency edge is emitted even when its
predecessor is already a semantic tool result. `tool_already_selected` makes
that state explicit, while allowed-tool and schema injection add only missing
predecessors and remain deduplicated.

Provider dependencies are composed in full before a deterministic topological
sort. A dependency cycle fails discovery closed. The content-only projection
keeps bounded ranked direct matches in order with descriptions and risk posture,
distinguishes them from companions, and includes the modern
`required_for_guided_sequence`, `server_call_enforced`, and
`tool_already_selected` workflow fields.

Discovery ordering is guidance and does not prove that a builder, plan, or
preview tool was invoked. REST apply revalidates its exact request and token and
retains runtime, scope, and confirmation gates; configured readback is attempted
where available but is not a universal success gate. Generic SOAP apply retains
the same request, token, runtime, scope, and confirmation checks but requires
follow-up verification. Typed yield apply also requires descendant-safe
post-apply readback.

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
success. A non-noop preview includes a schema-complete exact apply request, and
its confirmation fingerprint binds the excluded IDs, API and payload-output
choice, reason, expected impact, rollback note, idempotency key, and current
readback/update fingerprints.
