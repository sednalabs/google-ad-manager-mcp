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
- `src/evidence.rs`
  - neutral evidence receipts, fingerprints, and Contract/RMCP byte limits
- `src/ad_unit_retirement.rs` and `src/ad_unit_retirement/`
  - staged exact-identity, hierarchy, and freshness-bound evidence assessment
- `src/probe_projection/`
  - semantic-preserving compact exchange/dependency proof projections
  - typed omission accounting and bounded proof error fallbacks
- `src/tool_surface.rs`
  - `ToolInventory` metadata for `find_tools`
- `src/tool_discovery.rs`
  - GAM vocabulary, workflow-companion DAGs, bounded recovery, and local-state
    access-class guidance over toolkit-ranked results
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

The report adapter treats long-running operation handles as bound capabilities,
not free strings. `reports.run` sends a genuinely empty request body. A
definitive 4xx rejection is an upstream API failure. Once the non-idempotent POST
may otherwise have been dispatched, transport, body-read, response-bound,
plausible server-status, JSON, or handoff failure is uncertain and not
automatically replay-safe. A known requested report can supply an omitted
`metadata.report`, while inconsistent present metadata is rejected; every poll
must echo the requested operation name and the final `reportResult` must belong
to that same report. The validated POST observation is the first poll
observation, avoiding an unnecessary GET when it is already complete. Invalid
`done`/`error`/`response` unions fail closed. Operation reads are capped at 64
KiB and projected to the documented name/metadata/done/error/response fields.
Result-page reads are
capped at 512 KiB and page size 1,000, validate documented row, token, and count
types, then pass model-visible and complete RMCP result guards. Poll controls
require a 5-to-30-second initial interval and at most 24 hours, with bounded
backoff. An absolute deadline bounds every in-flight GET and sleep. Each
continuation carries the optional expected report identity; malformed poll
observations preserve the last valid observation and remain safely GET-resumable.
Deterministic result-size failures retain bounded operation/report/result/page
handles and return a non-executable smaller-page adjustment.

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

The two high-level proof tools return their native Contract V1 shape when it
fits the model-visible and RMCP limits. Oversized results cross a separate
projection boundary: authoritative semantics are extracted before and after
projection, root decisions, certainty, and dependency proof flags are
re-derived from retained source surfaces, omissions are recorded explicitly,
and an eligible receipt is rebound to the returned compact fingerprint. Oversized errors retain their stable
classification and a redacted UTF-8-safe message prefix. A projection that
drifts semantically or remains oversized is replaced by a bounded contract
error.

## Tool design

The initial first-class tool set is:

1. `gam_get_started`
2. `gam_auth_status`
3. `gam_auth_login_command`
4. `gam_networks_list`
5. `gam_network_catalog_list`
6. `gam_report_run`
7. `gam_report_operation_poll`
8. `gam_report_result_rows`
9. `gam_trafficking_tool_matrix`
10. `gam_rest_write_plan`
11. `gam_rest_write_apply`
12. `gam_soap_payload_build`
13. `gam_soap_trafficking_plan`
14. `gam_soap_trafficking_apply`
15. `gam_yield_group_exclusions_preview`
16. `gam_yield_group_exclusions_apply`
17. `gam_scratchpad_open_session`
18. `gam_scratchpad_close_session`
19. `gam_scratchpad_list_sessions`
20. `gam_scratchpad_list_tables`
21. `gam_scratchpad_drop_table`
22. `gam_scratchpad_query`
23. `gam_scratchpad_ingest_network_catalog`
24. `gam_scratchpad_ingest_report_result_rows`
25. `gam_scratchpad_ingest_soap_line_items`
26. `gam_scratchpad_export_evidence_bundle`

