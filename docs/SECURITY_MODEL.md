# Security Model

## Core posture

- Public repository
- Read tools and write previews are enabled by default
- Live write apply is disabled by default
- Official Google Ad Manager API (Beta) only
- No generic HTTP proxy
- No ambient write or approval surfaces
- No credential material in tool output
- Bounded local scratchpad analysis only

## Credential handling

The server uses standard Google credential sources plus explicit server-specific
service account inputs:

- `GOOGLE_APPLICATION_CREDENTIALS`
- `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH`
- `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON`
- local ADC from `gcloud auth application-default login`

Tool responses may report whether a credential source looks configured or
whether a low-cost access check succeeded. They must not return:

- access tokens
- refresh tokens
- bearer headers
- client secrets
- private keys
- whole credential files

## Tool-surface restrictions

The server intentionally does not expose:

- arbitrary Google REST calls
- raw OAuth token exchange helpers
- unallowlisted generic entity creation or patch methods
- default live mutations
- bulk export or file-write surfaces

The missing order, line-item, creative, line-item creative association, and
forecast apply tools are intentional REST beta boundaries, not hidden generic
proxy paths. They require a SOAP-capable follow-up layer before live use.

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
- apply requires `reason`, `expected_impact`, and `rollback_note`
- create and patch operations attempt post-apply readback through the returned
  or target resource name

Batch operations return the upstream response and explicit verification
guidance. Operators should follow with read/list tools or scratchpad evidence
when the upstream batch response does not include a directly readable resource.

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

## Public logging and diagnostics

Errors are redacted before being returned through Contract V1 envelopes.
Secret-bearing tokens such as `access_token`, `private_key`, and bearer headers
are replaced with `[redacted]`.
