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
| `gam_report_run` | Run a saved Ad Manager report, optionally wait, and optionally fetch the first result page. |
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
proof that those settings are clean in the GAM UI.

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

The response uses `dependency_decision` plus `proof_flags`, not a cleanup
approval. Any capped line-item read, truncated SOAP response, id-only target,
unknown placement membership shape, or blocked SOAP scope remains incomplete
evidence. Do not archive, deactivate, or retarget inventory solely because this
tool returns `no_dependencies_observed` or `incomplete_no_dependencies_observed`.

## `gam_report_run`

`gam_report_run` is designed for saved reports that already exist in Ad
Manager. It accepts:

- `network_code`
- `report_id`
- optional wait controls
- optional first-page fetch controls

When `wait_for_completion=true`, the tool polls the Ad Manager long-running
operation and returns the `report_result` resource name once complete. If
`fetch_first_page=true`, it also returns the first page of rows so the first
successful report run is immediately useful.

Use `gam_report_result_rows` with the returned `result_name` when:

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
It is intentionally a template renderer, not a live GAM operation.

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
  requested ad-unit IDs and descendant-safe update payload.

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
