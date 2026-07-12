# Security Model

## Core posture

- Public repository
- Read tools and write previews are enabled by default
- Live write apply is disabled by default
- Official Google Ad Manager REST beta and SOAP APIs only
- No generic HTTP or SOAP proxy
- No ambient write or approval surfaces
- No credential material in tool output
- Bounded local scratchpad analysis only

## Credential handling

The server uses standard Google credential sources plus explicit server-specific
service account inputs:

- `GOOGLE_APPLICATION_CREDENTIALS`
- `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH`
- `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON`
- server-specific local ADC from `google-ad-manager-mcp auth login`
- conventional shared local ADC from `gcloud auth application-default login`

The local login helper uses a Google-Ad-Manager-specific gcloud config
directory by default. This keeps its refresh token and scopes separate from
other Google MCP servers that may run under the same OS user. Conventional
shared local ADC remains a compatibility fallback when the server-specific ADC
file has not been created yet.

Tool responses may report whether a credential source looks configured or
whether a low-cost access check succeeded. They must not return:

- access tokens
- refresh tokens
- bearer headers
- client secrets
- private keys
- whole credential files

`find_tools` also treats its free-form query and group inputs as potentially
secret-bearing. Public discovery results omit raw query text and query-derived
terms, return an exact group only when it matches a registered strict
list-visible group, and otherwise expose only presence, recognition, and term
counts. Provider semantics are computed only from the toolkit's bounded ranked
query/group projection and are disabled when input truncation makes that
projection fail closed; raw caller strings are never rescanned after search.
Narrow schema expansion is capped at five direct-plus-companion tools.
Toolkit-reported negative or excluded intent suppresses provider canned workflow
and scratchpad recovery. Report starts create an upstream job and are registered
with a non-read-only toolkit risk posture, so default read-only discovery
excludes them. An explicit `read_only=false` search can expose the start tool,
but existing-operation continuation intent projects it as condition-only to
prevent duplicate report runs.
The complete RMCP result is guarded at 64 KiB with a 48 KiB structured-envelope
cap and a bounded actionable JSON text projection rather than a duplicate full
payload. Failed Contract V1 results set MCP `isError=true` as well as
`ok:false`.

Report operation and result handles are length-bounded and identity-bound. The
run request uses an empty body. Once the non-idempotent POST may have been
dispatched, transport, response, decode, and plausible server failures are
uncertain handoffs with `automatic_replay_safe=false` and no automatic retry
guidance; definitive 4xx rejections remain upstream API errors. The operation
name, optional `metadata.report`, and final `reportResult` remain in one
network/report scope. Known expected identity may fill omitted metadata but
cannot override inconsistent present metadata. Invalid LRO result unions fail
closed, terminal errors set MCP `isError`, and the validated POST observation
seeds polling. Operation/result HTTP bodies and complete MCP results have
explicit caps, successful row payloads are shape-validated, and deterministic
size rejection preserves bounded handles with a non-executable smaller-page
adjustment. GET-only continuations preserve the optional expected report
identity and the last valid observation.

## Tool-surface restrictions

The server intentionally does not expose:

- arbitrary Google REST calls
- arbitrary Google SOAP calls
- raw OAuth token exchange helpers
- unallowlisted generic entity creation or patch methods
- default live mutations
- bulk export or file-write surfaces

Order, line-item, creative, line-item creative association, preview URL, and
forecast SOAP operations are exposed only through a typed allowlist. Callers
provide the inner operation XML fragment; the server owns the envelope,
request header, endpoint, OAuth bearer header, confirmation token, and runtime
gates.

`gam_soap_payload_build` is not a write path and does not call Google. It
renders a bounded allowlist of common inner SOAP payload fragments from
validated IDs or safe name fragments, then directs callers to
`gam_soap_trafficking_plan`.

## Write safety

The write surface uses a preview/apply contract:

- default `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=preview_only`
- `gam_rest_write_plan` builds a dry-run plan without calling mutation
  endpoints
- `gam_rest_write_apply` requires
  `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`
- apply requires the manage scope
  `https://www.googleapis.com/auth/admanager`
- local ADC users can request that scope with
  `google-ad-manager-mcp auth login --manage-scope`
- apply requires the confirmation token returned by the matching plan
- plan/non-noop-preview receipts expose an explicit `apply_rediscovery` call
  for deferred-loading clients, but that continuation is non-authorizing and
  preserves every apply gate
- apply requires `reason`, `expected_impact`, and `rollback_note`
- create and patch operations attempt post-apply readback through the returned
  or target resource name
