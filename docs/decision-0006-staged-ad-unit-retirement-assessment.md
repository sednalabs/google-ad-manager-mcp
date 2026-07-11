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
| Reconcile descendants | same | read | bounded hierarchy scan | paginated REST `adUnits.list` | target/list reconciliation, complete root-to-parent validation, bidirectional child flags, descendant state, and child-first order | byte/row/page cap, pagination or order drift, malformed rows, sparse or cross-network ancestry | implemented |
| Grade evidence | same | read decisioning | freshness-bound receipts | existing proof tools, reports, site contract, telemetry | source/network/target/version/hash/time binding | stale, capped, blocked, unsupported, duplicate, or mismatched receipts | planned Stage 4; returns `not_run` now |
| Return recommendation | same | read decisioning | complete identity, hierarchy, and evidence proof | staged assessment | operator-review recommendation only | any incomplete or blocked surface | planned Stage 5; returns `not_run` now |
| Archive or deactivate | none | mutation | not accepted | none | none | every call | out of scope |

## Exact-Identity Contract

The public schema and runtime both require canonical positive signed-64-bit
decimal strings. Whitespace, zero, leading zeroes, duplicates, values above
`i64::MAX`, empty target sets, and more than ten targets fail before any
provider call.

For each target, the adapter constructs exactly
`networks/<network>/adUnits/<id>` and calls the REST get endpoint. A response is
clear for identity only when its resource name and canonical id match the
request, required identity fields are present, and any parent resource is a
canonical ad-unit resource in the same network. The compact summary includes
code, status, a bounded size projection with source counts and a complete size
source fingerprint, child flag, parent id, and update time, but omits
descriptions and display text. Provider errors are mapped to bounded proof
states without echoing raw provider details. Request metadata distinguishes
pre-authentication failures from attempted provider reads.

Ad-unit sizes require the official `BROWSER` or `VIDEO_PLAYER` environment;
companions are valid only for `VIDEO_PLAYER`. The full size source, including
tail entries omitted from the compact projection, remains fingerprint-bound.
A clear identity also requires an official usable status: `ACTIVE`, `INACTIVE`,
or `ARCHIVED`.
A batch with a confirmed blocker plus any unread or incomplete target is
`partial_blocked`; `complete_blocked` requires every other target to be fully
read as clear or blocked.

The inner response is capped at 7 KiB. The adapter also measures the complete
Contract V1 model-visible content and serialized RMCP result against their
advertised 8 KiB and 20 KiB limits. It always reports no mutation, no
authorization, and no safe-to-retire result.
Identity proof must not be treated as descendant, activity, protection, site,
telemetry, or operator-approval proof.

## Hierarchy And Descendant Contract

After exact identity reads, the adapter requests the complete ad-unit catalog
with a fixed page size and `orderBy=name`. The public `ad_unit_page_size` input
defaults to 1000 and is capped at 1000; `max_ad_units` defaults to 5000 and is
capped at 10000. The scan also caps pages at 100, each upstream response at
2 MiB before JSON decoding, and the aggregate response budget at 16 MiB.

Every page must contain an `adUnits` array and a valid optional continuation
token. Every row must contain an exact same-network canonical resource name,
an official status, a boolean `hasChildren`, and a complete `parentPath` from
root through the direct parent. Rows must remain strictly ordered across page
boundaries. Missing or malformed final pages, repeated tokens, zero-progress
pages, duplicate ids, catalog gaps, cycles, cross-network parents, incomplete
paths, or exceeded caps keep proof incomplete.

The adapter reconstructs direct children from the catalog and compares that
state bidirectionally with each listed `hasChildren` flag. Target flags are
also reconciled against the exact GET responses. External active or inactive
descendants are positive blockers even when a later page or request fails;
that state is `partial_blocked`, not a lost or falsely clear result. Archived
external descendants are reported but do not block. When assessed targets are
ancestors of other assessed targets, the response returns a deterministic
child-first order. Output contains only counts, a bounded sample, issue codes,
and fingerprints rather than the complete catalog.
