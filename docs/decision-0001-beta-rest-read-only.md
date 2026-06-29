# Decision 0001: Use Ad Manager Beta REST And Stay Read-Only First

## Status

Accepted

## Context

The product goal is a public, lightweight, easy-to-install Google Ad Manager
MCP built on `mcp-toolkit-rs`. The official Ad Manager API (Beta) now covers
the first release's most important read paths:

- networks
- ad units
- orders
- line items
- saved reports
- report execution
- report result rows

The legacy SOAP API remains relevant for broader historic coverage, but it adds
transport complexity and a migration burden that the initial public release does
not need.

## Decision

The initial public release will:

- use only the official Google Ad Manager API (Beta);
- keep the MCP surface read-only;
- treat saved report execution and result retrieval as the reporting path;
- keep any future SOAP fallback isolated behind a separate adapter boundary.

## Consequences

### Positive

- Smaller public surface
- Easier install and review story
- Cleaner `mcp-toolkit-rs` reference implementation
- Lower risk of accidentally exposing write flows

### Trade-offs

- Some SOAP-only read gaps remain out of scope for v1
- Report workflows depend on pre-existing saved reports
- Large report retrieval stays paginated and explicit

## Follow-up trigger

If a clearly valuable read-only Ad Manager workflow is blocked by missing Beta
coverage, add a separate design slice that proves the gap, keeps the MCP tool
surface stable, and documents the adapter boundary before any SOAP code lands.
