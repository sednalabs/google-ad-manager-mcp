# Google Ad Manager MCP

Use Google Ad Manager from an MCP client without giving the model a generic API
proxy.

`google-ad-manager-mcp` is an Apache-2.0-licensed Rust stdio server built on the
official Google Ad Manager APIs. It exposes a curated, structured interface
rather than arbitrary HTTP, SOAP, or SQL execution.

## Release status

This is a `0.x` project and the Google Ad Manager REST API it uses is Beta.
[GitHub Releases](https://github.com/sednalabs/google-ad-manager-mcp/releases)
is the source of truth for published versions and assets.

| Channel | Status | Surface |
| --- | --- | --- |
| `v0.1.0` | Current stable release | Read-only auth inspection, networks, curated catalogues, saved-report runs, and result pagination |
| `v0.1.1` source on `main` | Release preparation; not yet a published stable binary | Adds built-in auth commands, guarded REST/SOAP workflows, evidence probes, and the local scratchpad |

The stable quick start below intentionally documents `v0.1.0`. Do not assume
the broader `v0.1.1` surface is installed until GitHub marks that exact release
as published.

## Stable `v0.1.0` quick start

You need:

- access to at least one Google Ad Manager network;
- a Google Cloud project with the Google Ad Manager API enabled;
- `gcloud` or an authorised service-account JSON file; and
- an MCP client that can start a local stdio server.

## Install

### Prebuilt `v0.1.0` binary

The current stable release provides bundles for Linux x86_64, macOS Apple
Silicon, and Windows x86_64. Download the matching archive plus `SHA256SUMS`
from the
[`v0.1.0` release](https://github.com/sednalabs/google-ad-manager-mcp/releases/tag/v0.1.0),
verify the archive checksum, extract it, and put `google-ad-manager-mcp` (or
`google-ad-manager-mcp.exe`) on your `PATH`.

### Install `v0.1.0` from source

With a Rust toolchain:

```bash
cargo install --locked --git https://github.com/sednalabs/google-ad-manager-mcp \
  --tag v0.1.0 google-ad-manager-mcp
```

### Authenticate `v0.1.0`

For local user credentials, request Application Default Credentials with the
read-only Ad Manager scope:

```bash
gcloud auth application-default login \
  --scopes=https://www.googleapis.com/auth/admanager.readonly
gcloud auth application-default set-quota-project <PROJECT_ID>
```

For unattended use, point the server at an authorised service-account file:

```bash
export GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH=/absolute/path/to/service-account.json
```

The Ad Manager network must grant the chosen user or service account the
required visibility.

### Connect an MCP client

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

If a graphical client does not inherit your shell `PATH`, use the absolute
binary path. On Windows that command normally ends in `.exe`. For clients with
a different configuration format, select the **stdio** transport, use the
binary as the command, and pass no arguments.

Restart clients that keep a long-lived stdio child process after changing
credentials, configuration, or the installed binary.

### Make the first `v0.1.0` calls

From the connected MCP client:

```text
gam_auth_status { "verify_access": true }
gam_networks_list {}
gam_network_catalog_list { "collection": "ad_units", "network_code": "<NETWORK_CODE>" }
```

The auth call performs a low-cost `networks.list` access probe without
returning credential material. The stable release exposes eight tools:

- `find_tools`
- `gam_get_started`
- `gam_auth_status`
- `gam_auth_login_command`
- `gam_networks_list`
- `gam_network_catalog_list`
- `gam_report_run`
- `gam_report_result_rows`

See the [`v0.1.0` README](https://github.com/sednalabs/google-ad-manager-mcp/blob/v0.1.0/README.md)
for the version-matched configuration and report flow.

## `v0.1.1` release candidate

The following features and commands describe the current source candidate.
Use them only after `v0.1.1` is published, or when deliberately evaluating an
unreleased source checkout.

### Checksum-verifying release installer

The `v0.1.1` release workflow adds checksum-verifying installers and targets
Linux x86_64 and arm64, macOS Apple Silicon and Intel, and Windows x86_64.
Availability is determined by the asset list for the exact release.

After the selected release publishes `install.sh` or `install.ps1`:

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

The helpers select the platform bundle and verify its SHA-256 digest before
installation. Pin an installer-supported release with
`sh install.sh --version vX.Y.Z` or `.\install.ps1 -Version vX.Y.Z`.

For checksum and Sigstore provenance verification, see
[Releasing](docs/RELEASING.md#canonical-install-paths).

### Built-in authentication

The candidate's helper uses a server-specific Application Default Credentials
file so another Google integration cannot silently replace this server's token
or scope grant:

```bash
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID>
google-ad-manager-mcp auth status --verify-access
```

Omit `--headless` when the machine can open a browser. The status command
reports token acquisition and live network access separately without returning
credential material. See [Getting started](docs/GETTING_STARTED.md) for raw
`gcloud`, shared ADC, quota-project, service-account, and manage-scope variants.

### Candidate capabilities

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
report replay rules, and complete candidate workflows.

## Security and write controls

The stable `v0.1.0` release is read-only. The `v0.1.1` candidate defaults to
the read-only OAuth scope and `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=preview_only`.
Planning remains non-mutating.

A **mutating** live apply in the candidate requires all of the following:

1. an allowlisted operation with a matching plan or preview;
2. the full `https://www.googleapis.com/auth/admanager` manage scope;
3. `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`; and
4. the exact confirmation token bound to the reviewed request.

Use a separate, operator-approved session for mutations:

```bash
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> --manage-scope
```

Then add these settings to that MCP server process only:

```text
GOOGLE_AD_MANAGER_MCP_SCOPE=https://www.googleapis.com/auth/admanager
GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled
```

Non-mutating SOAP forecasts and reads also require the manage scope, but do not
require write mode. Generic SOAP mutations require follow-up verification; the
typed yield-group exclusion path additionally requires post-apply readback
before reporting success.

The server never returns raw access tokens, refresh tokens, private keys, or
complete credential files. See the [security model](docs/SECURITY_MODEL.md)
before enabling live writes or placing the server in an unattended environment.

## Configuration by version

| Setting | `v0.1.0` | `v0.1.1` candidate | Purpose |
| --- | --- | --- | --- |
| `GOOGLE_APPLICATION_CREDENTIALS` | supported | supported for service accounts | Standard Google credential file |
| `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH` | supported | supported | Server-specific service-account file |
| `GOOGLE_AD_MANAGER_MCP_SCOPE` | read-only by default | read-only by default | OAuth scope requested from Google credentials |
| `GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT` | optional | optional | `x-goog-user-project` value |
| `GOOGLE_AD_MANAGER_MCP_SHARED_ADC` | unavailable | `false` by default | Intentionally use conventional shared ADC |
| `GOOGLE_AD_MANAGER_MCP_WRITE_MODE` | unavailable; release is read-only | `preview_only` by default | `read_only`, `preview_only`, or `enabled` |
| `GOOGLE_AD_MANAGER_MCP_HTTP_TIMEOUT_MS` | `15000` | `15000` | Upstream request timeout |

Run `google-ad-manager-mcp --help` for the options supported by the installed
version. On `v0.1.1` and newer, use `google-ad-manager-mcp auth --help` for
credential commands.

## Troubleshooting

For stable `v0.1.0`, call `gam_auth_status` with `verify_access=true`, then
check that the API is enabled, the credential's Google Cloud project is
correct, the principal has access to the Ad Manager network, and the quota
project is available where required.

On the `v0.1.1` candidate, start with the non-secret diagnostic:

```bash
google-ad-manager-mcp auth doctor --verify-access --json
```

The [troubleshooting flow](docs/GETTING_STARTED.md#5-if-auth-looks-configured-but-access-still-fails)
has the full candidate checklist.

## Update or uninstall

- To update an installer-managed copy, rerun the helper for a newer published
  release; it replaces the binary in the selected install directory.
- To update a source install, rerun `cargo install` with a newer exact release
  tag after reviewing that release.
- To uninstall an installer-managed copy, remove the single binary from the
  path printed during installation. The Unix installer default is
  `$XDG_BIN_HOME/google-ad-manager-mcp` or `~/.local/bin/google-ad-manager-mcp`;
  the Windows installer default is
  `%LOCALAPPDATA%\Programs\google-ad-manager-mcp\bin\google-ad-manager-mcp.exe`.
- To remove a Cargo-managed copy, run `cargo uninstall google-ad-manager-mcp`.

Credential files are not removed with the binary. Review and remove a shared
ADC, server-specific ADC, or service-account file separately only when it is no
longer needed by any configured client.

## Documentation

- [Getting started](docs/GETTING_STARTED.md) — detailed `v0.1.1` candidate
  authentication and first workflows
- [Tool guide](docs/TOOL_GUIDE.md) — complete candidate tool contracts and
  operational guidance
- [Security model](docs/SECURITY_MODEL.md) — credentials, mutation gates, and
  trust boundaries
- [Architecture](docs/ARCHITECTURE.md) — implementation and extension model
- [Releasing](docs/RELEASING.md) — hosted release and provenance workflow
- [License](LICENSE) — Apache License 2.0
