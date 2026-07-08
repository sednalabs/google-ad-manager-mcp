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

## Auth dependency note

`mcp-toolkit-auth` is a direct dependency because this server delegates Google
ADC command construction, shell rendering, setup-plan generation, and common
Google auth error classification to the shared toolkit instead of maintaining
provider-specific copies. The MCP auth helper uses
`GoogleProviderAuthConfig::adc_login_command_contract()` for the shared
`gam_auth_login_command` shape, while `gam_auth_status` uses
`ProviderAuthCheckStatus` and `ProviderQuotaProjectStatus` for token/access
checks and ADC/runtime quota-project diagnostics.

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
