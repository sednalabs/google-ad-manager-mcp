# google-ad-manager-mcp

`google-ad-manager-mcp` is a public Rust stdio MCP server for read-only Google
Ad Manager workflows. It is built on `mcp-toolkit-rs` and the official Google
Ad Manager API (Beta).

The first public release focuses on the smallest useful surface:

- inspect Google Ad Manager credential readiness without exposing secrets;
- discover accessible Ad Manager networks;
- list curated network collections:
  - ad units
  - orders
  - line items
  - saved reports
- run saved reports and fetch paginated result rows.

The server intentionally does not expose a generic HTTP proxy, arbitrary query
surface, or default write operations.

## Documentation

- [Getting started](docs/GETTING_STARTED.md)
- [Tool guide](docs/TOOL_GUIDE.md)
- [Security model](docs/SECURITY_MODEL.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Decision 0001: Beta REST read-only first](docs/decision-0001-beta-rest-read-only.md)
- [Releasing](docs/RELEASING.md)

## Install

The lowest-friction install path today is:

```bash
cargo install --locked --git https://github.com/sednalabs/google-ad-manager-mcp google-ad-manager-mcp
```

For a pinned tagged source install:

```bash
cargo install --locked --git https://github.com/sednalabs/google-ad-manager-mcp --tag v0.1.0 google-ad-manager-mcp
```

The repository also publishes GitHub-hosted binary bundles through the release
workflow and a Linux artifact on `main` through the `Linux Artifact` workflow.
Those hosted artifacts are useful when you want a pinned binary plus SHA256
manifests and a Sigstore verification bundle from hosted compute rather than a
local `cargo install`.

## First Run

The server exposes setup tools that do not return secrets:

- `gam_get_started`
- `gam_auth_status`
- `gam_auth_login_command`

For local use, the normal path is:

```bash
gcloud auth application-default login \
  --scopes=https://www.googleapis.com/auth/admanager.readonly
gcloud auth application-default set-quota-project <PROJECT_ID>
```

Then restart any stdio MCP client that keeps a long-lived child process and
call:

```text
gam_auth_status { "verify_access": true }
```

After auth is proven:

1. `gam_networks_list`
2. `gam_network_catalog_list`
3. `gam_report_run`
4. `gam_report_result_rows` when a report result has more pages

## Authentication

The initial public release defaults to the read-only Ad Manager scope:

```text
https://www.googleapis.com/auth/admanager.readonly
```

Supported credential sources:

- Application Default Credentials from `gcloud auth application-default login`
- Standard Google credential file via `GOOGLE_APPLICATION_CREDENTIALS`
- Server-specific service account file via
  `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH`
- Server-specific raw service account JSON via
  `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON`

If you already have a service account for the Google Ad Manager SOAP API, the
official Ad Manager Beta docs say you can reuse it after enabling the Ad
Manager API on the Google Cloud project tied to that credential.

The server never returns raw access tokens, private keys, refresh tokens, or
whole credential files in tool responses.

## Configuration

| Setting | Default | Purpose |
| --- | --- | --- |
| `GOOGLE_AD_MANAGER_MCP_SCOPE` | `https://www.googleapis.com/auth/admanager.readonly` | OAuth scope requested from Google credentials |
| `GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT` | unset | Optional `x-goog-user-project` header |
| `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH` | unset | Server-specific service-account credential path |
| `GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON` | unset | Server-specific raw service-account JSON |
| `GOOGLE_AD_MANAGER_MCP_HTTP_TIMEOUT_MS` | `15000` | Upstream request timeout |
| `GOOGLE_AD_MANAGER_MCP_API_BASE_URL` | `https://admanager.googleapis.com/v1` | Upstream API root |
| `GOOGLE_AD_MANAGER_MCP_REPORT_POLL_TIMEOUT_MS` | `300000` | Default report wait timeout |
| `GOOGLE_AD_MANAGER_MCP_REPORT_POLL_INITIAL_INTERVAL_MS` | `5000` | Initial report polling interval |

## Tools

- `find_tools`
- `gam_get_started`
- `gam_auth_status`
- `gam_auth_login_command`
- `gam_networks_list`
- `gam_network_catalog_list`
- `gam_report_run`
- `gam_report_result_rows`

All tool responses use Contract V1 envelopes:

```json
{
  "ok": true,
  "data": {},
  "meta": {
    "elapsed_ms": 12
  }
}
```

## Upstream Scope

This server is intentionally shaped around the official Google Ad Manager API
(Beta) rather than the legacy SOAP API. The public v1 keeps the surface small,
read-only, and auditable. If a later slice needs a SOAP-only read path, it
should be isolated behind a documented adapter boundary instead of broadening
the existing tool surface into a generic proxy.
