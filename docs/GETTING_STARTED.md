# Getting Started

## Goal

Get from zero to the first useful Ad Manager read without exposing secrets or
guessing resource identifiers.

## 1. Enable the API

Enable the Google Ad Manager API on the Google Cloud project you plan to use.

## 2. Authenticate

For local use, the standard path is Application Default Credentials with the
read-only scope:

```bash
gcloud auth application-default login \
  --scopes=https://www.googleapis.com/auth/admanager.readonly
gcloud auth application-default set-quota-project <PROJECT_ID>
```

For unattended use, prefer a service account file:

```bash
export GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json
```

Or set the server-specific secret path:

```bash
export GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH=/path/to/service-account.json
```

## 3. Start the server

```bash
google-ad-manager-mcp
```

Useful local inspection commands:

```bash
google-ad-manager-mcp --print-tools
google-ad-manager-mcp --print-tool-schema > spec/tool_schema_snapshot.v1.json
```

## 4. First MCP calls

1. `gam_auth_status` with `verify_access=true`
2. `gam_networks_list`
3. `gam_network_catalog_list`

For reports:

1. `gam_network_catalog_list` with `collection="reports"`
2. `gam_report_run`
3. `gam_report_result_rows` when pagination is needed

## 5. If auth looks configured but access still fails

Check these in order:

1. The Ad Manager API is enabled on the Google Cloud project.
2. The Google principal actually has access to the target Ad Manager network.
3. If you are using a service account, the network has granted that service
   account user the needed Ad Manager visibility.
4. If you are using user ADC, the quota project is set on the ADC credential.
5. Restart the MCP client if it keeps a long-lived stdio subprocess.
