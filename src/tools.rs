use std::time::Instant;

use mcp_toolkit::rmcp::handler::server::wrapper::Parameters;
use mcp_toolkit::rmcp::model::CallToolResult;
use mcp_toolkit::rmcp::{self, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::client::CatalogCollection;
use crate::contract;
use crate::{AdManagerError, AdManagerServer, MANAGE_SCOPE, McpError};
use mcp_toolkit_core::tool_inventory::{ToolOperation, ToolSearchFilter, ToolSearchResponse};

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetStartedArgs {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AuthStatusArgs {
    /// When true, make a live low-cost Ad Manager API call to prove access.
    #[serde(default)]
    pub verify_access: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindToolsArgs {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_schema: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AuthLoginCommandArgs {
    /// Optional downloaded OAuth client JSON path for gcloud ADC login.
    pub client_id_file: Option<String>,
    /// Optional quota project to set after login.
    pub quota_project: Option<String>,
    /// Include --no-launch-browser for headless flows. Defaults to true.
    pub no_launch_browser: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NetworksListArgs {
    pub page_size: Option<u32>,
    pub page_token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NetworkCatalogListArgs {
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// Which curated collection to list.
    pub collection: CatalogCollection,
    pub page_size: Option<u32>,
    pub page_token: Option<String>,
    /// Optional Ad Manager Beta filter expression.
    pub filter: Option<String>,
    /// Optional Ad Manager Beta orderBy expression.
    pub order_by: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReportRunArgs {
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// Numeric report identifier from the Ad Manager UI or reports.list.
    pub report_id: String,
    /// Wait for the report operation to complete. Defaults to true.
    pub wait_for_completion: Option<bool>,
    /// If waiting, also fetch the first page of result rows. Defaults to true.
    pub fetch_first_page: Option<bool>,
    /// Optional first-page result row cap.
    pub result_page_size: Option<u32>,
    /// Optional polling timeout override.
    pub poll_timeout_ms: Option<u64>,
    /// Optional initial poll interval override.
    pub initial_poll_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReportResultRowsArgs {
    /// Result resource name, for example networks/123/reports/456/results/789.
    pub result_name: String,
    pub page_size: Option<u32>,
    pub page_token: Option<String>,
}

#[tool_router(router = tool_router_ad_manager, vis = "pub")]
impl AdManagerServer {
    #[tool(
        name = "find_tools",
        description = "Search Google Ad Manager MCP tools by keyword, group, and read-only status for OpenAI tool_search or deferred-loading clients."
    )]
    async fn find_tools(
        &self,
        Parameters(args): Parameters<FindToolsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let limit = args.limit.unwrap_or(20).clamp(1, 100);
        let filter = ToolSearchFilter {
            query: args.query.clone(),
            group: args.group.clone(),
            read_only: args.read_only,
            limit: Some(limit),
        };
        let results = self.inventory().search(
            &filter,
            ToolOperation::List,
            &mcp_toolkit_core::tool_inventory::ToolInventoryPolicy::strict(),
        );
        let schemas = if args.include_schema {
            let mut schema_map = serde_json::Map::new();
            for tool in self.tool_schema_snapshot() {
                if results
                    .iter()
                    .any(|result| result.name == tool.name.as_ref())
                {
                    schema_map.insert(tool.name.to_string(), json!(tool));
                }
            }
            Some(Value::Object(schema_map))
        } else {
            None
        };
        let response =
            ToolSearchResponse::find_tools(args.query, args.group, args.read_only, results)
                .with_schemas(schemas)
                .with_metadata_label("gpt-5.5-compatible tool_search metadata contract");
        Ok(contract::success(response.to_value(), started))
    }

    #[tool(
        name = "gam_get_started",
        description = "Explain the recommended first-run flow, credential options, and starter tools for Google Ad Manager."
    )]
    async fn gam_get_started(
        &self,
        _args: Parameters<GetStartedArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let scope = self.client().scope().to_string();
        Ok(contract::success(
            json!({
                "server": "google-ad-manager-mcp",
                "goal": "Inspect Google Ad Manager networks, inventory, delivery catalog data, and saved report results through a read-only MCP surface.",
                "recommended_steps": [
                    format!("Run gcloud auth application-default login --scopes={scope}"),
                    "If you used user credentials, run gcloud auth application-default set-quota-project <PROJECT_ID> for the Google Cloud project where the Ad Manager API is enabled.",
                    "Restart any stdio MCP client that keeps a long-lived child process.",
                    "Call gam_auth_status with verify_access=true.",
                    "Call gam_networks_list to discover the exact network code.",
                    "Call gam_network_catalog_list for ad_units, orders, line_items, or reports.",
                    "Call gam_report_run for saved reports and gam_report_result_rows for large paginated results."
                ],
                "supported_credential_sources": [
                    {
                        "name": "Application Default Credentials",
                        "env": ["GOOGLE_APPLICATION_CREDENTIALS"],
                        "notes": "Recommended for local use through gcloud auth application-default login."
                    },
                    {
                        "name": "Server-specific service account JSON path",
                        "env": ["GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH"],
                        "notes": "Recommended for sealed unattended deployments."
                    },
                    {
                        "name": "Server-specific raw service account JSON",
                        "env": ["GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON"],
                        "notes": "Use only when your platform cannot mount a secret file."
                    }
                ],
                "starter_tools": [
                    "gam_auth_status",
                    "gam_networks_list",
                    "gam_network_catalog_list"
                ],
                "notes": [
                    "The server is read-only in the initial public release.",
                    "The official Google Ad Manager Beta REST API is the primary upstream surface.",
                    "Saved report execution remains asynchronous; gam_report_run can wait for completion and optionally fetch the first page."
                ]
            }),
            started,
        ))
    }

    #[tool(
        name = "gam_auth_status",
        description = "Inspect configured Google Ad Manager credential inputs and optionally verify live upstream access without returning secrets."
    )]
    async fn gam_auth_status(
        &self,
        Parameters(args): Parameters<AuthStatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let verify_access = args.verify_access;
        let gcloud = gcloud_version().await;
        let verification = if verify_access {
            match self.client().list_networks(Some(1), None).await {
                Ok(payload) => json!({
                    "checked": true,
                    "ok": true,
                    "sample_network_count": payload.get("networks").and_then(Value::as_array).map(|rows| rows.len()).unwrap_or(0),
                }),
                Err(err) => json!({
                    "checked": true,
                    "ok": false,
                    "error": contract::redact_secret_text(&err.to_string()),
                    "hint": err.hint(),
                }),
            }
        } else {
            json!({
                "checked": false,
                "ok": null
            })
        };

        Ok(contract::success(
            json!({
                "requested_scope": self.client().scope(),
                "auth_source_candidate": self.client().auth_source().as_str(),
                "quota_project_configured": self.client().quota_project_configured(),
                "credential_material_detected": credential_material_detected(self.settings()),
                "detected": {
                    "gcloud_available": gcloud.is_some(),
                    "gcloud_version": gcloud,
                    "env": {
                        "GOOGLE_APPLICATION_CREDENTIALS": std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").is_some(),
                        "GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH": self.settings().service_account_json_path.is_some(),
                        "GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON": self.settings().service_account_json.is_some(),
                    }
                },
                "verification": verification,
                "next_steps": auth_next_steps(self.client().scope(), verify_access),
            }),
            started,
        ))
    }

    #[tool(
        name = "gam_auth_login_command",
        description = "Build a copyable gcloud Application Default Credentials login command for Google Ad Manager without running it."
    )]
    async fn gam_auth_login_command(
        &self,
        Parameters(args): Parameters<AuthLoginCommandArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let no_launch_browser = args.no_launch_browser.unwrap_or(true);
        let command = gcloud_adc_login_command(
            self.client().scope(),
            args.client_id_file.as_deref(),
            no_launch_browser,
        );
        let quota_project = args
            .quota_project
            .clone()
            .or_else(|| self.settings().quota_project.clone());
        let follow_up_commands = quota_project
            .as_deref()
            .map(|project| {
                vec![format!(
                    "gcloud auth application-default set-quota-project {project}"
                )]
            })
            .unwrap_or_default();
        Ok(contract::success(
            json!({
                "command": command,
                "shell_command": shell_join(&command),
                "follow_up_commands": follow_up_commands,
                "scope": self.client().scope(),
                "notes": [
                    "This command writes Application Default Credentials on the machine where it is run.",
                    "No token or client secret is returned by this tool.",
                    format!("Use {} when you need write-capable Ad Manager credentials in a future operator slice.", MANAGE_SCOPE),
                ]
            }),
            started,
        ))
    }

    #[tool(
        name = "gam_networks_list",
        description = "List Google Ad Manager networks visible to the authenticated principal."
    )]
    async fn gam_networks_list(
        &self,
        Parameters(args): Parameters<NetworksListArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        match self
            .client()
            .list_networks(args.page_size, args.page_token)
            .await
        {
            Ok(payload) => Ok(contract::success(payload, started)),
            Err(err) => Ok(contract::error(err, started)),
        }
    }

    #[tool(
        name = "gam_network_catalog_list",
        description = "List a curated Google Ad Manager network collection such as ad units, orders, line items, or reports."
    )]
    async fn gam_network_catalog_list(
        &self,
        Parameters(args): Parameters<NetworkCatalogListArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        match self
            .client()
            .list_network_catalog(
                &args.network_code,
                args.collection,
                args.page_size,
                args.page_token,
                args.filter,
                args.order_by,
            )
            .await
        {
            Ok(payload) => Ok(contract::success_with_meta(
                payload,
                json!({
                    "collection": args.collection.as_str(),
                    "network_code": args.network_code,
                }),
                started,
            )),
            Err(err) => Ok(contract::error(err, started)),
        }
    }

    #[tool(
        name = "gam_report_run",
        description = "Run a saved Google Ad Manager report, optionally wait for completion, and optionally fetch the first page of result rows."
    )]
    async fn gam_report_run(
        &self,
        Parameters(args): Parameters<ReportRunArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let wait_for_completion = args.wait_for_completion.unwrap_or(true);
        let fetch_first_page = args.fetch_first_page.unwrap_or(true);

        let operation = match self
            .client()
            .run_report(&args.network_code, &args.report_id)
            .await
        {
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };

        let operation_name = operation
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string);

        if !wait_for_completion {
            return Ok(contract::success(
                json!({
                    "operation": operation,
                    "waited": false,
                    "next_steps": [
                        "Poll the operation name again through gam_report_run with wait_for_completion=true if you want a completed report result name.",
                        "Once the operation is complete, fetch rows with gam_report_result_rows."
                    ]
                }),
                started,
            ));
        }

        let Some(operation_name) = operation_name else {
            return Ok(contract::error(
                AdManagerError::invalid(
                    "operation.name",
                    "run response did not contain an operation name",
                ),
                started,
            ));
        };

        let timeout = args
            .poll_timeout_ms
            .map(std::time::Duration::from_millis)
            .unwrap_or(self.settings().report_poll_timeout);
        let initial_interval = args
            .initial_poll_interval_ms
            .map(std::time::Duration::from_millis)
            .unwrap_or(self.settings().report_poll_initial_interval);

        let completed = match self
            .client()
            .wait_for_report_result(&operation_name, timeout, initial_interval)
            .await
        {
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };

        let first_page = if fetch_first_page {
            match self
                .client()
                .get_report_result_rows(&completed.report_result, args.result_page_size, None)
                .await
            {
                Ok(value) => Some(value),
                Err(err) => return Ok(contract::error(err, started)),
            }
        } else {
            None
        };

        Ok(contract::success(
            json!({
                "waited": true,
                "operation": completed.operation,
                "report_result": completed.report_result,
                "first_page": first_page,
                "next_steps": [
                    "Use gam_report_result_rows with the returned result_name and nextPageToken when the first page was truncated.",
                    "If the report returns no rows, inspect the saved report filters, date range, and sharing in the Ad Manager UI."
                ]
            }),
            started,
        ))
    }

    #[tool(
        name = "gam_report_result_rows",
        description = "Fetch rows from a completed Google Ad Manager report result resource."
    )]
    async fn gam_report_result_rows(
        &self,
        Parameters(args): Parameters<ReportResultRowsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        match self
            .client()
            .get_report_result_rows(&args.result_name, args.page_size, args.page_token)
            .await
        {
            Ok(payload) => Ok(contract::success_with_meta(
                payload,
                json!({ "result_name": args.result_name }),
                started,
            )),
            Err(err) => Ok(contract::error(err, started)),
        }
    }
}

