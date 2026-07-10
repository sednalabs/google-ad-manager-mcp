# Decision 0005: Ad-Unit Dependency Proof

Operators need a read-only way to decide whether an Ad Manager ad unit can be
retired, archived, retargeted, or treated as special inventory without manually
joining ad-unit rows, placement membership, line-item XML, exclusions, and custom
targeting. The helper must show dependencies and uncertainty. It must not turn a
bounded sample into an authoritative cleanup decision.

## Capability Matrix

| Workflow | Tool | Class | Inputs | Data Source | Proof | Redaction | Negative Tests | Docs | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Resolve exact ad-unit identities | `gam_ad_unit_dependency_probe` | read | `network_code`, `ad_unit_codes`, optional `ad_unit_ids` | REST `adUnits` collection | Exact code rows, resource ids, sizes, status, ancestors when exposed | OAuth/token data never returned; operator-supplied codes/ids are echoed | Missing code, duplicate code, id-only missing ancestor proof | Tool Guide | implemented |
| Resolve placement membership | `gam_ad_unit_dependency_probe` | read | same as above, `placement_page_size` | REST `placements` collection | Placement rows whose exposed assignment fields contain requested ad-unit ids, returned as counts plus bounded samples | Raw placement JSON is summarized by id/name/status/member-id sample | Capped placement read, unknown placement member shape, no membership fields, sample truncation | Tool Guide | implemented |
| Resolve line-item inventory dependencies | `gam_ad_unit_dependency_probe` | read | same as above, `line_item_page_size`, `max_line_items`, optional SOAP version | SOAP `LineItemService.getLineItemsByStatement` | Inventory targeting XML classification: exact, ancestor-descendant, placement, root/broad, excluded, inactive/archived, activity unknown, returned as counts plus bounded samples | Raw SOAP XML omitted by default; optional matched XML is byte-capped; custom-targeting ids are ids only | SOAP scope missing, SOAP fault, response truncation, capped total, id-only ancestor uncertainty, sample truncation | Tool Guide | implemented |
| Surface custom-targeting context on dependent line items | `gam_ad_unit_dependency_probe` | read | same as above | SOAP `LineItemService` | Key ids and value ids present on matched line items | No names or values are expanded in this helper | Missing XML fields, custom targeting absent, capped line item read | Tool Guide | implemented |
| Decide that an ad unit is safe to archive | none | unsupported proof | n/a | n/a | The MCP must not claim this from a bounded read alone | n/a | Any capped, blocked, status-filtered, or id-only dependency result remains incomplete | Tool Guide | deliberately unsupported |

## Certainty Contract

`gam_ad_unit_dependency_probe` returns a `dependency_decision`:

- `dependencies_found`: at least one line item or placement dependency was
  observed.
- `incomplete_no_dependencies_observed`: no dependency was observed, but a
  required proof surface was capped, blocked, sampled, or shape-incomplete.
- `no_dependencies_observed`: the exposed reads completed without observed
  dependencies, but this is not a cleanup approval if any incomplete proof flag
  is present.
- `missing_or_ambiguous_targets`: one or more requested ad-unit codes did not
  resolve exactly or resolved ambiguously.
- `blocked`: the helper could not perform the required upstream proof.

The tool also returns `proof_flags` so operators can distinguish a real absence
from a sample:

- `line_items_capped_or_truncated`
- `placements_capped_or_shape_unknown`
- `id_only_targets_have_unknown_ancestors`
- `soap_manage_scope_required`

The response includes a stable `result_fingerprint` over the bounded proof
payload. It can bind a later evidence-grading receipt, but it does not upgrade a
capped or blocked proof state. For one to ten resolved exact targets in the
requested network, the probe also returns a canonical caller-supplied receipt
template under the explicit `gam-evidence-producer-v1` source contract. An
operator must still review the underlying result before using it.

Target rows contribute dependency evidence only when their resource names bind
exactly to the canonical requested network and positive numeric ad-unit ID.
Ancestor, placement, and placement-member resource identities use the same
network-bound rule. Invalid nested identities cannot contribute dependencies
and keep the affected proof surface incomplete. Invalid SOAP API versions fail
before an upstream read. Caller-actionable permission and authentication faults
remain explicit permission blocks; authentication-service connection failures
remain upstream read blocks.

Paginated line-item proof is monotonic for positive evidence. When a later SOAP
plan, transport call, HTTP response, or SOAP fault blocks the scan, the result
retains all dependencies and progress confirmed on earlier pages. The surface
still reports `proof_state=blocked`; its decision is `dependencies_found` only
when an earlier page proved at least one dependency, and otherwise remains
`blocked`.

The helper is read-only and always reports `mutation_performed=false`.
