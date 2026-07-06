# Decision 0002: Add Guarded REST Writes Before SOAP Trafficking

## Status

Accepted. The SOAP follow-up described here is implemented by
[Decision 0003](decision-0003-guarded-soap-trafficking-adapter.md).

## Context

Operators need write-capable Google Ad Manager workflows, but the official Ad
Manager API (Beta) does not expose every trafficking mutation through REST.
The REST beta surface currently supports writes for inventory and supporting
resources such as ad units, placements, reports, labels, teams, contacts,
custom fields, custom targeting keys, applications, sites, ad spots, and
related batch state actions.

The classic trafficking workflows around orders, line items, creatives,
line-item creative associations, and forecasting remain SOAP-shaped. Adding
those directly to the REST adapter would either create fake tools that fail at
apply time or push the server toward a generic proxy.

## Decision

The MCP server will add the REST write surface through a guarded plan/apply
pair:

- `gam_rest_write_plan`
- `gam_rest_write_apply`
- `gam_trafficking_tool_matrix`

The plan tool performs no upstream mutation and returns the exact REST request
shape, a stable plan id, and a confirmation token. The apply tool requires:

- `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`
- `GOOGLE_AD_MANAGER_MCP_SCOPE=https://www.googleapis.com/auth/admanager`
- the exact confirmation token from the matching plan
- a human-readable reason
- expected impact
- rollback or reversal notes

At the time of this REST-write release, SOAP-only trafficking workflows stayed
outside the release and were required to land behind a separate SOAP-capable
adapter boundary. That follow-up is now captured in Decision 0003.

## Consequences

### Positive

- Users can plan writes immediately without risking live mutation.
- Operators get one consistent apply contract instead of many bespoke gates.
- The tool surface honestly reports REST-supported writes and SOAP-only gaps
  until the SOAP adapter lands.
- Future SOAP work has a documented boundary rather than leaking into REST
  helpers.

### Trade-offs

- Order and line-item mutation tools are not live apply tools in this REST-only
  release.
- Generic REST write planning requires users to provide the official REST
  request JSON body for the selected operation.
- Batch operation readback may require a follow-up list/get or scratchpad proof
  when the upstream response does not include a directly readable resource
  name.

## Follow-up trigger

Start the SOAP trafficking adapter when an operator needs live order,
line-item, creative, line-item creative association, or forecast workflows.
That adapter should keep the same guarded plan/apply posture and should add
forecast/readback proof before enabling live line-item apply.
