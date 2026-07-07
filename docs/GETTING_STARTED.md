# Getting Started

## Goal

Get from zero to the first useful Ad Manager read, write preview, or guarded
SOAP trafficking plan without exposing secrets or guessing resource
identifiers.

## 1. Enable the API

Enable the Google Ad Manager API on the Google Cloud project you plan to use.

## 2. Authenticate The Easy Way

For local use, use the built-in helper:

```bash
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID>
```

The helper:

- runs the browser-based Google Application Default Credentials flow;
- writes a Google-Ad-Manager-specific ADC file by default so other Google MCPs
  keep their own tokens and scopes;
- includes the required ADC `cloud-platform` scope and the read-only Ad Manager
  scope;
- sets the ADC quota project when `--quota-project` is supplied;
- verifies access with a safe `networks.list` request.

Useful variants:

```bash
google-ad-manager-mcp auth login --quota-project <PROJECT_ID>
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> --manage-scope
google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> --shared-adc
google-ad-manager-mcp auth command --headless
google-ad-manager-mcp auth command --headless --manage-scope
google-ad-manager-mcp auth status --verify-token
google-ad-manager-mcp auth doctor --verify-token --json
```

If you prefer raw `gcloud`, set `CLOUDSDK_CONFIG` to the server-specific config
directory and use both scopes:

```bash
CLOUDSDK_CONFIG="$HOME/.config/google-ad-manager-mcp/gcloud" \
  gcloud auth application-default login \
  --scopes=https://www.googleapis.com/auth/cloud-platform,https://www.googleapis.com/auth/admanager.readonly
CLOUDSDK_CONFIG="$HOME/.config/google-ad-manager-mcp/gcloud" \
  gcloud auth application-default set-quota-project <PROJECT_ID>
```

For operator-approved write testing, use the manage scope instead:

```bash
CLOUDSDK_CONFIG="$HOME/.config/google-ad-manager-mcp/gcloud" \
  gcloud auth application-default login \
  --scopes=https://www.googleapis.com/auth/cloud-platform,https://www.googleapis.com/auth/admanager
CLOUDSDK_CONFIG="$HOME/.config/google-ad-manager-mcp/gcloud" \
  gcloud auth application-default set-quota-project <PROJECT_ID>
```

Use the manage-scope login for SOAP trafficking and forecasts too. Google
Ad Manager's legacy SOAP API requires the full
`https://www.googleapis.com/auth/admanager` scope, even for non-mutating SOAP
forecast/read calls.

For unattended use, prefer a service account file:

```bash
export GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json
```

Or set the server-specific secret path:

```bash
export GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH=/path/to/service-account.json
```

## 3. Start the server

If you installed from a tagged GitHub release bundle, unpack the archive,
verify it against `SHA256SUMS` and the attached
`SHA256SUMS.sigstore.json` bundle, and place the extracted binary on your
`PATH`.

```bash
google-ad-manager-mcp
```

By default the server uses `GOOGLE_AD_MANAGER_MCP_WRITE_MODE=preview_only`.
That allows write planning but denies live apply. For an operator-approved
apply session, start it with both the manage scope and enabled write mode:

```bash
GOOGLE_AD_MANAGER_MCP_SCOPE=https://www.googleapis.com/auth/admanager \
GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled \
google-ad-manager-mcp
```

Useful local inspection commands:

```bash
google-ad-manager-mcp --help
google-ad-manager-mcp auth --help
google-ad-manager-mcp --print-tools
google-ad-manager-mcp --print-tool-schema > spec/tool_schema_snapshot.v1.json
```

## 4. First MCP calls

1. `gam_auth_status` with `verify_access=true`
2. `gam_networks_list`
3. `gam_network_catalog_list`
4. `gam_exchange_protection_probe` when you need exchange/yield/protection
   proof for exact ad units

For reports:

1. `gam_network_catalog_list` with `collection="reports"`
2. `gam_report_run`
3. `gam_report_result_rows` when pagination is needed

For REST write planning:

1. `gam_trafficking_tool_matrix`
2. `gam_rest_write_plan`
3. `gam_rest_write_apply` only after enabling write mode, using the manage
   scope, and passing the exact confirmation token from the plan

For SOAP trafficking:

1. `gam_trafficking_tool_matrix`
2. `gam_soap_payload_build` when one of the supported templates matches the
   operation
3. `gam_soap_trafficking_plan`
4. `gam_soap_trafficking_apply` with the exact confirmation token from the
   plan

SOAP forecast/read operations can run without write mode enabled, but still
need the manage scope. Mutating SOAP operations also require
`GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled`, `expected_impact`, and
`rollback_note`.

For scratchpad analysis:

1. `gam_scratchpad_open_session`
2. `gam_scratchpad_ingest_network_catalog` or
   `gam_scratchpad_ingest_report_result_rows`
3. `gam_scratchpad_query`
4. `gam_scratchpad_export_evidence_bundle`

## 5. If auth looks configured but access still fails

Check these in order:

1. The Ad Manager API is enabled on the Google Cloud project.
2. The Google principal actually has access to the target Ad Manager network.
3. If you are using a service account, the network has granted that service
   account user the needed Ad Manager visibility.
4. If you are using user ADC, the server-specific ADC quota project is set and
   `GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT` is present when the server needs an
   `x-goog-user-project` header.
5. Restart the MCP client if it keeps a long-lived stdio subprocess.

The server sends `x-goog-user-project` only when
`GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT` is set in the MCP server environment.
`gcloud auth application-default set-quota-project` is still part of the easy
ADC login path for Google tooling. The auth helper applies it to the
server-specific ADC file by default.
