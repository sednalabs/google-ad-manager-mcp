# Decision 0003: Add Guarded SOAP Trafficking Adapter

## Status

Accepted

## Context

Google Ad Manager REST beta is useful for networks, catalog listing, saved
reports, and some supporting-resource writes, but classic trafficking still
depends on the legacy SOAP API. Operators need end-to-end coverage for orders,
line items, creatives, line-item creative associations, preview URLs, and
forecasting.

Fully hand-modeling every SOAP object in Rust would delay useful operator
coverage and recreate a partial SDK. A generic SOAP proxy would be faster but
would undermine the MCP safety boundary.

Google's SOAP API is document/literal wrapped. Requests use a service-specific
endpoint, a `RequestHeader` containing `networkCode` and `applicationName`,
and an OAuth bearer token in the HTTP header. The legacy SOAP API requires the
full Ad Manager manage scope, including for non-mutating forecast/read calls.

## Decision

Add a guarded SOAP trafficking adapter with:

- an allowlisted `SoapTraffickingOperation` enum;
- `gam_soap_trafficking_plan`;
- `gam_soap_trafficking_apply`;
- server-owned endpoint selection, SOAP envelope construction, request header,
  and OAuth bearer header;
- caller-supplied inner operation XML fragments only;
- rejection of caller-supplied envelopes, request headers, XML declarations,
  DTD/entity constructs, and credential-bearing strings;
- stable confirmation tokens binding request, endpoint, namespace, service,
  method, and payload;
- bounded raw XML responses plus request metadata where available.

The operation allowlist covers:

- `OrderService`
- `LineItemService`
- `CreativeService`
- `LineItemCreativeAssociationService`
- `ForecastService`

SOAP read/forecast calls are classified as no-mutation proof operations. They
can run without write mode enabled but still require the manage scope because
that is an upstream SOAP requirement. Mutating SOAP operations require
`GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`, the manage scope, a matching
confirmation token, `reason`, `expected_impact`, and `rollback_note`.

## Consequences

### Positive

- Operators get real end-to-end trafficking coverage without waiting for a
  complete Rust SOAP SDK.
- The tool surface remains compact and discoverable.
- SOAP calls share the same preview/apply mental model as REST writes.
- The adapter supports future high-level builders without changing the safety
  boundary.

### Trade-offs

- Callers must provide official SOAP payload XML for the selected operation.
- SOAP responses are returned as bounded XML rather than fully typed JSON.
- Post-apply readback is currently a follow-up operation chosen by the
  operator or agent rather than an automatic companion request.

## Follow-up

Add ergonomic builders for the highest-frequency payloads:

- order create/update;
- line item create/update;
- pause/resume/archive/activate/reserve actions;
- creative create/update;
- line-item creative association create/update;
- forecast availability and delivery checks.

Add optional structured response extraction for common `rval`, page, and
`UpdateResult` shapes after the raw XML path has been validated against real
networks.
