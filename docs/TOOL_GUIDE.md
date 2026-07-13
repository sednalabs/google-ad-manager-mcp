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
| `gam_network_catalog_list` | List one curated network collection: `ad_units`, `orders`, `line_items`, `placements`, `private_auctions`, `private_auction_deals`, or `reports`. |
| `gam_exchange_protection_probe` | Read-only proof for exact ad-unit exchange/yield/protection exposure, with explicit partial-proof states. |
| `gam_ad_unit_dependency_probe` | Read-only proof for exact ad-unit dependencies across placements and SOAP line-item inventory targeting. |
| `gam_ad_unit_retirement_assessment` | Read-only exact-identity, hierarchy, and freshness-bound evidence assessment for conservative ad-unit retirement review. |
| `gam_report_run` | Run a saved Ad Manager report, optionally wait, and optionally fetch the first result page. |
| `gam_report_operation_poll` | Wait on an existing asynchronous report operation without starting another report run, then optionally fetch the first result page. |
| `gam_report_result_rows` | Fetch rows from a completed report result resource. |
| `gam_trafficking_tool_matrix` | Describe REST-supported writes, SOAP trafficking operations, and remaining ergonomics gaps. |
| `gam_rest_write_plan` | Create a dry-run plan and confirmation token for an allowlisted REST write. |
| `gam_rest_write_apply` | Apply an allowlisted REST write after runtime, scope, and confirmation gates. |
| `gam_soap_payload_build` | Build a safe inner SOAP `payload_xml` fragment for common trafficking templates without calling upstream. |
| `gam_soap_trafficking_plan` | Create a dry-run plan and confirmation token for an allowlisted SOAP trafficking or forecast operation. |
| `gam_soap_trafficking_apply` | Run an allowlisted SOAP trafficking or forecast operation after scope, runtime, and confirmation gates. |
| `gam_yield_group_exclusions_preview` | Read one yield group and preview descendant-safe ad-unit IDs in `excludedAdUnits` without mutating GAM. |
| `gam_yield_group_exclusions_apply` | Apply a previewed yield-group descendant-safe exclusion update after write-mode, confirmation, and readback gates. |
| `gam_scratchpad_open_session` | Open or refresh a bounded local DuckDB scratchpad session. |
| `gam_scratchpad_close_session` | Close a scratchpad session and remove its local database. |
| `gam_scratchpad_list_sessions` | List active scratchpad sessions. |
| `gam_scratchpad_list_tables` | List tables in a scratchpad session. |
| `gam_scratchpad_drop_table` | Drop one scratchpad table. |
| `gam_scratchpad_query` | Run guarded read-only DuckDB SQL against a scratchpad session. |
| `gam_scratchpad_ingest_network_catalog` | Fetch one network catalog page and ingest it into a scratchpad table. |
| `gam_scratchpad_ingest_report_result_rows` | Fetch one report-result page and ingest it into a scratchpad table. |
| `gam_scratchpad_ingest_soap_line_items` | Run a bounded read-only SOAP line-item query and ingest parsed delivery rows into a scratchpad table. |
| `gam_scratchpad_export_evidence_bundle` | Export bounded markdown evidence from scratchpad tables. |

## `find_tools`

`find_tools` accepts a natural-language `query` plus optional exact `group`,
`read_only`, and `limit` filters. Deterministic ranked matching ignores common
conversational words and returns total, returned, limit, and truncation state.
Whitespace-only `group` input is treated as omitted; non-empty invalid or
truncated group input remains fail-closed.
It does not return free-form query text or query-derived terms;
`request_summary` contains only presence, recognition, and term counts. `limit`
defaults to 20 and must be at
least 1. Values above 100 are passed through so the toolkit reports a
`match_summary.result_limit` of 100 and the `result_limit_clamped` reason.

The default `include_schema=false` response omits schemas and hosted-client
metadata and stays within the toolkit's 32 KiB compact-selection budget. Set
`include_schema=true` only after discovery has narrowed the complete direct and
companion selection to at most five tools; broader schema requests fail closed.
Unrecognized or truncated group text is omitted. The structured Contract V1
envelope is capped at 48 KiB and the complete RMCP result at 64 KiB. A bounded
actionable JSON text projection exposes ordered ranked direct matches with
descriptions and risk posture, allowed tools, direct-versus-companion selection,
modern workflow fields, recovery, and schema names to content-only clients
without duplicating the full payload; complete records and requested schemas
remain in `structuredContent.data`.
Omit `read_only`, set it to `null`, or set `read_only=true` to search only non-mutating execution
paths, including plans, previews, and no-mutation proof
reads. Every current scratchpad tool is excluded because the pinned scratchpad
runtime may create, refresh, or prune local session state even during queries,
listings, and evidence export. Set `read_only=false` to search only write-like
or local-state-mutating tools. Use two explicit searches when both mutation
classes are needed. Scratchpad close and drop operations are labelled
destructive, while every other scratchpad tool is labelled mutating, without
implying an upstream GAM write.

