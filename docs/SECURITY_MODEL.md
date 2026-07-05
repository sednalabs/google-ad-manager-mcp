# Security Model

## Core posture

- Public repository
- Read-only initial release
- Official Google Ad Manager API (Beta) only
- No generic HTTP proxy
- No write or approval surfaces
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
- generic report-definition mutations
- generic entity creation or patch methods
- bulk export or file-write surfaces

The missing tools are intentional safety boundaries, not backlog accidents.

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