fn auth_next_steps(scope: &str, access_checked: bool) -> Vec<String> {
    let mut steps = vec![
        format!("Run gcloud auth application-default login --scopes={scope} if no credential source is configured."),
        "Restart stdio MCP clients that keep long-lived server child processes after changing credentials or environment.".to_string(),
    ];
    if !access_checked {
        steps.push("Call gam_auth_status with verify_access=true when you are ready to prove Ad Manager access.".to_string());
    }
    steps.push("Call gam_networks_list to discover the exact network code before using gam_network_catalog_list or gam_report_run.".to_string());
    steps
}

fn credential_material_detected(settings: &crate::Settings) -> bool {
    std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").is_some()
        || settings.service_account_json_path.is_some()
        || settings.service_account_json.is_some()
}

async fn gcloud_version() -> Option<String> {
    let output = Command::new("gcloud")
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|stdout| stdout.lines().next().map(str::trim).map(str::to_string))
}

fn gcloud_adc_login_command(
    scope: &str,
    client_id_file: Option<&str>,
    no_launch_browser: bool,
) -> Vec<String> {
    let mut command = vec![
        "gcloud".to_string(),
        "auth".to_string(),
        "application-default".to_string(),
        "login".to_string(),
        format!("--scopes={scope}"),
    ];
    if no_launch_browser {
        command.push("--no-launch-browser".to_string());
    }
    if let Some(path) = client_id_file
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.push(format!("--client-id-file={path}"));
    }
    command
}

fn shell_join(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| {
            if part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_=:/.,+".contains(ch))
            {
                part.clone()
            } else {
                format!("'{}'", part.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
fn client_id_file_exists(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}

#[cfg(test)]
mod tests {
    use super::{client_id_file_exists, gcloud_adc_login_command, shell_join};

    #[test]
    fn login_command_includes_scope_and_headless_flag() {
        let command = gcloud_adc_login_command(
            "https://www.googleapis.com/auth/admanager.readonly",
            None,
            true,
        );
        let rendered = shell_join(&command);
        assert!(rendered.contains("application-default login"));
        assert!(rendered.contains("--no-launch-browser"));
        assert!(rendered.contains("admanager.readonly"));
    }

    #[test]
    fn missing_client_id_path_is_false() {
        assert!(!client_id_file_exists("/definitely/not/here/client.json"));
    }
}