For an explicit `group="scratchpad", read_only=true` no-match, recovery adds a
machine-readable `local_state_alternatives` record. Strong scratchpad query
intent under `read_only=true` also adds the same option as a standalone
`filter_alternative` when the ranker returns weaker read-only matches. It does
not change the active filter or add scratchpad tools to
`openai_allowed_tools`; it explains the
explicit `group="scratchpad", read_only=false` retry, sets
`mutation_scope="local_mcp_scratchpad_state"`, states
`upstream_gam_mutation=false`, and notes that local-state writes need no separate
runtime write-mode switch once the caller explicitly selects this filter. It
enumerates every eligible local-state tool and lists close-session/drop-table
separately as destructive. `tool_access_classes` distinguishes local-only
calls, REST reads using the Ad Manager read-only scope, and the SOAP line-item
read that requires the full manage scope.

Discovery adds guided dependency edges:

- `gam_rest_write_plan` precedes `gam_rest_write_apply` and is required for the
  guided sequence;
- `gam_soap_payload_build` precedes `gam_soap_trafficking_plan` and is optional
  in the guided sequence;
- `gam_soap_trafficking_plan` precedes `gam_soap_trafficking_apply` and is
  required for the guided sequence;
- `gam_yield_group_exclusions_preview` precedes
  `gam_yield_group_exclusions_apply` and is required for the guided sequence.

The provider composes the complete applicable dependency graph, rejects cycles,
and then topologically sorts it deterministically. SOAP prerequisites are
transitive: direct plan discovery adds the builder; direct apply discovery adds
builder then plan. Every reachable edge is emitted
even when its predecessor is already a semantic result, with
`tool_already_selected` identifying that case. Only missing predecessor names
are injected into allowed tools and schemas, so injection remains deduplicated
without suppressing dependency edges. Companions are non-mutating and do not
increase semantic match counts.

Every companion record uses `required_for_guided_sequence` and
`server_call_enforced:false`. The legacy `required` field remains as the same
boolean for compatibility and carries
`required_semantics:"guided_sequence_compatibility_alias"`; clients should use
the new fields. This ordering is guidance and does not prove that the companion
tool was invoked.

REST apply independently revalidates its exact request and token and retains
runtime, scope, and confirmation gates; configured readback is attempted where
available but is not a universal success gate. Generic SOAP apply retains those
request, token, runtime, scope, and confirmation checks and requires follow-up
verification. Typed yield apply also requires descendant-safe post-apply
readback. A search that returns no tools adds a bounded `search_recovery` record
with available groups and fail-closed reason codes when relevant. Each recovery
candidate is rerun against the same strict list inventory, toolkit-normalized
exact active `group`/`read_only` filters, and `limit=1`; its query is emitted
only when the expected tool ranks first. `retry.example_queries` remains a
string array, `active_filter` records the applied filters, and
`retry.example_queries_validated_under_active_filter=true` states how the list
was produced. Invalid groups can return no examples while listing alternatives
from the complete strict list-visible inventory under the active `read_only`
filter. Dynamic groups, example queries, and local-state tool lists are capped
and include total, returned, and truncated counts. Recovery never recommends turning off `read_only` merely to produce a
match. The only scoped exception is a clear, exact scratchpad request whose
separate local-state record makes the opt-in, upstream reads, and scope classes
explicit; its content-only projection retains a bounded rediscovery call,
eligible/destructive counts, and access-class context. Fail-closed searches do
not receive it.
Ambiguous, truncated, negative, or exclusion-bearing intent marks provider
recovery fail-closed and returns no canned positive guidance. Mutating examples
require an explicit `read_only=false` search.