`find_tools` is also exposed for deferred-loading and `tool_search` clients. It
uses the toolkit's additive ranked search path and compact serializer by
default. The provider layer adds only domain workflow relationships: REST plan
before apply; a report cold-start and asynchronous continuation chain from
network discovery through report catalog, run, operation poll, and rows;
optional SOAP payload builder before SOAP plan; SOAP plan before SOAP apply;
yield-group preview before apply; and filter-validated bounded empty-result
recovery. Destructive ad-unit archive/deactivate/retirement intent also adds
network and catalogue identity, dependency-probe, and retirement-assessment
predecessors before the REST plan.
The provider composes the complete applicable dependency graph, rejects cycles,
then computes callable transitive prerequisites in deterministic topological
order and models non-callable conditions separately. Direct
report-operation discovery keeps the original run only as a non-callable
cold-start condition: it is present with the reason not to rerun an existing
operation, but is absent from allowed tools and schemas. Direct SOAP plan
discovery still adds its optional builder as callable guidance, and direct SOAP
apply discovery adds builder then plan. Every reachable edge is emitted
even when its predecessor is already a semantic result;
`tool_already_selected` distinguishes that case from a missing predecessor.
Only missing callable predecessor names are injected into allowed tools and schemas,
with name deduplication separate from edge emission. Match counts remain about
semantic inventory results; companion and recovery records are separate OpenAI
extra results so guidance does not inflate search counts. Full schemas and
hosted-client metadata are emitted only when `include_schema=true`, and only
when the complete direct-plus-companion selection contains at most five tools.
Report-run starts use a non-read-only toolkit risk posture because they create
an upstream job. Discovery exposes a direct start only when `read_only=false`
and the bounded toolkit query expresses explicit new-run intent.
Existing-operation continuation context replaces any ranked start with a non-callable
condition record and, when the active filter would exclude it, exposes the
GET-only poll tool as a callable safe alternative to prevent replay.
Content-only clients receive a
bounded actionable JSON projection with
ordered ranked direct matches, descriptions and risk posture, allowed tools,
callable/condition workflow edges and reasons, modern workflow fields, recovery,
and schema names; complete ranked records and schemas remain in structured
content.

Recovery candidates are a typed static catalog separate from representative
ranking probes. They cover every current tool group but deliberately omit the
upstream-job-creating report-start class. Candidate examples
are evaluated through the same strict list inventory and toolkit-normalized
active exact `group`/`read_only` filters as the failed search, always at
`limit=1`; only an expected rank-one match is serialized. The response keeps
`retry.example_queries` as a string array, adds `active_filter`, and marks the
examples as validated under it. Invalid groups can have no examples while
`available_groups` still offers alternatives from the complete strict
list-visible inventory under the active `read_only` filter. Dynamic group,
query, and local-state tool lists are capped and report total, returned, and
truncated counts. Recovery never
relaxes an upstream-safety filter merely to force a match. A clear, exact
scratchpad request can instead expose the separately scoped local-state option
described below; fail-closed searches cannot.
Omitted or explicitly null `read_only` is normalized to `true` at the provider
boundary. Ambiguous, truncated, negative, or exclusion-bearing intent emits no
provider canned guidance, and mutating candidates are eligible only under an
explicit `read_only=false` filter. For stateful workflows, scratchpad recovery
orders the executable session entry before ingestion. Report-run starts are not
canned retry examples; existing-operation polling and completed-result retrieval
remain executable discovery paths, and continuation context suppresses duplicate
starts.
An explicit scratchpad-group search under `read_only=true` remains empty because
all current scratchpad calls can touch local session state. Its recovery carries
a separate `local_state_alternatives` contract. A query with strong scratchpad
intent also emits that local-state option as a standalone `filter_alternative`
when weaker read-only matches exist, so ranking noise cannot hide it. Both forms
scope an explicit
`read_only=false` retry to MCP-local state, deny upstream GAM mutation, and
enumerates destructive close/drop tools without weakening the active filter.
Provider-owned access classes distinguish local-only calls, REST reads under
the read-only scope, and the SOAP line-item read that requires manage scope.
The content-only projection retains the exact bounded rediscovery call plus
eligible/destructive counts and access-class risk context.

The provider rejects `limit=0` before inventory search. The public default is
20; larger values flow into the toolkit unchanged so its hard maximum of 100,
`match_summary.result_limit`, and `result_limit_clamped` diagnostics remain
authoritative.
The public projection removes free-form query text and query-derived terms and
returns only presence, recognition, and term counts. Unrecognized or truncated
group text is also omitted. Oversized inputs retain bounded compact responses, report their
input truncation reason, and mark recovery fail-closed. Current full-group
contracts require guided dependency edges and allowed-tool names to remain
intact within the toolkit's 32 KiB compact data budget. The provider emits a
bounded actionable RMCP text projection rather than duplicating full
structured discovery data, then
guards the complete result at 64 KiB and its structured Contract V1 envelope at
48 KiB.

All Contract V1 failure helpers set MCP `isError=true` while retaining their
stable `ok:false` structured envelope. Successes retain `isError=false`.

Companion edges describe a guided sequence, not server-side invocation proof.
Each record exposes `required_for_guided_sequence` and
`server_call_enforced:false`. The legacy `required` field is preserved as an
equal compatibility alias with
`required_semantics:"guided_sequence_compatibility_alias"`; clients should use
the new fields. No companion record claims that the builder, plan, or preview
call occurred.