- `gam_soap_payload_build` performs no upstream call and produces only inner
  payload XML for the guarded SOAP plan/apply tools
- `gam_soap_trafficking_plan` builds a SOAP envelope without calling upstream
- `gam_soap_trafficking_apply` requires the exact confirmation token returned
  by the matching SOAP plan
- every live SOAP call requires the manage scope because the legacy SOAP API
  does not accept the newer Ad Manager read-only scope
- mutating SOAP calls also require write mode enabled, expected impact, and a
  rollback note
- `gam_yield_group_exclusions_preview` reads the current yield group and binds
  the confirmation token to the readback fingerprint, requested ad-unit
  exclusions, descendant-safe update payload, payload-output choice, reason,
  expected impact, rollback note, and idempotency key; its receipt contains the
  exact schema-complete request for apply
- `gam_yield_group_exclusions_apply` re-reads before apply, calls
  `updateYieldGroups` only when requested exclusions are missing or not already
  `includeDescendants=true`, and re-reads after apply before reporting success
- SOAP payload fragments may not include envelopes, request headers, XML
  declarations, DTD/entity constructs, bearer tokens, refresh tokens, client
  secrets, or private keys

Batch operations return the upstream response and explicit verification
guidance. Operators should follow with read/list tools or scratchpad evidence
when the upstream batch response does not include a directly readable resource.

SOAP apply returns bounded raw XML plus request metadata when Google includes
it. Operators should follow mutating SOAP calls with a get-by-statement,
forecast, report, or scratchpad evidence check appropriate to the trafficking
change.

Yield-group exclusion apply is the exception to the generic SOAP readback gap:
it performs the matching post-apply yield-group readback itself and fails
closed when the requested ad-unit IDs are not proven in `excludedAdUnits` with
`includeDescendants=true`.

## Retirement evidence safety

`gam_ad_unit_retirement_assessment` accepts compact evidence receipts, not raw
reports, telemetry rows, screenshots, or provider payloads. Each receipt is
bound to one known source contract, the exact network and target set, an opaque
result hash, an observation time, and a bounded TTL. Delivery and telemetry
also require an evidence window of at least 30 days. Duplicate, stale,
unsupported, malformed, cross-network, or differently scoped receipts remain
incomplete. Exchange/protection evidence cannot become clear from API proof
alone while relevant GAM UI-only surfaces remain unsupported. Operator notes
are bounded and never echoed. Receipts remain caller-supplied and do not verify
operator identity or authorize any mutation.
Built-in probe receipts are additionally checked against their complete
producer contract: exact fingerprint shape, fixed TTL, fixed provenance and
non-authorisation metadata, and source-possible states. Receipt objects reject unknown fields instead of silently discarding
attached raw payloads. Freshness is evaluated after provider reads so evidence
cannot clear merely because it was valid when a long assessment began.

## Scratchpad safety

Scratchpad tools use `mcp-toolkit-scratchpad` and DuckDB for local, bounded
analysis of rows already returned by read-only Ad Manager tools.

The scratchpad boundary is:

- local to the MCP server runtime;
- bounded by session TTL, session count, table count, row count, memory, SQL
  payload size, and query timeout;
- restricted to read-only SQL inspection patterns;
- not an Ad Manager write path;
- not a generic filesystem or external data ingestion path.

If `GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_ROOT_DIR` is set, it must point to an
existing absolute directory. Otherwise the default scratchpad location is under
the operating system temporary directory.

## Report safety

Saved report execution is still read-only, but the result payload can be large.
This server bounds that surface by:

- requiring an explicit saved report identifier
- using paginated result fetching
- returning structured JSON rather than writing files
- keeping long-running operation polling bounded by a timeout
- enforcing a 5-second minimum initial interval with bounded backoff and a
  deadline over every in-flight GET
- validating successful fetchRows objects, row arrays, page tokens, and row
  counts before returning `ok:true`

## Public logging and diagnostics

Errors are redacted before being returned through Contract V1 envelopes.
Secret-bearing tokens such as `access_token`, `private_key`, and bearer headers
are replaced with `[redacted]`. When an explicit credential marker has no
inline syntactic value boundary, the remainder of that diagnostic is redacted;
the adapter does not guess which later prose token contains the secret. Benign
authorization-failure wording without credential syntax remains readable.

Probe provider diagnostics use the same redaction boundary and a UTF-8-safe
byte cap before fingerprinting. SOAP faults, request metadata, and provider
error text are returned only as bounded samples. Repeated transport metadata
is count-preserving and sample-capped, and every diagnostic cap has an explicit
truncation flag. A multibyte character at the cap cannot panic or split invalid
UTF-8.