Report-run starts are entry-point aware and registered as non-read-only because
starting a saved report creates an upstream job. An explicit
`read_only=false` report-start search exposes `gam_report_run`, its schema, and
its toolkit risk posture only when the bounded toolkit query also expresses
explicit action-object new-run intent: start, run, launch, or execute must
follow a bounded directive prefix and precede a report object with only
recognized report modifiers between them. The remaining query must be empty or
a bounded continuation of that newly started run. Planning, explanatory,
negative, or unrelated language fails closed. Bare, existing-state, and
reverse `run of/for ... report` noun phrases are existing-operation references
and expose only GET-based polling when the complete query is a bounded report
command or reference, including bounded `show me` requests. Explanatory or deliberative framing is not discarded at
`and` or `then`, and collective modal questions remain deliberative rather than
becoming directives. Direct poll matches or workflow companions are filtered
by the same authority decision under every discovery filter. Bounded
completed-result retrieval retains its optional poll predecessor only after the
same identity-coherence check. A query may identify at most one report
operation or run, and every explicit identity must be locally bound to the
report clause before it can authorize report continuation. A runtime-valid
canonical operation resource name counts as that one identity, including opaque
alphanumeric, hyphenated, or underscored operation IDs. Inline, quoted,
backticked, and `operation_name=...` forms use the runtime validator; repeating
the same canonical handle remains one identity, while distinct handles fail
closed. Canonical handles retain local clause ownership, so quoting or
assignment syntax cannot turn an advertiser, campaign, or other non-report
reference into report authority. Unbound identities and
identities related to or premodified by another domain block generic
report-continuation fallback, and a label without a value cannot override a
new-run action object, even when a later tail term makes the complete start
request invalid. Invalid start tails fail closed instead of being reinterpreted
as existing-operation requests. Conjunction-led imperative uses of `report`
are not ownership anchors. Coordinated relation targets are evaluated as a
whole, and any non-report GAM domain in such a target fails closed even when a
report is also named. Generic non-report operation references cannot
inject the report poll tool. Bounded `get`/`retrieve` continuations and
`first N ... rows` requests use the same clause and identity checks. A `to` or
`with` target naming another GAM domain fails closed, while lifecycle phrases
such as `to completion` remain valid. Report starts are representative rank probes but are never
no-match recovery candidates.
Negative query syntax remains fail-closed under the shared inventory policy.
For a nonblocking run, use an affirmative bounded form such as `start a report
and return immediately` or `start a report asynchronously`; `without waiting`
is intentionally not an authority exception.
When discovery identifies an existing
`operation_name` continuation, the same tool is instead emitted as a
non-callable condition record so it cannot prompt a duplicate start.
Continuation, status, resume, check, poll, monitor, operation-name,
operation-handle, and wait-without-new-run language under `read_only=false` also
exposes `gam_report_operation_poll` as a callable GET-only safe alternative in
`openai_allowed_tools` and requested schemas only when the bounded query
establishes report context. Generic operation or waiting language does not.
Existing-operation polling and completed-result retrieval remain executable
discovery paths. Optional SOAP builder guidance remains callable. A broad
scratchpad recovery starts with
`gam_scratchpad_open_session`, then offers ingestion as an existing-session
continuation.

Operator language remains plan-first. Phrases such as pause, resume, or archive
a line item map to `gam_soap_trafficking_plan`; deactivate or archive an ad unit
maps to `gam_rest_write_plan`. Ad-unit archive/deactivate/retirement intent also
adds network lookup, catalogue identity, dependency proof, and retirement
assessment before that plan. Apply tools require `read_only=false` explicitly.

## `gam_network_catalog_list`

`gam_network_catalog_list` is intentionally curated rather than generic. The
`collection` field is an allowlist:

- `ad_units`
- `orders`
- `line_items`
- `placements`
- `private_auctions`
- `private_auction_deals`
- `reports`

This keeps the tool useful without turning the MCP into an arbitrary upstream
endpoint browser.

## `gam_exchange_protection_probe`

`gam_exchange_protection_probe` is the high-level read-only proof tool for
special inventory and exchange/yield concerns. It accepts:

- `network_code`
- exact `ad_unit_codes`
- optional `page_size`
- optional SOAP `api_version`
- optional `include_raw` for bounded YieldGroupService XML

The tool checks:

- exact ad-unit rows, sizes, status, `appliedAdsenseEnabled`, and
  `effectiveAdsenseEnabled`;
- private auctions and private auction deals through REST catalog reads;
- yield groups through SOAP `YieldGroupService.getYieldGroupsByStatement` when
  the configured credential has the Ad Manager manage scope;
- the REST discovery document for exchange/protection-like resource exposure.

For yield groups, the probe evaluates `targetedAdUnits` separately from
`excludedAdUnits`. A requested ad unit matched by active yield-group targeting
without a covering exclusion is reported as `targeted_exposed`; a requested ad
unit covered by an exact exclusion, or by a descendant-inclusive exclusion when
ancestor context is available, is reported as `targeted_and_excluded`.

The tool deliberately does not claim full certainty when GAM does not expose a
surface. Its top-level decision returns one of:

- `attention_required` when an exposed surface shows a direct issue or target
  match, including `targeted_exposed` yield-group exposure;
