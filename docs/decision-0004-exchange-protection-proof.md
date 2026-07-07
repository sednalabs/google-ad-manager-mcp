# Decision 0004: Exchange And Protection Proof Surface

The MCP needs a read-only way to prove, as far as the Google Ad Manager APIs
allow, whether premium or special-occasion ad units are eligible for open
exchange, private-auction, open-bidding, or other account-level delivery paths.
The tool must not turn partial API coverage into a green-looking answer.

## Capability Matrix

| Workflow | Tool | Class | Inputs | Data Source | Proof | Redaction | Negative Tests | Docs | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Prove exact ad-unit flags and sizes | `gam_exchange_protection_probe` plus `gam_network_catalog_list` | read | `network_code`, exact ad-unit codes | REST `adUnits` collection | Exact ad-unit row, size list, status, `appliedAdsenseEnabled`, `effectiveAdsenseEnabled`, `explicitlyTargeted` | Raw OAuth/token data never returned; ad-unit codes are operator-supplied identifiers | Missing unit, duplicate rows, missing boolean fields, capped rows | Tool Guide | implemented |
| List private-auction and private-auction-deal exposure | `gam_exchange_protection_probe` plus `gam_network_catalog_list` | read | `network_code`, optional page cap | REST `privateAuctions`, `privateAuctionDeals` collections | Row count sample, next-page token/capped flag, status sample | Full rows omitted from the high-level probe; direct catalog list remains explicit | Unsupported upstream collection, permission failure, capped reads | Tool Guide | implemented |
| Prove open-bidding/yield group targeting | `gam_exchange_protection_probe`; raw read through `gam_soap_trafficking_*` | read | `network_code`, exact ad-unit codes, optional SOAP version | SOAP `YieldGroupService` | `getYieldGroupsByStatement` response status, request id, total result count, inspected result count, ad-unit-id matches | High-level probe returns summary and redacted fault text, not raw SOAP by default | SOAP permission failure, SOAP fault, truncated XML, capped result set, missing ad-unit ids | Tool Guide | implemented |
| Add descendant-safe ad-unit exclusions to an existing yield group | `gam_yield_group_exclusions_preview`, `gam_yield_group_exclusions_apply` | preview/apply | `network_code`, `yield_group_id`, ad-unit ids, optional SOAP version, reason, expected impact, rollback note, confirmation token | SOAP `YieldGroupService` | Preview readback fingerprint, generated `updateYieldGroups` payload hash, no-op detection, post-apply `getYieldGroupsByStatement` proof that every requested id is in `excludedAdUnits` with `includeDescendants=true` | Payload XML is omitted by default; tool returns ids, include-descendant policy, hashes, byte counts, request ids, and bounded fault text | Missing manage scope, write-mode disabled, token mismatch after stale readback, missing targeting, target/exclude conflict, no-op duplicate, self-only readback, post-apply readback mismatch | Tool Guide, Security Model | implemented |
| Read yield partners | `gam_soap_payload_build` plus `gam_soap_trafficking_*` | read | `network_code`, optional SOAP version, empty payload from `yield_partners` | SOAP `YieldGroupService` | `getYieldPartners` response status and raw SOAP response through explicit SOAP apply | Raw SOAP only returned by explicit SOAP tool call | SOAP permission failure, SOAP fault, accidental non-empty payload dependency | Tool Guide | implemented |
| Confirm whether current REST API exposes protection, inventory-rule, or unified-pricing resources | `gam_exchange_protection_probe` | read | none beyond network context | REST discovery document | Observed discovery resource names and unsupported-surface list | Discovery document is public API metadata; no account data | Discovery fetch failure, unexpected resource names | Tool Guide | implemented |
| Claim account is fully protected from all exchange/yield paths | none | unsupported proof | n/a | n/a | The MCP must not claim this when GAM exposes only partial API coverage | n/a | Any partial/capped/unsupported surface returns partial or attention-required state | Tool Guide | deliberately unsupported |

## Certainty Contract

`gam_exchange_protection_probe` returns an `overall_decision`:

- `attention_required`: a requested ad unit is missing, a direct flag indicates
  AdSense/open eligibility, private auction/deal rows are present, or an active
  yield group reports `targeted_exposed` for one of the requested units.
- `partial_api_proof`: exposed API surfaces were checked, but at least one
  relevant surface is capped, unsupported, or unavailable.
- `api_exposed_surfaces_clear`: exposed API surfaces were checked and no target
  exposure was found; this is not a claim that UI-only or future API surfaces do
  not exist.

The tool always sets `mutation_performed=false` and names unsupported surfaces
explicitly.

For yield groups, `targeted_exposed` means active targeting remains without a
covering inventory exclusion. `targeted_and_excluded` means the requested unit is
covered by an exact exclusion, or by a descendant-inclusive exclusion when the
probe has ancestor context, so that yield-group exposure does not drive the
top-level `attention_required` decision.

False `appliedAdsenseEnabled` or `effectiveAdsenseEnabled` ad-unit flags only
prove those exposed ad-unit fields. GPT render observations such as a slot
rendering with no line-item id are separate delivery evidence; they must not be
collapsed into proof that AdX, protections, inventory rules, unified pricing,
or other account-level yield controls are clean.