Verification authority remains tool-specific. REST apply independently
revalidates its request and token and retains runtime, scope, and confirmation
gates; configured readback is attempted where available but is not a universal
success gate. Generic SOAP apply retains request, token, runtime, scope, and
confirmation checks and requires follow-up verification. Typed yield apply
retains those gates and requires descendant-safe post-apply readback.

The deliberately grouped tool is `gam_network_catalog_list`. It keeps the
surface compact while still covering the curated network collections that
matter most for a first useful release and the exchange-proof workflow:

- ad units
- orders
- line items
- placements
- private auctions
- private auction deals
- saved reports

The public `CatalogCollection` enum is non-exhaustive. New curated collections
may be added while the crate remains alpha, and downstream callers should keep a
fallback arm for exhaustive matches.

`gam_exchange_protection_probe` layers a product-neutral proof workflow over
those catalog reads and SOAP YieldGroupService reads. It reports partial proof
states for capped, blocked, or unsupported protection surfaces instead of
turning missing API coverage into a clean result.

`gam_ad_unit_dependency_probe` layers a read-only dependency workflow over REST
ad-unit and placement rows plus SOAP LineItemService reads. It classifies exact,
ancestor-descendant, placement, root/network, and excluded targeting evidence,
and returns capped or incomplete proof flags rather than an archive/deactivate
decision.

`gam_ad_unit_retirement_assessment` is deliberately staged. The current
implementation accepts one to ten canonical positive ad-unit ids, calls exact
REST `adUnits.get` reads, and returns compact current identity plus stable
fingerprints. It then performs a minimal-field, bounded, byte-capped REST
`adUnits.list` scan, verifies numeric resource-id order, reconciles documented
root-inclusive and observed root-omitted parent paths against the complete
catalog chain plus exact network/effective-root reads, reconciles child flags
for every catalog row, reports external descendants, and returns a
deterministic child-first target order. Malformed pages, pagination drift,
cross-network paths, catalog gaps, or caps fail closed while already-observed
positive descendant blockers remain visible. It then grades at most one
caller-supplied receipt for each dependency, delivery, exchange/protection,
site-contract, and telemetry surface. Receipt conclusions are bound to the
exact network and target set, a supported source contract, an opaque result
hash, observation time, TTL, and, for delivery or telemetry, a recent window of
at least 30 days. Exchange/protection clear proof additionally requires a
recorded manual GAM UI review. Receipt provenance remains
`caller_supplied_unverified`. Confirmed blockers outrank incomplete evidence;
otherwise the decision is either incomplete or
`evidence_complete_operator_review_required`. Even that strongest result is a
read-only recommendation: it never verifies operator identity, authorizes, or
applies a GAM mutation.

`gam_yield_group_exclusions_preview` and
`gam_yield_group_exclusions_apply` are the typed mutation path for descendant-safe
YieldGroupService ad-unit exclusions. They read the current yield group,
preserve the existing yield-group targeting object, add or repair only requested
`excludedAdUnits` entries with `includeDescendants=true`, and require post-apply
readback before reporting an applied state. They deliberately do not make
`updateYieldGroups` a generic SOAP operation. The preview receipt serializes the
exact schema-complete request for apply, and its confirmation fingerprint binds
all mutation and approval-context fields plus the current readback and generated
update fingerprints.

The deliberately grouped write tools are `gam_rest_write_plan` and
`gam_rest_write_apply`. They cover the current REST beta write surface through
typed allowlists rather than exposing arbitrary HTTP. Planning is a no-mutation
preview; apply requires explicit runtime enablement, the manage OAuth scope, a
matching confirmation token, and operator context. When the upstream response
exposes a resource name, configured readback is attempted but is not a universal
success gate.

The deliberately grouped SOAP tools are `gam_soap_trafficking_plan` and
`gam_soap_trafficking_apply`. They cover classic trafficking and forecast
workflows through an operation enum:

- orders
- line items
- creatives
- line-item creative associations
- preview URLs
- forecasts

SOAP plans are no-mutation previews. Discovery guides callers through optional
builder, plan, then apply, but it does not enforce or prove those calls. SOAP
apply independently validates the exact request and token, always requires the
full Ad Manager manage scope because the legacy SOAP API does not support the
newer read-only scope, retains runtime and confirmation gates, and requires
follow-up verification rather than claiming universal readback. Mutating SOAP
apply also requires explicit write-mode enablement and operator context.

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