- `partial_api_proof` when reads are capped, blocked, or unsupported by the
  current API surface;
- `api_exposed_surfaces_clear` only when exposed API surfaces are complete and
  clear.

`protections`, `inventory_rules`, and `unified_pricing_rules` are reported as
unsupported or unintegrated surfaces unless a future API/read implementation
adds authoritative coverage. Do not interpret their absence from the probe as
proof that those settings are clean in the GAM UI. The response includes a
stable `result_fingerprint`. For one to ten exact targets resolved to resource
names in the requested network, it also includes a canonical receipt template
with `source_version=gam-evidence-producer-v3`. Complete API proof remains
`manual_ui_proof_required` until the unsupported GAM UI surfaces are reviewed.
Unknown yield-group activity remains partial. Probe network codes are
canonicalized once, and foreign-network or malformed resource rows cannot
contribute targets or exposure decisions. Nested ancestor identities use the
same network and canonical-ID rule; malformed hierarchy data makes the proof
partial instead of being treated as a clear hierarchy read. Invalid SOAP API
versions fail input validation before any provider read.

Observed private-auction or private-deal rows require attention even when the
REST page is capped; the cap separately keeps certainty partial. Yield-group
proof is complete only when the SOAP response supplies a usable total matching
the inspected result set. Missing, malformed, or inconsistent totals remain
sample-only evidence.

Receipt state `partial_blocked` means the producer observed a stop condition
while one or more exposed-API surfaces remained incomplete. It prevents a
retirement consumer from losing either half of that evidence. State
`complete_blocked` is reserved for confirmed target exposure with complete
exposed-API proof.

Small results keep the native response shape. If the full response would exceed
the Contract V1 or RMCP transport limit, the adapter returns a validated compact
projection instead. Decisions, certainty and proof states, aggregate
counts, cap/truncation flags, block classes, and the no-mutation policy remain
authoritative. Expanded match arrays, raw SOAP, and diagnostic detail move to a
typed omission ledger with exact source and witness counts. Eligible receipts
are rebound to that projection; `not_generated` remains explicitly unbound. A
compact result is rejected rather than returned if those semantics or receipt
bindings drift.
Compact success is explicit at `data.result_projection`; its
`receipt_binds_returned_projection` flag states whether the returned receipt is
usable. `data.source_result_fingerprint` identifies the pre-projection proof for
audit only. Consumers must persist and compare the returned
`data.result_fingerprint` and, when binding is true, the matching
`data.evidence_receipt_template.result_hash`. They must not substitute the
source fingerprint for the returned receipt hash. An oversized error uses the
same explicit marker at `meta.result_projection` and is never receipt-bearing.

## `gam_ad_unit_dependency_probe`

`gam_ad_unit_dependency_probe` is a read-only helper for ad-unit cleanup,
archive, and retargeting investigations. It accepts:

- `network_code`
- exact `ad_unit_codes` and/or numeric `ad_unit_ids`
- optional SOAP `api_version`
- optional `line_item_page_size`, `max_line_items`, and `placement_page_size`
- optional `include_line_item_xml` for bounded matched line-item XML

The tool resolves exact ad-unit rows through REST, scans placement membership
through the curated `placements` collection, then scans bounded pages of SOAP
`LineItemService.getLineItemsByStatement`. It reports dependency classes such
as `exact_target`, `ancestor_descendant_target`, `placement_target`,
`root_or_network_target`, and the corresponding excluded states.

Placement and line-item dependencies are returned as counts plus bounded
samples. When `include_line_item_xml=true`, matched XML is still capped and
marked with byte and truncation metadata.

`line_items.request_ids` and `line_items.response_times` are prefix samples,
not authoritative totals. Consumers must use `request_id_count` and
`response_time_count` for observed totals, inspect `request_ids_truncated` and
`response_times_truncated`, and honor `transport_metadata_sample_limit`. A
sample is complete only when its truncation flag is false and its length equals
the corresponding count. Results using this sampling contract identify
`source_version=gam-evidence-producer-v3`; consumers that only understand older
producer versions must reject the result rather than treat a prefix sample as
complete. Generated and `not_generated` receipt templates both expose this
producer version.

