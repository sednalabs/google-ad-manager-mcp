# Decision 0006: Staged Ad-Unit Retirement Assessment

Ad-unit cleanup needs a decision surface that cannot turn a broad catalog
search, caller-supplied name, partial dependency sample, or missing evidence
into an archive recommendation. The assessment is therefore delivered in
sequential fail-closed stages. Each stage is useful on its own and keeps every
later proof surface explicit rather than implying it ran.

## Capability Matrix

| Workflow | Tool | Class | Inputs | Data source | Proof | Negative cases | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Resolve exact targets | `gam_ad_unit_retirement_assessment` | read | canonical network and 1-10 canonical exact ad-unit ids | REST `adUnits.get` | compact current identity, exact resource match, stable fingerprints | zero, whitespace, leading zeroes, duplicates, overflow, missing target, identity mismatch, permission or upstream failure | implemented |
| Reconcile descendants | same | read | bounded hierarchy scan | paginated REST `adUnits.list` | target/list reconciliation and descendant state | cap, pagination drift, sparse or cross-network ancestry | planned Stage 3; returns `not_run` now |
| Grade evidence | same | read decisioning | freshness-bound receipts | existing proof tools, reports, site contract, telemetry | source/network/target/version/hash/time binding | stale, capped, blocked, unsupported, duplicate, or mismatched receipts | planned Stage 4; returns `not_run` now |
| Return recommendation | same | read decisioning | complete identity, hierarchy, and evidence proof | staged assessment | operator-review recommendation only | any incomplete or blocked surface | planned Stage 5; returns `not_run` now |
| Archive or deactivate | none | mutation | not accepted | none | none | every call | out of scope |

## Exact-Identity Contract

The public schema and runtime both require canonical positive decimal strings
of at most 20 digits. Whitespace, zero, leading zeroes, duplicates, values that
overflow `u64`, empty target sets, and more than ten targets fail before any
provider call.

For each target, the adapter constructs exactly
`networks/<network>/adUnits/<id>` and calls the REST get endpoint. A response is
clear for identity only when its resource name and canonical id match the
request. The compact summary includes code, status, sizes, child flag, parent
id, and update time, but omits descriptions and display text. Provider errors
are mapped to bounded proof states without echoing raw provider details.

The response is capped at 5 KiB before the Contract V1 envelope is created. It
always reports no mutation, no authorization, and no safe-to-retire result.
Identity proof must not be treated as descendant, activity, protection, site,
telemetry, or operator-approval proof.
