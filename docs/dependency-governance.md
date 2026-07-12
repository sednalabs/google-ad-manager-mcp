# Dependency Governance

This starter ships a lightweight dependency-governance lane so a new public MCP
server can begin with the same basic posture as `mcp-toolkit-rs`:

- `cargo-deny` is blocking for advisories, licenses, bans, and sources.
- `cargo-audit` is blocking for RustSec advisories.
- `cargo-outdated` is advisory by default and blocking only when
  `STRICT_OUTDATED=1`.
- `scripts/rmcp_macro_runtime_pin_check.py` ensures any direct `rmcp-macros`
  dependency stays aligned with the pinned `rmcp` runtime, while allowing the
  recommended `rmcp`-with-`macros` setup.

Install the local tooling with:

```bash
cargo install cargo-deny cargo-audit cargo-outdated
```

Run the full check with:

```bash
./scripts/dependency_governance_check.sh
```

Use the stricter stale-dependency mode only when you want dependency freshness
to block the lane:

```bash
STRICT_OUTDATED=1 ./scripts/dependency_governance_check.sh
```

When you add a new direct dependency or make a major upgrade, include a short
PR note that records why the crate is needed, which safer alternatives were
considered, and how you would roll the change back.

## Toolkit and RMCP upgrade receipts

The semantic-discovery change advances all six direct toolkit packages together
from immutable revision
`679d7a4d93ba33f582ea8ac3f23e15e0c2d133f9` to
`f90934bd647d3d114ae3b651b11b58a0363c3bc4`. The interval is intentionally
documented as two compatibility and rollback domains even though it lands in
one pull request.

### Receipt 1: runtime, auth, and catalogue compatibility

- **Range:** `679d7a4d93ba33f582ea8ac3f23e15e0c2d133f9` through the
  pre-discovery commit `87d21ed9749d0178717ad6c464512080d1af3791`.
- **Impact:** RMCP moves from 1.8.0 to 2.1.0 and `rmcp-macros` from 1.8.0 to
  2.2.0. The toolkit range also includes Google ADC/OAuth hardening, shared
  response primitives, policy provenance, JWT-claim isolation, and explicit
  complete-catalogue collection.
- **Reason:** keep the server on one coherent toolkit revision and consume the
  reviewed runtime, authentication, catalogue, and testing contracts that the
  ranked-discovery API builds upon.
- **Alternatives considered:** remaining on the old pin would omit required
  catalogue/discovery contracts; copying selected toolkit code into this server
  would create a second security and maintenance authority; mixing toolkit
  package revisions or RMCP macro/runtime versions would be unsupported.
- **Compatibility proof:** exact server head
  `e52c68b2eac4ddc94402f504f5c6b4ea3e43940a` passed hosted
  [Rust baseline](https://github.com/sednalabs/google-ad-manager-mcp/actions/runs/29198446153),
  [Cargo package readiness](https://github.com/sednalabs/google-ad-manager-mcp/actions/runs/29198446138),
  and [dependency governance](https://github.com/sednalabs/google-ad-manager-mcp/actions/runs/29198446127).
  Those lanes cover compilation, Clippy, focused stdio/tool contracts, package
  installation, dependency policy, and the RMCP pin check.
- **Rollback:** restore every toolkit dependency and test dependency to the old
  immutable revision in one change, restore the corresponding lockfile and
  schema snapshot, and revert code that uses post-pin toolkit/RMCP APIs. Re-run
  the same hosted compatibility lanes before release. Do not roll back only one
  toolkit crate or only `rmcp-macros`.

### Receipt 2: ranked and bounded discovery

- **Range:** pre-discovery commit
  `87d21ed9749d0178717ad6c464512080d1af3791` to
  `f90934bd647d3d114ae3b651b11b58a0363c3bc4`.
- **Impact:** adds toolkit-owned deterministic ranking, query and metadata
  bounds, match-completeness diagnostics, OpenAI selection shaping, and compact
  response serialization. GAM owns only its tool vocabulary, workflow edges,
  provider-specific recovery, and final public-output policy.
- **Reason:** one shared ranking and compactness authority is safer and easier
  to improve than a GAM-only search implementation.
- **Alternatives considered:** pinning to `87d21ed` and implementing ranking,
  truncation, or compact serialization locally would duplicate generic logic;
  exposing the full tool catalogue without deferred discovery would preserve
  the original agent-ergonomics problem.
- **Compatibility proof:** the same exact-head hosted lanes above pass the
  complete semantic, dependency-edge, schema-union, redaction, result-bound,
  auth, and stdio contract suite.
- **Rollback:** remove the ranked `find_tools` integration and its discovery
  contracts, restore the prior tool schema snapshot, and pin every toolkit
  package to `87d21ed9749d0178717ad6c464512080d1af3791`. If the runtime/auth
  uplift must also be rolled back, follow Receipt 1 instead. Re-run hosted
  baseline, package readiness, and dependency governance before release.

## Auth dependency note

`mcp-toolkit-auth` is a direct dependency because this server delegates Google
ADC command construction, shell rendering, setup-plan generation, and common
Google auth error classification to the shared toolkit instead of maintaining
provider-specific copies.

The toolkit auth stack currently brings in OAuth/browser-login dependencies
including `reqwest` 0.13 and the platform-verifier WebPKI root bundle. The
`CDLA-Permissive-2.0` exception for `webpki-root-certs` matches the existing
exception for `webpki-roots`; keep both package-scoped rather than widening the
global license allowlist.

Rollback path: remove `mcp-toolkit-auth`, restore local gcloud command and
diagnostic helpers, regenerate `Cargo.lock`, and keep the MCP auth tool output
compatible with the documented setup flow.

## Scratchpad dependency note

`mcp-toolkit-scratchpad` is a direct dependency because this server exposes
bounded DuckDB scratchpad sessions for local analysis and evidence bundles.
That crate intentionally brings in DuckDB and Arrow transitively. Keeping the
scratchpad lifecycle in the toolkit is preferred over copying DuckDB session,
SQL policy, TTL, row-limit, and evidence-export code into each Google provider
MCP.

The `CC0-1.0` license is allowed because Arrow's hash-map path can pull in
`tiny-keccak` through `const-random` and `ahash`. Treat any future non-
permissive scratchpad transitive as a fresh review rather than widening the
allowlist automatically.

Rollback path: remove `mcp-toolkit-scratchpad`, remove the
`gam_scratchpad_*` tools, and regenerate `Cargo.lock` and the tool schema
snapshot. The core auth, network, catalog, and report tools do not depend on
DuckDB.