The response uses `dependency_decision` plus `proof_flags`, not a cleanup
approval. Any capped line-item read, truncated SOAP response, id-only target,
unknown placement membership shape, or blocked SOAP scope remains incomplete
evidence. Do not archive, deactivate, or retarget inventory solely because this
tool returns `no_dependencies_observed` or `incomplete_no_dependencies_observed`.
When a dependency is observed during an incomplete scan, the v3 receipt uses
`partial_blocked`; `complete_blocked` requires complete placement and line-item
proof.
The response includes a stable `result_fingerprint` and emits the same
versioned receipt template only for one to ten fully resolved, exact,
network-bound target rows. Unresolved, ambiguous, id-only, or cross-network
resolved target scopes receive an explicit `not_generated` marker instead;
duplicate request inputs fail validation. Placement resources, placement
members, and ad-unit ancestors are also checked against the requested network.
Malformed nested identities make the relevant surface incomplete and cannot
create a dependency. Invalid SOAP API versions fail before provider access.
Caller-actionable SOAP permission and authentication faults are classified
separately from server-side connection and generic upstream read failures.
If a later SOAP page blocks after an earlier page observed a dependency, the
line-item summary preserves the accumulated count, bounded sample, status
counts, request metadata, and inspected-row progress. Its `proof_state` remains
`blocked`, while its decision remains `dependencies_found`; a late read failure
cannot erase already-observed positive evidence. A block before any dependency
is observed remains `blocked`.

Small dependency results keep the native response shape. Oversized results use
the same validated compact-projection contract as the exchange probe. The
dependency decision, every proof flag, placement and line-item counts and
progress fields, late-block state, status counts, and
`safe_to_archive_or_retire=false` remain unchanged. Detail arrays and optional
XML are replaced by explicit aggregate counts, bounded witnesses, and an exact
typed omission ledger; they are never silently shortened under native
completeness flags. Generated receipts are rebound to the returned projection,
while `not_generated` remains ineligible.

Before rebinding, the adapter re-derives all six dependency proof flags from
the retained targets, resolution issues, placement state, and line-item
progress. Contradictory source flags fail closed rather than receiving a new
compact receipt.

Skipped and permission-blocked dependency surfaces use the same compact
contract without inventing scan metadata they never produced. When optional
line-item XML is omitted, `upstream_xml_sample_summary` preserves the sample
count, original and retained UTF-8 byte totals, and the number of truncated XML
samples. The omission ledger separately counts and fingerprints the actual
retained XML sample bytes removed from the returned projection; original
upstream byte totals remain summary metadata. Nested placement, yield-group, and line-item target evidence must stay
within the exact top-level probe target scope before a returned receipt can bind
the compact result.

## `gam_ad_unit_retirement_assessment`

`gam_ad_unit_retirement_assessment` is the staged, read-only retirement review
surface. The current stage combines exact identity, bounded hierarchy and
descendant reconciliation, and freshness-bound evidence grading. It accepts:

- a canonical positive signed-64-bit numeric `network_code` with no whitespace
  or leading zeroes;
- one to ten unique canonical positive signed-64-bit numeric `ad_unit_ids`;
- zero to five `evidence` receipts, with at most one receipt for each supported
  source;
- optional `ad_unit_page_size` from 1 to 1000, default 1000;
- optional `max_ad_units` from 1 to 10000, default 5000.

Each target is read with an exact REST `adUnits.get` resource name. The result
contains a bounded current identity summary, per-target identity fingerprint,
aggregate identity proof state, and assessment fingerprint. Required row shape
is validated before identity can be clear. Parent resources must be canonical
ad-unit resources in the requested network. Size output includes source and
retained counts, a truncation flag, and a fingerprint over the complete source
array so environment, companion, and tail changes remain bound even when only
20 items are returned. Every size must declare the official `BROWSER` or
`VIDEO_PLAYER` environment; companions are accepted only for `VIDEO_PLAYER`.
Status must be one of `ACTIVE`, `INACTIVE`, or `ARCHIVED`; unspecified or
unknown values keep identity incomplete. Provider errors are classified
without returning provider error text. A
missing target or identity mismatch blocks the identity surface; permission
and pre-authentication failures remain distinct. A confirmed blocker combined
with any unread or incomplete target is `partial_blocked`, not complete batch
proof. Request metadata reports whether identity and catalog calls were
actually attempted.
Target ids are not repeated inside each compact `current` object, and derived
resource names are omitted; the target id plus exact-match flag remain the
identity binding.

The hierarchy scan is bounded by page, row, per-page byte, and total byte
limits, and verifies the numeric ad-unit-id ordering returned by `orderBy=name`.
It validates every row's exact network resource, root-to-parent path, official
status, and child flag; reconstructs direct child state to reconcile
`hasChildren` in both directions; and compares every target against its exact
identity read. An exact `networks.get` read binds the network's
`effectiveRootAdUnit`, and an exact GET of that ad unit binds its Google-created
parent. The catalog reconciliation accepts both the documented root-inclusive
`parentPath` form and the root-omitted form returned by some networks, but only
when the sole catalog root and effective-root relationship match those
authoritative reads. A missing deeper ancestor still fails closed.
Pagination drift, malformed pages or tokens, catalog gaps, cycles, duplicate
ids, cross-network paths, or caps remain incomplete. Known active or inactive
external descendants remain positive blockers even if a later page fails.
Archived descendants do not block. When targets contain an ancestor and its
child, the response returns a deterministic child-first order. Only aggregate
counts, bounded samples, issue codes, and fingerprints are returned.

