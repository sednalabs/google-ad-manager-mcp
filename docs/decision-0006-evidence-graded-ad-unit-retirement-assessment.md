# Decision 0006: Evidence-Graded Ad-Unit Retirement Assessment

Ad-unit cleanup needs a decision surface that cannot turn a bounded dependency
sample, an old delivery report, sparse hierarchy data, or an unsupported GAM
protection surface into an archive recommendation. The assessment therefore
binds exact ad-unit ids to current REST identity, reconciles catalog ancestry,
and grades bounded caller-supplied evidence receipts without authorizing a
mutation.

## Capability Matrix

| Workflow | Tool | Class | Inputs | Data Source | Proof | Fail-closed boundary | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Resolve exact targets | `gam_ad_unit_retirement_assessment` | read | canonical network and one to ten exact ad-unit ids | REST `adUnits.get` | compact current identity and fingerprint | missing/changed identity or permission failure blocks completion | implemented |
| Reconcile descendants | same | read | bounded row/page limits | paginated REST `adUnits.list` | target/list reconciliation, parent-path consistency, external descendants, child-first order | cap, zero-progress, repeated token, missing target row, sparse/cross-network ancestry, or child-flag mismatch remains partial | implemented |
| Grade evidence | same | read decisioning | at most one receipt per dependency, delivery, exchange/protection, site-contract, and telemetry source | caller-supplied summaries plus dependency/protection templates | network/source/version/target/hash/time/TTL binding; recent activity windows | invalid, stale, short, capped, blocked, unsupported, missing, or duplicate receipts remain incomplete | implemented |
| Return a recommendation | same | read decisioning | current inventory proof plus graded evidence | hybrid current REST proof and supplied receipts | blocked, incomplete, or `evidence_complete_operator_review_required` | never verifies operator identity, authorizes retirement, or mutates GAM | implemented |

## Evidence Contract

The tool accepts only exact numeric ad-unit ids. Broad candidate discovery and
code/name lookup remain separate workflows. Evidence is a list of no more than
five receipts, with at most one receipt for each required source. Each receipt
must bind the same canonical network and target set, the expected source and
source version, an opaque result hash, observation epoch, and bounded TTL.
Accepted source contracts are the running MCP package version for
`dependency_probe` and `exchange_protection_review`, `gam-report-v1` for
`delivery_report`, `site-contract-v1` for `site_contract`, and `telemetry-v1`
for `telemetry`. Unknown versions fail closed instead of being treated as
compatible evidence.

Delivery and telemetry receipts must cover a non-zero activity window of at
least 30 days. Both the observation and window end must remain inside the
receipt TTL, so running a fresh report over an old period cannot clear the
surface. Protection evidence generated as `manual_ui_proof_required` becomes
`complete_clear` only when the caller records completion of the required GAM
UI review in that same receipt. Partial API proof cannot be upgraded by that
flag.

Receipts are always `caller_supplied_unverified`. The MCP process cannot verify
the identity of a human who supplied or edited one, so an all-clear bundle
returns `evidence_complete_operator_review_required`, never a reviewed or
automatically eligible retirement decision. Explicit operator approval and any
guarded write remain separate workflows.

Parent and child targets may be assessed together, but the response records a
required child-first order. Inner assessment data is capped at 5 KiB, each
model-visible structured result at 8 KiB, and the final serialized RMCP result
at 20 KiB to account for the protocol's duplicate text representation. The
dependency and exchange/protection proof tools use the same item and wire caps.
Over-limit results fail closed and ask the operator to narrow targets or page
limits, omit optional raw XML, or ingest the underlying evidence into the
scratchpad.
