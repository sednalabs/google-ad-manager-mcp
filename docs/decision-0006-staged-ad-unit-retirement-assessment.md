# Decision 0006: Evidence-Graded Ad-Unit Retirement Assessment

Ad-unit cleanup needs a decision surface that cannot turn a broad catalog
search, caller-supplied name, partial dependency sample, or missing evidence
into an archive recommendation. The assessment is therefore delivered in
sequential fail-closed stages. Each stage is useful on its own and keeps every
later proof surface explicit rather than implying it ran.

## Capability Matrix

| Workflow | Tool | Class | Inputs | Data source | Proof | Negative cases | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Resolve exact targets | `gam_ad_unit_retirement_assessment` | read | canonical network and 1-10 canonical exact ad-unit ids | REST `adUnits.get` | compact current identity, exact resource match, stable fingerprints | zero, whitespace, leading zeroes, duplicates, overflow, missing target, identity mismatch, permission or upstream failure | implemented |
| Reconcile descendants | same | read | bounded hierarchy scan | paginated REST `adUnits.list` | target/list reconciliation, catalog-proven root-to-parent validation, bidirectional child flags, descendant state, and child-first order | byte/row/page cap, pagination or numeric-id order drift, malformed rows, sparse or cross-network ancestry | implemented |
| Grade evidence | same | read decisioning | freshness-bound receipts | existing proof tools, reports, site contract, telemetry | source/network/target/version/hash/time binding | stale, capped, blocked, unsupported, duplicate, or mismatched receipts | implemented |
| Return recommendation | same | read decisioning | complete identity, hierarchy, and evidence proof | staged assessment | blocked, incomplete, or operator-review-required decision | any incomplete or blocked surface | implemented |
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
Canonical target ids are carried once per target and in the aggregate target
list; the derived resource name and duplicate id are omitted from each compact
current-identity object.

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
advertised 8 KiB and 20 KiB limits. Every successful assessment reports no
mutation, no authorization, and no safe-to-retire result.
Identity proof must not be treated as descendant, activity, protection, site,
telemetry, or operator-approval proof.

## Hierarchy And Descendant Contract

After exact identity reads, the adapter requests the complete ad-unit catalog
with a fixed minimal hierarchy field mask, page size, and `orderBy=name`. The public `ad_unit_page_size` input
defaults to 1000 and is capped at 1000; `max_ad_units` defaults to 5000 and is
capped at 10000. The scan also caps pages at 100, each upstream response at
2 MiB before JSON decoding, and the aggregate response budget at 16 MiB.

Every page must contain an `adUnits` array and a valid optional continuation
token. Every row must contain an exact same-network canonical resource name,
an official status, and a boolean `hasChildren`. Rows must remain ordered by
their signed 64-bit numeric ad-unit id across page boundaries. An exact network
read binds `effectiveRootAdUnit`; an exact GET of that ad unit binds its
Google-created parent. The adapter reconstructs the complete root-to-parent
chain from direct parents and accepts either the documented root-inclusive
`parentPath` or the root-omitted form observed on some networks. The latter
clears only when the sole catalog root and effective-root relationship match
those authoritative reads. Missing or malformed final pages, repeated tokens,
zero-progress pages, duplicate ids, catalog gaps, cycles, cross-network
parents, incomplete deeper paths, or exceeded caps keep proof incomplete.

The adapter reconstructs direct children from the catalog and compares that
state bidirectionally with each listed `hasChildren` flag. Target flags are
also reconciled against the exact GET responses. External active or inactive
descendants are positive blockers even when a later page or request fails;
that state is `partial_blocked`, not a lost or falsely clear result. Archived
external descendants are reported but do not block. When assessed targets are
ancestors of other assessed targets, the response returns a deterministic
child-first order. Output contains only counts, a bounded sample, issue codes,
and fingerprints rather than the complete catalog.

## External Evidence Contract

The assessment accepts at most one receipt for each dependency, delivery,
exchange/protection, site-contract, and telemetry source. Receipts remain
`caller_supplied_unverified`: the adapter checks their structure and binding,
but does not claim to verify operator identity or reconstruct the source result
from its hash.

Every non-`not_run` receipt must use the exact assessed network and complete
canonical target set, a supported source version, a bounded opaque result hash,
an observation timestamp no more than five minutes in the future, and a
positive TTL no greater than 31 days. Dependency and exchange/protection
receipts use the current evidence-producer contract version. Delivery,
site-contract, and telemetry receipts use their explicit V1 contracts.
Built-in producer receipts must also preserve the producer's exact 16-character
lowercase hexadecimal fingerprint, 3600-second TTL, fixed provenance and
non-authorisation metadata, and source-possible state.
Delivery and telemetry additionally require a non-zero activity window of at
least 30 days whose end does not exceed the observation time. Freshness uses
the older of the observation and activity-window end.

Duplicate sources reject the request. Missing sources remain `not_run`.
Unknown versions, stale observations, invalid hashes, malformed ids, different
target sets, and cross-network receipts return bounded incomplete grading.
Positive partial states remain visible but cannot make the evidence summary
complete. Exchange/protection clear proof also requires
`manual_ui_proof_included=true` while relevant protection surfaces remain
UI-only. Optional notes are bounded, validated, and never echoed.
Unknown receipt fields are rejected, and freshness is evaluated after all live
identity and hierarchy reads complete.

## Final Recommendation Contract

The final read-only decision has three states. Positive blockers take
precedence: `complete_blocked`, `partial_blocked`, or an observed non-archived
external descendant returns `blocked_by_current_state_or_evidence`, even when
another surface is missing or capped. With no confirmed blocker, any non-clear
surface returns `not_eligible_incomplete_evidence`. Only seven complete-clear
surfaces return `evidence_complete_operator_review_required`: current identity,
descendants, dependency, delivery, exchange/protection, site contract, and
telemetry.

The recommendation repeats the assessment fingerprint, whose preimage includes
the versioned recommendation contract as well as the network, targets, current
identity, hierarchy, and evidence. This prevents a later decision-contract
revision from reusing an earlier-stage fingerprint. It preserves the
deterministic child-first target order and supplies bounded, surface-aware
actions for every blocked or incomplete surface. Identity-shape and hierarchy
reconciliation failures never tell an operator to attach an unrelated receipt.
Adjustable row caps, hard catalog limits, and structural hierarchy failures have
distinct guidance. External non-archived descendants remain blockers, but the
tool requires each one to be separately assessed as an exact target before it
suggests any disposition. `operator_review_required` remains true,
`automated_retirement_eligible` and `safe_to_archive_or_retire` remain false,
and the response explicitly states that it is not an archive authorization.
No archive, deactivate, rename, retarget, or other GAM mutation is exposed or
performed by this workflow.