The evidence grader accepts dependency, delivery, exchange/protection,
site-contract, and telemetry receipts. Every non-`not_run` receipt must bind to
the exact network and complete target set, a supported source version, a
canonical opaque result hash, an observation timestamp, and a TTL no greater
than 31 days. Delivery and telemetry receipts also require a non-zero window of
at least 30 days whose end is no later than the observation. Stale, duplicate,
malformed, unsupported, cross-network, or differently scoped receipts fail
closed. A protection receipt cannot become clear without
`manual_ui_proof_included=true` while GAM protection surfaces remain partly
UI-only. The output exposes bounded grading states and binding fingerprints,
not raw receipt notes or provider payloads. Receipt provenance is always
`caller_supplied_unverified`.
Absent sources use the compact `{state: "not_run"}` form; full binding and
freshness diagnostics are returned only for supplied receipts.

Receipts from the built-in dependency and exchange/protection probes must match
the complete `gam-evidence-producer-v3` contract: a 16-character lowercase
hexadecimal result fingerprint, the producer's 3600-second TTL, its fixed
provenance and non-authorisation metadata, and a state the named producer can
actually emit. Unknown receipt fields are rejected rather
than discarded, so callers cannot attach raw reports or telemetry payloads to
the compact contract. Freshness is evaluated after the live identity and
hierarchy reads complete.

The `recommendation` surface returns one of three fail-closed decisions:

- `blocked_by_current_state_or_evidence` when any surface reports a confirmed
  blocker, including a blocker observed during an incomplete scan;
- `not_eligible_incomplete_evidence` when no blocker is confirmed but one or
  more required surfaces are incomplete; or
- `evidence_complete_operator_review_required` only when current identity,
  hierarchy, dependency, delivery, exchange/protection, site-contract, and
  telemetry surfaces are all complete and clear.

The response preserves the required child-first target order and provides a
bounded, surface-aware next action for each incomplete or blocked surface. Its
assessment fingerprint includes the versioned recommendation contract, so a
decision-semantic change cannot silently reuse an older assessment identity.
Every successful
assessment still reports `mutation_performed=false`,
`archive_or_deactivate_authorized=false`,
`automated_retirement_eligible=false`, and
`safe_to_archive_or_retire=false`. The strongest decision does not verify who
supplied the receipts and is not an archive authorization; explicit operator
review and any separate guarded write remain outside this tool.
The inner data, complete model-visible Contract V1 content, and serialized RMCP
result are independently measured against 7 KiB, 8 KiB, and 20 KiB limits.

## `gam_report_run`

`gam_report_run` is designed for saved reports that already exist in Ad
Manager. It accepts:

- `network_code`
- `report_id`
- optional wait controls
- optional first-page fetch controls

When `wait_for_completion=true`, the tool consumes the validated POST response as
the first long-running-operation observation, then polls only if needed and
returns the `report_result` resource name once complete. If
`fetch_first_page=true`, it also returns the first page of rows so the first
successful report run is immediately useful.

