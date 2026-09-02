# Google Ad Manager MCP

Use Google Ad Manager from an MCP client without giving the model a generic API
proxy or enabling live writes by default.

`google-ad-manager-mcp` is an Apache-2.0-licensed Rust stdio server built on the
official Google Ad Manager REST Beta and SOAP APIs. It gives MCP clients a
curated interface for:

- discovering networks, ad units, orders, line items, placements, private
  auctions, deals, and saved reports;
- running saved reports and working with paginated results;
- collecting read-only evidence for exchange exposure, dependencies, and
  ad-unit retirement decisions;
- previewing allowlisted REST and SOAP trafficking operations before any apply;
- applying an explicitly confirmed operation only when write mode and the
  required Google scope are both enabled; and
- analysing bounded catalogue, report, and SOAP readbacks in a local DuckDB
  scratchpad.

The server does not expose arbitrary REST requests, SOAP envelopes, SQL writes,
or live writes by default.

> [!IMPORTANT]
> This is a `0.x` project and the Google Ad Manager REST API it uses is Beta.
> [GitHub Releases](https://github.com/sednalabs/google-ad-manager-mcp/releases)
> is the source of truth for published versions and assets. At the time of this
> update, `v0.1.0` is the latest stable release; the `v0.1.1` source on `main`
> is release preparation, not a published stable binary.

## Quick start

You need:

- access to at least one Google Ad Manager network;
- a Google Cloud project with the Google Ad Manager API enabled;
- an MCP client that can start a local stdio server; and
- either `gcloud` for local user login or an authorised service-account JSON
  file.

Start with the read-only scope. SOAP operations—including SOAP reads and
forecasts—require the broader Ad Manager manage scope because the legacy SOAP
API does not accept the newer read-only scope.

### 1. Install

#### Current stable release

The current stable `v0.1.0` release provides bundles for Linux x86_64, macOS
Apple Silicon, and Windows x86_64. Download the matching archive plus
`SHA256SUMS` from the
[`v0.1.0` release](https://github.com/sednalabs/google-ad-manager-mcp/releases/tag/v0.1.0),
verify the archive checksum, extract it, and put `google-ad-manager-mcp` (or
`google-ad-manager-mcp.exe`) on your `PATH`.

With a Rust toolchain, the same immutable release can be installed from source:

```bash
cargo install --locked --git https://github.com/sednalabs/google-ad-manager-mcp \
  --tag v0.1.0 google-ad-manager-mcp
```

#### Checksum-verifying installer (`v0.1.1` and newer)

Starting with the release that publishes `install.sh` and `install.ps1`, the
release helpers select the platform bundle and verify its SHA-256 digest before
installing. Confirm those files appear in the selected release's asset list,
then run:

Linux or macOS:

```bash
curl --proto '=https' --tlsv1.2 -fsSLO \
  https://github.com/sednalabs/google-ad-manager-mcp/releases/latest/download/install.sh
sh install.sh
```

Windows PowerShell:

```powershell
Invoke-WebRequest https://github.com/sednalabs/google-ad-manager-mcp/releases/latest/download/install.ps1 -OutFile install.ps1
.\install.ps1
```

The release workflow targets Linux x86_64 and arm64, macOS Apple Silicon and
Intel, and Windows x86_64. Availability is still determined by the asset list
for the exact release you select. Pin an installer-supported version with
`sh install.sh --version vX.Y.Z` or `.\install.ps1 -Version vX.Y.Z`.

For release checksum and Sigstore provenance verification, see
[Releasing](docs/RELEASING.md#canonical-install-paths).

### 2. Authenticate

For local user credentials, the built-in helper keeps a server-specific
Application Default Credentials file so another Google integration cannot
silently replace this server's token or scope grant:

```bash
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID>
```

Omit `--headless` when the machine can open a browser. Then verify the token and
the low-cost `networks.list` access probe:

```bash
google-ad-manager-mcp auth status --verify-access
```

For unattended use, point the server at an authorised service-account file:

```bash
export GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH=/absolute/path/to/service-account.json
```

The Ad Manager network must grant the chosen user or service account the
required visibility. See [Getting started](docs/GETTING_STARTED.md) for raw
`gcloud`, shared ADC, quota-project, and manage-scope variants.

### 3. Connect an MCP client

Configure the installed executable as a local stdio server. Clients that accept
the common `mcpServers` JSON shape can use:

```json
{
  "mcpServers": {
    "google-ad-manager": {
      "command": "google-ad-manager-mcp"
    }
  }
}
```

If a graphical client does not inherit your shell `PATH`, use the absolute path
printed by the installer. On Windows that command normally ends in `.exe`.
Restart clients that keep a long-lived stdio child process after changing
credentials, configuration, or the installed binary.

For clients with a different configuration format, select the **stdio**
transport, use `google-ad-manager-mcp` as the command, and pass no arguments for
the default read-only/preview-only server.

### 4. Make the first calls

From the connected MCP client:

```text
gam_auth_status { "verify_token": true, "verify_access": true }
gam_networks_list {}
```

The first call reports token acquisition and live network access separately
without returning credential material. The second returns networks visible to
the authenticated principal. Continue with `gam_network_catalog_list` for
curated inventory or report discovery.

## Safe operating model

The default OAuth scope is:

```text
https://www.googleapis.com/auth/admanager.readonly
```

The default write mode is `preview_only`. A live apply requires all of the
following:

1. an allowlisted operation with a matching plan or preview;
2. the full `https://www.googleapis.com/auth/admanager` manage scope;
3. `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`; and
4. the exact confirmation token bound to the reviewed request.

Use a separate, operator-approved session for writes:

```bash
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> --manage-scope
```

Then add these settings to that MCP server process only:

```text
GOOGLE_AD_MANAGER_MCP_SCOPE=https://www.googleapis.com/auth/admanager
GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled
```

Planning remains non-mutating. Generic SOAP applies require follow-up
verification; the typed yield-group exclusion path additionally requires
post-apply readback before it reports success. The server never returns raw
access tokens, refresh tokens, private keys, or complete credential files.

See the [security model](docs/SECURITY_MODEL.md) before enabling live writes or
placing the server in an unattended environment.

## What to use next

| Goal | Start with |
| --- | --- |
| Check setup and credentials | `gam_get_started`, `gam_auth_status` |
| Discover inventory | `gam_networks_list`, `gam_network_catalog_list` |
| Run a saved report | `gam_report_run`, then poll or page only when needed |
| Assess ad-unit dependencies or retirement | `gam_ad_unit_dependency_probe`, `gam_ad_unit_retirement_assessment` |
| Inspect exchange, yield, or protection exposure | `gam_exchange_protection_probe` |
| Preview a REST change | `gam_trafficking_tool_matrix`, `gam_rest_write_plan` |
| Preview SOAP trafficking | `gam_soap_payload_build`, `gam_soap_trafficking_plan` |
| Analyse bounded data locally | `gam_scratchpad_open_session`, then an ingest tool and `gam_scratchpad_query` |
| Discover tools from natural language | `find_tools` |

All tools return structured Contract V1 envelopes: `ok/data/meta` on success
and `ok/error/meta` with the MCP error signal on failure. The
[tool guide](docs/TOOL_GUIDE.md) documents inputs, safety gates, pagination,
report replay rules, and complete workflows.

## Common configuration

| Setting | Default | Purpose |
| --- | --- | --- |
| `GOOGLE_AD_MANAGER_MCP_SCOPE` | Ad Manager read-only scope | OAuth scope requested from Google credentials |
| `GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT` | unset | Optional `x-goog-user-project` value |
| `GOOGLE_AD_MANAGER_MCP_SHARED_ADC` | `false` | Intentionally use conventional shared ADC instead of server-specific ADC |
| `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH` | unset | Server-specific service-account file |
| `GOOGLE_AD_MANAGER_MCP_HTTP_TIMEOUT_MS` | `15000` | Upstream request timeout |
| `GOOGLE_AD_MANAGER_MCP_WRITE_MODE` | `preview_only` | `read_only`, `preview_only`, or `enabled` |

Run `google-ad-manager-mcp --help` for the complete runtime surface and
`google-ad-manager-mcp auth --help` for credential commands.

## Troubleshooting

Start with the non-secret diagnostic:

```bash
google-ad-manager-mcp auth doctor --verify-access --json
```

If access fails, check that the API is enabled, the credential's Google Cloud
project is correct, the principal has access to the Ad Manager network, and the
quota project is available where required. The
[troubleshooting flow](docs/GETTING_STARTED.md#5-if-auth-looks-configured-but-access-still-fails)
has the full checklist.

## Update or uninstall

- To update an installer-managed copy, rerun the helper for a newer published
  release; it replaces the binary in the selected install directory.
- To update a source install, rerun `cargo install` with a newer exact release
  tag after reviewing that release.
- To uninstall an installer-managed copy, remove the single binary from the
  path printed during installation. The Unix default is
  `$XDG_BIN_HOME/google-ad-manager-mcp` or `~/.local/bin/google-ad-manager-mcp`;
  the Windows default is
  `%LOCALAPPDATA%\Programs\google-ad-manager-mcp\bin\google-ad-manager-mcp.exe`.
- To remove a Cargo-managed copy, run `cargo uninstall google-ad-manager-mcp`.

Credential files are not removed with the binary. Review and remove the
server-specific ADC or service-account file separately only when it is no
longer needed by any configured client.

## Documentation

- [Getting started](docs/GETTING_STARTED.md) — detailed authentication and
  first workflows
- [Tool guide](docs/TOOL_GUIDE.md) — complete tool contracts and operational
  guidance
- [Security model](docs/SECURITY_MODEL.md) — credentials, mutation gates, and
  trust boundaries
- [Architecture](docs/ARCHITECTURE.md) — implementation and extension model
- [Releasing](docs/RELEASING.md) — hosted release and provenance workflow
- [License](LICENSE) — Apache License 2.0