When `wait_for_completion=false`, the tool sends the provider-required empty
POST body, starts exactly one report run, and returns its `operation_name`.
Continue that same run with
`gam_report_operation_poll`; do not call `gam_report_run` again merely to poll.
If the POST handoff is already complete, the tool returns the terminal result
directly with `waited=false` and no redundant poll continuation.
The poll tool validates the existing operation name, waits for completion, and
can fetch the first result page without creating another run. The run request,
returned operation, polled operation, optional `metadata.report`, and final
`reportResult` must all bind to the same network/report identity. A known
requested report fills omitted metadata; for a completed poll without that
caller binding, a valid `reportResult` may safely derive the report identity.
Inconsistent present metadata and invalid `done`/`error`/`response` unions are
rejected.
Both the upstream handoff and caller input must match the exact
`networks/{networkCode}/operations/reports/runs/{operationId}` resource shape;
noncanonical values are rejected before polling.
Operation responses are read through a 64 KiB limit and projected to documented
fields. Result pages are read through a 512 KiB limit, accept page sizes from 1
through 1,000, validate documented object/row/page-token/count types, and pass
model-visible and complete RMCP result guards. Poll timeouts are between 1 ms
and 24 hours; startup rejects a
`GOOGLE_AD_MANAGER_MCP_REPORT_POLL_TIMEOUT_MS` default outside that same range.
Initial intervals are between 5 and 30 seconds with bounded
exponential backoff. The absolute deadline covers each in-flight GET and sleep.
Post-start timeout, transport, or provider-contract errors preserve
`operation_name`, the optional `expected_report_name`, the last valid
observation, and a GET-only poll continuation when the GET can safely be
retried. A definitive poll-time 4xx other than 408 or 429 returns
remediation-required detail with no executable continuation and does not claim
that the report operation itself is terminal. Timeout continuation uses a
bounded larger timeout rather than repeating the expired value. Terminal
operation errors and completed operations without a safe report/result identity
do not offer another poll. Run-time 4xx rejections other than HTTP 408 are
definitive upstream API errors. Once the initial
POST may otherwise have been dispatched, transport, body-read, response-bound,
plausible server-status, JSON, missing, malformed, or cross-target handoff
failures report an uncertain handoff, set `automatic_replay_safe=false`, and
prohibit automatic reruns. Every uncertain receipt retains the canonical
`report_name`, an explicit `dispatch_state`, and `started_new_run=null` rather
than claiming that a new run was confirmed. An immediate terminal operation error is a redacted
MCP error with no success continuation. If
completion succeeds but retryable first-page retrieval fails, the error instead
preserves `report_result`, page size, and a `gam_report_result_rows`
continuation. Authentication failures and definitive first-page 4xx responses
other than 408 or 429 preserve the handle but require remediation without an
executable unchanged continuation.
Direct `gam_report_result_rows` failures follow the same contract and preserve
the exact bounded `result_name` and `page_size`. Opaque `page_token` values are
not copied into error output: the receipt records whether a token was supplied
and its byte length, while retryable continuations require the caller to reuse
the exact token from the original request. Raw token length is checked before
interpretation, and leading or trailing whitespace is rejected rather than
silently normalized. Only transport, 408, 429, and 5xx
failures return a GET continuation. Invalid input, authentication, permanent
4xx, malformed JSON, and provider-contract failures require remediation and
expose no unchanged retry.
Deterministic upstream or final RMCP result-size failures instead preserve
bounded operation/report/result/page context, set
`automatic_replay_safe=false`, and expose a non-executable smaller page
adjustment. They do not repeat the rejected page size; a page already at
size 1 has no smaller recommendation and requires reducing the saved report's
dimensions or filters before a new run.

Use `gam_report_result_rows` with the returned `report_result` when:

- the first page was truncated;
- you intentionally skipped fetching the first page during `gam_report_run`;
- you want to revisit a completed report result later.

## Write And Trafficking Tools

Write and trafficking tools are available as guarded preview/apply pairs:

1. `gam_trafficking_tool_matrix`
2. `gam_rest_write_plan`
3. `gam_rest_write_apply`
4. `gam_soap_payload_build`
5. `gam_soap_trafficking_plan`
6. `gam_soap_trafficking_apply`
7. `gam_yield_group_exclusions_preview`
8. `gam_yield_group_exclusions_apply`

Every REST/SOAP plan and every non-noop yield preview includes an
`apply_rediscovery` object. It gives deferred-loading clients the exact
`find_tools` request (`group="trafficking", read_only=false`) needed to load the
matching apply schema, plus receipt paths for the reviewed request and
confirmation token. The continuation requires explicit operator intent and
does not authorize a mutation or weaken any apply gate.

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

`gam_soap_payload_build` is a no-upstream-call helper for the most common inner
SOAP `payload_xml` fragments. It returns the matching SOAP operation, generated
payload XML, warnings, and the next `gam_soap_trafficking_plan` request shape.
It is intentionally a template renderer, not a live GAM operation. Discovery
guides the optional builder before plan and the plan before apply; the server
does not treat those discovery edges as proof that either companion call ran.

Payload templates currently exposed:

- `order_by_id`
- `line_item_by_id`
- `line_items_by_order_id`
- `creatives_by_advertiser_name`
- `licas_by_line_item_id`
- `lica_preview_url`
- `create_lica`
- `pause_line_item`
- `resume_line_item`
- `archive_line_item`
- `delivery_forecast_by_line_item_ids`
- `availability_forecast_by_line_item_id`
- `yield_groups_by_statement`
- `yield_groups_all`
- `yield_partners`

The delivery-forecast template emits repeated `<lineItemIds>` elements plus an
empty `<forecastOptions />` argument, which Ad Manager SOAP expects even when
no optional forecast controls are being set.

`yield_partners` intentionally emits an empty `payload_xml` string because
`YieldGroupService.getYieldPartners` has no request body. Other SOAP operations
still require a non-empty inner XML fragment.

`yield_groups_by_statement` and `yield_groups_all` emit a
`<statement><query>...</query></statement>` fragment. `YieldGroupService`
uses the `statement` wrapper for `getYieldGroupsByStatement`; the
`filterStatement` wrapper remains correct for order, line-item, creative, and
LICA by-statement operations.

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
- `YieldGroupService`: `get_yield_groups_by_statement`, `get_yield_partners`

The SOAP request shape is intentionally thin:

- choose an allowlisted `operation`;
- provide `payload_xml` as the inner operation XML only;
- use an empty `payload_xml` only for no-body reads such as
  `get_yield_partners`;
- do not include a SOAP envelope, SOAP header, OAuth token, request header,
  XML declaration, DTD, or entity declaration;
- review the generated envelope returned by `gam_soap_trafficking_plan`;
- call `gam_soap_trafficking_apply` with the exact matching confirmation
  token.

Example SOAP payload builder call:

```json
{
  "template": "line_items_by_order_id",
  "values": {
    "order_id": "123456789"
  }
}
```

The returned `payload_xml` can be copied into `gam_soap_trafficking_plan`.

All live SOAP calls require
`GOOGLE_AD_MANAGER_MCP_SCOPE=https://www.googleapis.com/auth/admanager`.
This includes non-mutating forecast/read operations because Google Ad
Manager's legacy SOAP API does not accept the newer read-only Ad Manager
scope. Mutating SOAP operations additionally require
`GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`, `expected_impact`, and
`rollback_note`.

## `gam_yield_group_exclusions_preview` And `gam_yield_group_exclusions_apply`

These tools provide a typed, guarded path for adding descendant-safe ad-unit exclusions
to an existing `YieldGroupService` yield group. They are intentionally separate
from the generic SOAP tools because a safe update must preserve the current
yield-group object and prove readback.

`gam_yield_group_exclusions_preview` accepts:

- `network_code`
- `yield_group_id`
- `excluded_ad_unit_ids`
- optional SOAP `api_version`
- optional `include_payload_xml`
- `reason`
- optional apply context fields

The preview tool:

- reads the current yield group with
  `YieldGroupService.getYieldGroupsByStatement`;
- preserves existing yield-group targeting, including targeted ad units,
  existing excluded ad units, and targeted placement ids;
- adds missing `excludedAdUnits` entries, or repairs requested existing entries,
  with `includeDescendants=true` because GAM can reject self-only inventory-unit
  exclusions with
  `InventoryTargetingError.SELF_ONLY_INVENTORY_UNIT_NOT_ALLOWED`;
- refuses to exclude an ad unit that the same yield group directly targets;
- binds the confirmation token to the current readback fingerprint and the
  exact schema-complete apply request, including requested ad-unit IDs,
  `include_payload_xml`, reason, expected impact, rollback note, and idempotency
  key, plus the descendant-safe update payload.

`gam_yield_group_exclusions_apply` requires the same request, the exact
preview confirmation token, the manage scope, write mode enabled,
`expected_impact`, and `rollback_note`. Before applying it re-reads the yield
group and rebuilds the payload. If the readback changed, the old confirmation
token no longer matches. After `updateYieldGroups`, it re-reads the yield group
and reports success only when every requested ad-unit ID is present in
`excludedAdUnits` with `includeDescendants=true`.

If every requested ad-unit ID is already excluded with `includeDescendants=true`,
the apply path returns a no-op proof and does not call `updateYieldGroups`.

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
4. `gam_scratchpad_ingest_soap_line_items` for current delivery/status rows
   from `LineItemService.getLineItemsByStatement`.
5. `gam_scratchpad_query` with read-only SQL.
6. `gam_scratchpad_export_evidence_bundle` for a bounded markdown summary.
7. `gam_scratchpad_close_session` when finished.

Scratchpad report-row ingestion uses the same schema and runtime bounds as
direct row retrieval: `page_size` is 1 through 1,000 and `page_token` is at most
4,096 bytes.

`gam_scratchpad_ingest_soap_line_items` accepts a bounded line-item PQL query.
Queries must start with `WHERE`, `ORDER BY`, or `LIMIT`; the tool appends
`LIMIT 500` when a query has no explicit limit and rejects limits above 1000.
The ingested rows include line item/order ids and names, status, type, priority,
creative sizes, impressions, clicks, delivery percentages, goals, missing
creative/archive flags, targeted ad-unit ids, custom-targeting ids, and the
bounded upstream XML for private local evidence.

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
