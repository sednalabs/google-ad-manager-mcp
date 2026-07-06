use std::time::Instant;

use mcp_toolkit::rmcp::handler::server::wrapper::Parameters;
use mcp_toolkit::rmcp::model::CallToolResult;
use mcp_toolkit::rmcp::{self, tool, tool_router};
use mcp_toolkit_auth::provider_auth::{GoogleProviderAuthConfig, google_adc_quota_project_command};
use mcp_toolkit_scratchpad::{
    ScratchpadIngestColumn, ScratchpadIngestMode, ScratchpadQueryProjection, ScratchpadSessionInfo,
    ScratchpadSessionSnapshot, ScratchpadTableInfo, run_scratchpad_blocking,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::process::Command;

use crate::auth_ux::{gcloud_adc_login_command, shell_join};
use crate::client::{
    CatalogCollection, DEFAULT_SOAP_API_VERSION, RestWriteOperation, RestWritePlan,
    RestWriteResource, SoapTraffickingOperation, SoapTraffickingPlan,
};
use crate::config::GCLOUD_ADC_REQUIRED_SCOPE;
use crate::contract;
use crate::{AdManagerError, AdManagerServer, MANAGE_SCOPE, McpError};
use mcp_toolkit_core::guarded_action::{
    GuardedActionApply, GuardedActionError, GuardedActionNoMutationProof, GuardedActionPlanSeed,
    GuardedActionPosture, GuardedActionPreview, GuardedActionRuntimeMode,
};
use mcp_toolkit_core::tool_inventory::{ToolOperation, ToolSearchFilter, ToolSearchResponse};

const AD_MANAGER_PROVIDER_API_NAME: &str = "Google Ad Manager API";
const AD_MANAGER_PROVIDER_API_SERVICE: &str = "admanager.googleapis.com";

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
    /// Request the write-capable Ad Manager manage scope instead of the current server scope.
    #[serde(default)]
    pub manage_scope: Option<bool>,
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

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RestWriteRequestArgs {
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// Allowlisted REST beta resource to mutate.
    pub resource: RestWriteResource,
    /// Allowlisted REST beta write operation.
    pub operation: RestWriteOperation,
    /// Full resource name for patch operations, for example networks/123/adUnits/456.
    pub resource_name: Option<String>,
    /// Optional field mask for patch operations, for example displayName,status.
    pub update_mask: Option<String>,
    /// Official Ad Manager REST request JSON body.
    pub body: Value,
    /// Human-readable reason for the proposed change.
    pub reason: String,
    /// Expected advertiser, campaign, delivery, or operational impact.
    pub expected_impact: Option<String>,
    /// Rollback or reversal note. Required by policy for live apply review.
    pub rollback_note: Option<String>,
    /// Optional caller-supplied idempotency or ticket reference.
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestWritePlanArgs {
    pub request: RestWriteRequestArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestWriteApplyArgs {
    pub request: RestWriteRequestArgs,
    /// Confirmation token returned by gam_rest_write_plan for this exact request.
    pub confirmation_token: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SoapTraffickingRequestArgs {
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// SOAP API version, for example v202605. Defaults to the current server default.
    pub api_version: Option<String>,
    /// Allowlisted Google Ad Manager SOAP trafficking or forecast operation.
    pub operation: SoapTraffickingOperation,
    /// Inner XML payload for the selected operation, excluding SOAP Envelope, Header, and operation wrapper.
    pub payload_xml: String,
    /// Human-readable reason for the proposed live SOAP call.
    pub reason: String,
    /// Expected advertiser, campaign, delivery, or operational impact. Required for mutating apply.
    pub expected_impact: Option<String>,
    /// Rollback or reversal note. Required by policy for mutating apply review.
    pub rollback_note: Option<String>,
    /// Optional caller-supplied idempotency or ticket reference.
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SoapTraffickingPlanArgs {
    pub request: SoapTraffickingRequestArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SoapTraffickingApplyArgs {
    pub request: SoapTraffickingRequestArgs,
    /// Confirmation token returned by gam_soap_trafficking_plan for this exact request.
    pub confirmation_token: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct TraffickingToolMatrixArgs {
    /// Include remaining high-level builder, response-modeling, and readback gaps. Defaults to true.
    pub include_gaps: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScratchpadSessionArgs {
    /// Scratchpad session identifier. Use stable names such as gam_delivery_2026_07.
    pub session_id: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ScratchpadInventoryArgs {
    /// Maximum sessions to return.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScratchpadTableInventoryArgs {
    /// Scratchpad session identifier.
    pub session_id: String,
    /// Maximum tables to return.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScratchpadDropTableArgs {
    /// Scratchpad session identifier.
    pub session_id: String,
    /// Scratchpad table name to drop.
    pub table_name: String,
    /// Treat a missing table as a success. Defaults to true.
    #[serde(default = "default_true")]
    pub if_exists: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScratchpadQueryArgs {
    /// Scratchpad session identifier.
    pub session_id: String,
    /// Read-only SQL query. Only SELECT/WITH/EXPLAIN/DESCRIBE/SUMMARIZE style queries are allowed.
    pub sql: String,
    /// Zero-based result offset.
    pub offset: Option<u64>,
    /// Page size for returned rows.
    pub page_size: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScratchpadIngestNetworkCatalogArgs {
    /// Scratchpad session identifier.
    pub session_id: String,
    /// Scratchpad table name to create or append to.
    pub table_name: String,
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// Which curated network collection to fetch and ingest.
    pub collection: CatalogCollection,
    pub page_size: Option<u32>,
    pub page_token: Option<String>,
    /// Optional Ad Manager Beta filter expression.
    pub filter: Option<String>,
    /// Optional Ad Manager Beta orderBy expression.
    pub order_by: Option<String>,
    /// Append rows to an existing scratchpad table instead of replacing it.
    #[serde(default)]
    pub append: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScratchpadIngestReportRowsArgs {
    /// Scratchpad session identifier.
    pub session_id: String,
    /// Scratchpad table name to create or append to.
    pub table_name: String,
    /// Result resource name, for example networks/123/reports/456/results/789.
    pub result_name: String,
    pub page_size: Option<u32>,
    pub page_token: Option<String>,
    /// Append rows to an existing scratchpad table instead of replacing it.
    #[serde(default)]
    pub append: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScratchpadEvidenceBundleArgs {
    /// Scratchpad session identifier.
    pub session_id: String,
    /// Optional table allowlist. Defaults to every table in the session.
    pub tables: Option<Vec<String>>,
    /// Rows sampled from each table in the markdown bundle.
    pub sample_rows_per_table: Option<u64>,
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
                "goal": "Inspect Google Ad Manager networks, inventory, delivery catalog data, saved report results, guarded REST write plans, and guarded SOAP trafficking through an MCP surface.",
                "recommended_steps": [
                    "Run google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> for the easiest local browser login.",
                    format!("The login helper requests both {GCLOUD_ADC_REQUIRED_SCOPE} and {scope}, matching gcloud ADC requirements."),
                    "Restart any stdio MCP client that keeps a long-lived child process.",
                    "Call gam_auth_status with verify_access=true.",
                    "Call gam_networks_list to discover the exact network code.",
                    "Call gam_network_catalog_list for ad_units, orders, line_items, or reports.",
                    "Call gam_report_run for saved reports and gam_report_result_rows for large paginated results.",
                    "Call gam_trafficking_tool_matrix before planning writes so the REST and SOAP trafficking surfaces are explicit.",
                    "Use gam_rest_write_plan for dry-run write plans; gam_rest_write_apply only works when the server is explicitly started with GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled and the manage scope.",
                    "Use gam_soap_trafficking_plan and gam_soap_trafficking_apply for order, line-item, creative, LICA, and forecast SOAP workflows.",
                    "For local operator apply testing, rerun auth with google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> --manage-scope.",
                    "Use gam_scratchpad_open_session plus the gam_scratchpad_ingest_* tools when you need local joins, filtering, evidence bundles, or larger result review."
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
                    "gam_network_catalog_list",
                    "gam_trafficking_tool_matrix",
                    "gam_rest_write_plan",
                    "gam_soap_trafficking_plan",
                    "gam_scratchpad_open_session"
                ],
                "notes": [
                    format!("Current write mode is {}. Live apply is disabled unless this is enabled.", self.settings().write_mode.as_str()),
                    "The official Google Ad Manager Beta REST API is the primary upstream surface.",
                    "The guarded SOAP layer covers production trafficking operations that are not yet available through the REST beta surface.",
                    "SOAP live calls require the full Ad Manager manage scope, including non-mutating forecast/read calls, because the legacy SOAP API does not accept the newer read-only scope.",
                    "Saved report execution remains asynchronous; gam_report_run can wait for completion and optionally fetch the first page.",
                    "Scratchpad data stays local to the MCP server process and is bounded by session, table, row, memory, SQL-size, and query-time limits."
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
        let requested_scope = if args.manage_scope.unwrap_or(false) {
            MANAGE_SCOPE
        } else {
            self.client().scope()
        };
        let setup_plan = ad_manager_provider_auth_config(requested_scope).adc_setup_plan();
        let command = gcloud_adc_login_command(
            requested_scope,
            args.client_id_file.as_deref().map(std::path::Path::new),
            no_launch_browser,
        );
        let quota_project = args
            .quota_project
            .clone()
            .or_else(|| self.settings().quota_project.clone());
        let follow_up_commands = quota_project
            .as_deref()
            .map(|project| vec![shell_join(&google_adc_quota_project_command(project))])
            .unwrap_or_default();
        Ok(contract::success(
            json!({
                "command": command,
                "shell_command": shell_join(&command),
                "headless_command": setup_plan.headless_login.argv,
                "headless_shell_command": setup_plan.headless_login.shell.clone(),
                "client_id_file_command": setup_plan.login_with_client_id_file.argv,
                "client_id_file_shell_command": setup_plan.login_with_client_id_file.shell.clone(),
                "client_id_file_headless_command": setup_plan.headless_login_with_client_id_file.argv,
                "client_id_file_headless_shell_command": setup_plan.headless_login_with_client_id_file.shell.clone(),
                "quota_project_command": setup_plan.quota_project.shell.clone(),
                "api_enable_command": setup_plan.api_enable.as_ref().map(|command| command.shell.as_str()),
                "adc_scopes": setup_plan.scopes.clone(),
                "follow_up_commands": follow_up_commands,
                "setup_next_steps": setup_plan.next_steps.clone(),
                "scope": requested_scope,
                "manage_scope": requested_scope == MANAGE_SCOPE,
                "next_steps": setup_plan.next_steps.clone(),
                "notes": [
                    "This command writes Application Default Credentials on the machine where it is run.",
                    "No token or client secret is returned by this tool.",
                    format!("Use manage_scope=true or --manage-scope when you need write-capable Ad Manager credentials for operator-approved apply."),
                    "Use the client-id-file command if Google rejects the Ad Manager scope during ADC login.",
                    "For unattended deployments, prefer service-account or workload identity credentials over local user ADC.",
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

    #[tool(
        name = "gam_rest_write_plan",
        description = "Create a dry-run plan and confirmation token for an allowlisted Google Ad Manager REST write operation without mutating upstream state."
    )]
    async fn gam_rest_write_plan(
        &self,
        Parameters(args): Parameters<RestWritePlanArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        if let Err(err) = self
            .settings()
            .write_mode
            .assert_allowed("gam_rest_write_plan", GuardedActionPosture::preview())
        {
            return Ok(contract::error(write_action_disabled(err), started));
        }

        let plan = match build_write_plan(self, &args.request) {
            Ok(plan) => plan,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let (plan_id, confirmation_token, fingerprint) =
            match guarded_write_identifiers(&args.request, &plan) {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
        let warnings = write_plan_warnings(
            self.settings().write_mode,
            self.client().scope(),
            &args.request,
            &plan,
        );
        let preview = GuardedActionPreview::new(
            plan_id,
            self.settings().write_mode,
            GuardedActionPosture::preview(),
            json!({
                "request": args.request,
                "rest_request": rest_write_plan_to_json(&plan),
                "confirmation_token": confirmation_token,
                "fingerprint": fingerprint,
                "warnings": warnings,
                "next_step": "Review the plan. To apply, restart/configure the server with GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled, use the manage scope, and call gam_rest_write_apply with this exact request and confirmation_token.",
            }),
            json!({
                "mutation_performed": false,
                "upstream_called": false,
                "write_mode": self.settings().write_mode.as_str(),
                "required_apply_scope": MANAGE_SCOPE,
                "policy": provider_safety_contract_json(),
            }),
        );
        Ok(contract::success(json!(preview), started))
    }

    #[tool(
        name = "gam_rest_write_apply",
        description = "Apply an allowlisted Google Ad Manager REST write plan after explicit runtime, scope, and confirmation-token gates."
    )]
    async fn gam_rest_write_apply(
        &self,
        Parameters(args): Parameters<RestWriteApplyArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let plan = match build_write_plan(self, &args.request) {
            Ok(plan) => plan,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let apply_posture = apply_posture_for_plan(&plan);
        if let Err(err) = self
            .settings()
            .write_mode
            .assert_allowed("gam_rest_write_apply", apply_posture)
        {
            return Ok(contract::error(write_action_disabled(err), started));
        }
        if !scope_allows_write(self.client().scope()) {
            return Ok(contract::error(
                AdManagerError::WriteScopeRequired {
                    scope: self.client().scope().to_string(),
                },
                started,
            ));
        }
        if let Err(err) = validate_apply_context(&args.request) {
            return Ok(contract::error(err, started));
        }

        let (plan_id, expected_token, fingerprint) =
            match guarded_write_identifiers(&args.request, &plan) {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
        if args.confirmation_token.trim() != expected_token {
            return Ok(contract::error(
                AdManagerError::ConfirmationTokenMismatch,
                started,
            ));
        }

        let applied = match self.client().execute_rest_write_plan(&plan).await {
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let apply = GuardedActionApply::new(
            plan_id,
            self.settings().write_mode,
            apply_posture,
            json!({
                "rest_request": rest_write_plan_to_json(&plan),
                "upstream_response": applied.upstream_response,
                "post_apply_readback": {
                    "attempted": plan.readback_path.is_some(),
                    "result": applied.readback,
                    "error": applied.readback_error,
                },
            }),
            json!({
                "mutation_performed": true,
                "fingerprint": fingerprint,
                "required_apply_scope": MANAGE_SCOPE,
                "operator_context": {
                    "reason": args.request.reason,
                    "expected_impact": args.request.expected_impact,
                    "rollback_note": args.request.rollback_note,
                    "idempotency_key": args.request.idempotency_key,
                },
                "policy": provider_safety_contract_json(),
            }),
        );
        Ok(contract::success(json!(apply), started))
    }

    #[tool(
        name = "gam_soap_trafficking_plan",
        description = "Create a dry-run plan and confirmation token for an allowlisted Google Ad Manager SOAP trafficking or forecast operation without calling upstream."
    )]
    async fn gam_soap_trafficking_plan(
        &self,
        Parameters(args): Parameters<SoapTraffickingPlanArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        if let Err(err) = self
            .settings()
            .write_mode
            .assert_allowed("gam_soap_trafficking_plan", GuardedActionPosture::preview())
        {
            return Ok(contract::error(write_action_disabled(err), started));
        }

        let plan = match build_soap_trafficking_plan(self, &args.request) {
            Ok(plan) => plan,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let (plan_id, confirmation_token, fingerprint) =
            match guarded_soap_identifiers(&args.request, &plan) {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
        let warnings = soap_trafficking_plan_warnings(
            self.settings().write_mode,
            self.client().scope(),
            &args.request,
            &plan,
        );
        let preview = GuardedActionPreview::new(
            plan_id,
            self.settings().write_mode,
            GuardedActionPosture::preview(),
            json!({
                "request": args.request,
                "soap_request": soap_trafficking_plan_to_json(&plan),
                "confirmation_token": confirmation_token,
                "fingerprint": fingerprint,
                "warnings": warnings,
                "next_step": "Review the SOAP envelope and operation impact. To run it, use credentials with https://www.googleapis.com/auth/admanager and call gam_soap_trafficking_apply with this exact request and confirmation_token. Mutating operations also require GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled.",
            }),
            json!({
                "mutation_performed": false,
                "upstream_called": false,
                "soap_default_api_version": DEFAULT_SOAP_API_VERSION,
                "required_soap_scope": MANAGE_SCOPE,
                "policy": provider_safety_contract_json(),
            }),
        );
        Ok(contract::success(json!(preview), started))
    }

    #[tool(
        name = "gam_soap_trafficking_apply",
        description = "Run an allowlisted Google Ad Manager SOAP trafficking or forecast operation after scope, runtime, and confirmation-token gates."
    )]
    async fn gam_soap_trafficking_apply(
        &self,
        Parameters(args): Parameters<SoapTraffickingApplyArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let plan = match build_soap_trafficking_plan(self, &args.request) {
            Ok(plan) => plan,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let apply_posture = soap_posture_for_plan(&plan);
        if let Err(err) = self
            .settings()
            .write_mode
            .assert_allowed("gam_soap_trafficking_apply", apply_posture)
        {
            return Ok(contract::error(write_action_disabled(err), started));
        }
        if !scope_allows_write(self.client().scope()) {
            return Ok(contract::error(
                AdManagerError::WriteScopeRequired {
                    scope: self.client().scope().to_string(),
                },
                started,
            ));
        }
        if plan.mutating {
            if let Err(err) = validate_soap_apply_context(&args.request) {
                return Ok(contract::error(err, started));
            }
        }

        let (plan_id, expected_token, fingerprint) =
            match guarded_soap_identifiers(&args.request, &plan) {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
        if args.confirmation_token.trim() != expected_token {
            return Ok(contract::error(
                AdManagerError::ConfirmationTokenMismatch,
                started,
            ));
        }

        let applied = match self.client().execute_soap_trafficking_plan(&plan).await {
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };
        if applied.upstream_status >= 400 {
            return Ok(contract::error_with_detail(
                AdManagerError::UpstreamApi {
                    status: applied.upstream_status,
                    message: crate::client::soap_error_message(&applied),
                },
                json!({
                    "soap_request": soap_trafficking_plan_to_json(&plan),
                    "upstream_status": applied.upstream_status,
                    "upstream_response_xml": applied.upstream_response_xml,
                    "response_truncated": applied.response_truncated,
                    "request_id": applied.request_id,
                    "response_time": applied.response_time,
                    "soap_fault": applied.soap_fault,
                }),
                started,
            ));
        }
        let apply = GuardedActionApply::new(
            plan_id,
            self.settings().write_mode,
            apply_posture,
            json!({
                "soap_request": soap_trafficking_plan_to_json(&plan),
                "upstream_status": applied.upstream_status,
                "upstream_response_xml": applied.upstream_response_xml,
                "response_truncated": applied.response_truncated,
                "request_id": applied.request_id,
                "response_time": applied.response_time,
                "soap_fault": applied.soap_fault,
            }),
            json!({
                "mutation_performed": plan.mutating,
                "upstream_called": true,
                "fingerprint": fingerprint,
                "required_soap_scope": MANAGE_SCOPE,
                "operator_context": {
                    "reason": args.request.reason,
                    "expected_impact": args.request.expected_impact,
                    "rollback_note": args.request.rollback_note,
                    "idempotency_key": args.request.idempotency_key,
                },
                "policy": provider_safety_contract_json(),
            }),
        );
        Ok(contract::success(json!(apply), started))
    }

    #[tool(
        name = "gam_trafficking_tool_matrix",
        description = "Describe the current Google Ad Manager write and trafficking surface, including REST-supported writes, SOAP trafficking operations, and remaining ergonomics gaps."
    )]
    async fn gam_trafficking_tool_matrix(
        &self,
        Parameters(args): Parameters<TraffickingToolMatrixArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let include_gaps = args.include_gaps.unwrap_or(true);
        let proof = GuardedActionNoMutationProof::new(
            self.settings().write_mode,
            json!({
                "write_mode": self.settings().write_mode.as_str(),
                "apply_enabled": self.settings().write_mode == GuardedActionRuntimeMode::Enabled,
                "current_scope": self.client().scope(),
                "required_apply_scope": MANAGE_SCOPE,
                "rest_beta_write_tools": [
                    "gam_rest_write_plan",
                    "gam_rest_write_apply"
                ],
                "soap_trafficking_tools": [
                    "gam_soap_trafficking_plan",
                    "gam_soap_trafficking_apply"
                ],
                "rest_beta_supported_resources": rest_supported_resource_matrix(),
                "soap_trafficking_supported_operations": soap_trafficking_supported_operation_matrix(),
                "trafficking_gaps": if include_gaps { trafficking_gap_matrix() } else { Value::Null },
                "remaining_gaps": if include_gaps { trafficking_gap_matrix() } else { Value::Null },
            }),
            json!({
                "mutation_performed": false,
                "upstream_called": false,
                "policy": provider_safety_contract_json(),
                "notes": [
                    "Google Ad Manager REST beta exposes write methods for inventory/supporting resources and saved reports.",
                    "The guarded SOAP tools cover order, line item, creative, line-item creative association, preview URL, and forecast operations.",
                    "Live SOAP calls require the Ad Manager manage OAuth scope; mutating SOAP calls also require write mode enabled."
                ],
            }),
        );
        Ok(contract::success(json!(proof), started))
    }

    #[tool(
        name = "gam_scratchpad_open_session",
        description = "Open or refresh a bounded local DuckDB scratchpad session for Google Ad Manager evidence work."
    )]
    async fn gam_scratchpad_open_session(
        &self,
        Parameters(args): Parameters<ScratchpadSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let sessions = self.scratchpad_sessions().clone();
        let session_id = args.session_id.clone();
        match run_scratchpad_blocking(move || sessions.open_session(&session_id)).await {
            Ok(info) => Ok(contract::success(
                scratchpad_session_info_to_json(info),
                started,
            )),
            Err(err) => Ok(contract::scratchpad_error(err, started)),
        }
    }

    #[tool(
        name = "gam_scratchpad_close_session",
        description = "Close a Google Ad Manager scratchpad session and remove its local DuckDB database."
    )]
    async fn gam_scratchpad_close_session(
        &self,
        Parameters(args): Parameters<ScratchpadSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let sessions = self.scratchpad_sessions().clone();
        let session_id = args.session_id.clone();
        match run_scratchpad_blocking(move || sessions.release_session(&session_id)).await {
            Ok(released) => Ok(contract::success(
                json!({
                    "session_id": args.session_id,
                    "released": released,
                }),
                started,
            )),
            Err(err) => Ok(contract::scratchpad_error(err, started)),
        }
    }

    #[tool(
        name = "gam_scratchpad_list_sessions",
        description = "List active Google Ad Manager scratchpad sessions."
    )]
    async fn gam_scratchpad_list_sessions(
        &self,
        Parameters(args): Parameters<ScratchpadInventoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let limit = args.limit.unwrap_or(20).clamp(1, 100);
        let sessions = self.scratchpad_sessions().clone();
        match run_scratchpad_blocking(move || sessions.list_sessions(limit)).await {
            Ok(sessions) => Ok(contract::success(
                json!({
                    "sessions": sessions
                        .into_iter()
                        .map(scratchpad_session_info_to_json)
                        .collect::<Vec<_>>(),
                    "limit": limit,
                }),
                started,
            )),
            Err(err) => Ok(contract::scratchpad_error(err, started)),
        }
    }

    #[tool(
        name = "gam_scratchpad_list_tables",
        description = "List tables in a Google Ad Manager scratchpad session."
    )]
    async fn gam_scratchpad_list_tables(
        &self,
        Parameters(args): Parameters<ScratchpadTableInventoryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let limit = args.limit.unwrap_or(50).clamp(1, 200);
        let sessions = self.scratchpad_sessions().clone();
        let session_id = args.session_id.clone();
        match run_scratchpad_blocking(move || sessions.list_tables(&session_id, limit)).await {
            Ok(tables) => Ok(contract::success(
                json!({
                    "session_id": args.session_id,
                    "tables": tables
                        .into_iter()
                        .map(scratchpad_table_info_to_json)
                        .collect::<Vec<_>>(),
                    "limit": limit,
                }),
                started,
            )),
            Err(err) => Ok(contract::scratchpad_error(err, started)),
        }
    }

    #[tool(
        name = "gam_scratchpad_drop_table",
        description = "Drop one table from a Google Ad Manager scratchpad session."
    )]
    async fn gam_scratchpad_drop_table(
        &self,
        Parameters(args): Parameters<ScratchpadDropTableArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let sessions = self.scratchpad_sessions().clone();
        let session_id = args.session_id.clone();
        let table_name = args.table_name.clone();
        let if_exists = args.if_exists;
        match run_scratchpad_blocking(move || {
            sessions.drop_table(&session_id, &table_name, if_exists)
        })
        .await
        {
            Ok(stats) => Ok(contract::success(
                json!({
                    "session_id": args.session_id,
                    "table_name": args.table_name,
                    "dropped": stats.dropped,
                    "rows_removed": stats.rows_removed,
                    "session": scratchpad_snapshot_to_json(stats.session_snapshot),
                }),
                started,
            )),
            Err(err) => Ok(contract::scratchpad_error(err, started)),
        }
    }

    #[tool(
        name = "gam_scratchpad_query",
        description = "Run bounded read-only DuckDB SQL against a Google Ad Manager scratchpad session."
    )]
    async fn gam_scratchpad_query(
        &self,
        Parameters(args): Parameters<ScratchpadQueryArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let offset = args.offset.unwrap_or(0);
        let page_size = args.page_size.unwrap_or(100).clamp(1, 1_000);
        let sessions = self.scratchpad_sessions().clone();
        let session_id = args.session_id.clone();
        let sql = args.sql.clone();
        match run_scratchpad_blocking(move || {
            sessions.query_rows(&session_id, &sql, offset, page_size)
        })
        .await
        {
            Ok(projection) => Ok(contract::success(
                scratchpad_query_projection_to_json(projection, offset, page_size),
                started,
            )),
            Err(err) => Ok(contract::scratchpad_error(err, started)),
        }
    }

    #[tool(
        name = "gam_scratchpad_ingest_network_catalog",
        description = "Fetch one Google Ad Manager network catalog page and ingest it into a scratchpad table."
    )]
    async fn gam_scratchpad_ingest_network_catalog(
        &self,
        Parameters(args): Parameters<ScratchpadIngestNetworkCatalogArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let upstream = match self
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
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let columns = network_catalog_ingest_columns();
        let rows =
            network_catalog_rows_for_scratchpad(&upstream, args.collection, &args.network_code);
        let ingest_mode = if args.append {
            ScratchpadIngestMode::Append
        } else {
            ScratchpadIngestMode::Create
        };
        let columns_response = scratchpad_ingest_columns_to_json(columns.clone());
        let row_count = rows.len();
        let sessions = self.scratchpad_sessions().clone();
        let session_id = args.session_id.clone();
        let table_name = args.table_name.clone();
        match run_scratchpad_blocking(move || {
            sessions.ingest_rows_with_mode(&session_id, &table_name, &columns, &rows, ingest_mode)
        })
        .await
        {
            Ok(stats) => Ok(contract::success(
                json!({
                    "session_id": args.session_id,
                    "table_name": args.table_name,
                    "mode": ingest_mode_label(ingest_mode),
                    "rows_inserted": stats.rows_inserted,
                    "columns_inserted": stats.columns_inserted,
                    "columns": columns_response,
                    "session": scratchpad_snapshot_to_json(stats.session_snapshot),
                    "upstream_summary": {
                        "network_code": args.network_code,
                        "collection": args.collection.as_str(),
                        "response_field": args.collection.response_field(),
                        "row_count": row_count,
                        "next_page_token": upstream.get("nextPageToken").and_then(Value::as_str),
                    },
                }),
                started,
            )),
            Err(err) => Ok(contract::scratchpad_error(err, started)),
        }
    }

    #[tool(
        name = "gam_scratchpad_ingest_report_result_rows",
        description = "Fetch one completed Google Ad Manager report-result page and ingest it into a scratchpad table."
    )]
    async fn gam_scratchpad_ingest_report_result_rows(
        &self,
        Parameters(args): Parameters<ScratchpadIngestReportRowsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let upstream = match self
            .client()
            .get_report_result_rows(&args.result_name, args.page_size, args.page_token)
            .await
        {
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let columns = report_rows_ingest_columns();
        let rows = report_result_rows_for_scratchpad(&upstream, &args.result_name);
        let ingest_mode = if args.append {
            ScratchpadIngestMode::Append
        } else {
            ScratchpadIngestMode::Create
        };
        let columns_response = scratchpad_ingest_columns_to_json(columns.clone());
        let row_count = rows.len();
        let sessions = self.scratchpad_sessions().clone();
        let session_id = args.session_id.clone();
        let table_name = args.table_name.clone();
        match run_scratchpad_blocking(move || {
            sessions.ingest_rows_with_mode(&session_id, &table_name, &columns, &rows, ingest_mode)
        })
        .await
        {
            Ok(stats) => Ok(contract::success(
                json!({
                    "session_id": args.session_id,
                    "table_name": args.table_name,
                    "mode": ingest_mode_label(ingest_mode),
                    "rows_inserted": stats.rows_inserted,
                    "columns_inserted": stats.columns_inserted,
                    "columns": columns_response,
                    "session": scratchpad_snapshot_to_json(stats.session_snapshot),
                    "upstream_summary": {
                        "result_name": args.result_name,
                        "row_count": row_count,
                        "next_page_token": upstream.get("nextPageToken").and_then(Value::as_str),
                    },
                }),
                started,
            )),
            Err(err) => Ok(contract::scratchpad_error(err, started)),
        }
    }

    #[tool(
        name = "gam_scratchpad_export_evidence_bundle",
        description = "Export a bounded markdown evidence bundle from Google Ad Manager scratchpad tables."
    )]
    async fn gam_scratchpad_export_evidence_bundle(
        &self,
        Parameters(args): Parameters<ScratchpadEvidenceBundleArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let sample_rows = args.sample_rows_per_table.unwrap_or(10).clamp(1, 100);
        let sessions = self.scratchpad_sessions().clone();
        let session_id = args.session_id.clone();
        let requested_tables = args.tables.clone();
        let bundle_result = run_scratchpad_blocking(move || {
            let table_names = match requested_tables {
                Some(tables) => tables,
                None => sessions
                    .list_tables(&session_id, 100)?
                    .into_iter()
                    .map(|table| table.name)
                    .collect(),
            };

            let mut bundle = format!(
                "# Google Ad Manager Scratchpad Evidence Bundle\n\n- Session: `{}`\n- Tables: `{}`\n- Sample rows per table: `{}`\n\n",
                session_id,
                table_names.len(),
                sample_rows,
            );
            let mut summaries = Vec::new();
            for table_name in table_names {
                let quoted = quote_scratchpad_ident(&table_name);
                let count_sql = format!("SELECT COUNT(*) AS row_count FROM {quoted}");
                let count_projection = match sessions.query_rows(&session_id, &count_sql, 0, 1) {
                    Ok(projection) => projection,
                    Err(err) => {
                        append_evidence_table_error(&mut bundle, &table_name, &err);
                        summaries.push(json!({
                            "table_name": table_name,
                            "error": err.to_string(),
                        }));
                        continue;
                    }
                };
                let row_count = count_projection
                    .rows
                    .first()
                    .and_then(|row| row.get("row_count"))
                    .and_then(json_u64)
                    .unwrap_or(0);
                let sample_sql = format!("SELECT * FROM {quoted}");
                let sample_projection =
                    match sessions.query_rows(&session_id, &sample_sql, 0, sample_rows) {
                        Ok(projection) => projection,
                        Err(err) => {
                            append_evidence_table_error(&mut bundle, &table_name, &err);
                            summaries.push(json!({
                                "table_name": table_name,
                                "row_count": row_count,
                                "error": err.to_string(),
                            }));
                            continue;
                        }
                    };

                bundle.push_str(&format!("## `{table_name}`\n\n"));
                bundle.push_str(&format!("- Rows: `{row_count}`\n"));
                bundle.push_str(&format!(
                    "- Columns: `{}`\n\n",
                    sample_projection.columns.len()
                ));
                bundle.push_str(&markdown_table(&sample_projection));
                bundle.push('\n');
                summaries.push(json!({
                    "table_name": table_name,
                    "row_count": row_count,
                    "sample_rows": sample_projection.rows.len(),
                    "columns": sample_projection.columns
                        .into_iter()
                        .map(|column| json!({
                            "name": column.name,
                            "logical_type": column.logical_type,
                            "nullable": column.nullable,
                        }))
                        .collect::<Vec<_>>(),
                }));
            }
            Ok((bundle, summaries))
        })
        .await;

        let (bundle, summaries) = match bundle_result {
            Ok(bundle) => bundle,
            Err(err) => return Ok(contract::scratchpad_error(err, started)),
        };

        Ok(contract::success(
            json!({
                "session_id": args.session_id,
                "format": "markdown",
                "bundle": bundle,
                "tables": summaries,
            }),
            started,
        ))
    }
}

fn default_true() -> bool {
    true
}

fn build_write_plan(
    server: &AdManagerServer,
    request: &RestWriteRequestArgs,
) -> Result<RestWritePlan, AdManagerError> {
    validate_plan_context(request)?;
    server.client().build_rest_write_plan(
        &request.network_code,
        request.resource,
        request.operation,
        request.resource_name.as_deref(),
        request.update_mask.as_deref(),
        request.body.clone(),
    )
}

fn build_soap_trafficking_plan(
    server: &AdManagerServer,
    request: &SoapTraffickingRequestArgs,
) -> Result<SoapTraffickingPlan, AdManagerError> {
    validate_soap_plan_context(request)?;
    server.client().build_soap_trafficking_plan(
        &request.network_code,
        request.api_version.as_deref(),
        request.operation,
        &request.payload_xml,
    )
}

fn validate_plan_context(request: &RestWriteRequestArgs) -> Result<(), AdManagerError> {
    if request.reason.trim().is_empty() {
        return Err(AdManagerError::invalid(
            "reason",
            "must explain why the provider write is being planned",
        ));
    }
    Ok(())
}

fn validate_soap_plan_context(request: &SoapTraffickingRequestArgs) -> Result<(), AdManagerError> {
    if request.reason.trim().is_empty() {
        return Err(AdManagerError::invalid(
            "reason",
            "must explain why the SOAP operation is being planned",
        ));
    }
    Ok(())
}

fn validate_apply_context(request: &RestWriteRequestArgs) -> Result<(), AdManagerError> {
    if request
        .expected_impact
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        return Err(AdManagerError::invalid(
            "expected_impact",
            "is required before applying provider writes",
        ));
    }
    if request
        .rollback_note
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        return Err(AdManagerError::invalid(
            "rollback_note",
            "is required before applying provider writes",
        ));
    }
    Ok(())
}

fn validate_soap_apply_context(request: &SoapTraffickingRequestArgs) -> Result<(), AdManagerError> {
    if request
        .expected_impact
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        return Err(AdManagerError::invalid(
            "expected_impact",
            "is required before applying mutating SOAP trafficking operations",
        ));
    }
    if request
        .rollback_note
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        return Err(AdManagerError::invalid(
            "rollback_note",
            "is required before applying mutating SOAP trafficking operations",
        ));
    }
    Ok(())
}

fn write_action_disabled(err: GuardedActionError) -> AdManagerError {
    AdManagerError::WriteActionDisabled {
        message: err.to_string(),
    }
}

fn apply_posture_for_plan(plan: &RestWritePlan) -> GuardedActionPosture {
    if plan.destructive {
        GuardedActionPosture::destructive()
    } else if plan.send_adjacent {
        GuardedActionPosture::send_adjacent()
    } else {
        GuardedActionPosture::guarded_apply()
    }
}

fn soap_posture_for_plan(plan: &SoapTraffickingPlan) -> GuardedActionPosture {
    if !plan.mutating {
        GuardedActionPosture::no_mutation_proof()
    } else if plan.destructive {
        GuardedActionPosture::destructive()
    } else if plan.send_adjacent {
        GuardedActionPosture::send_adjacent()
    } else {
        GuardedActionPosture::guarded_apply()
    }
}

fn scope_allows_write(scope: &str) -> bool {
    scope
        .split([',', ' ', '\t', '\n'])
        .any(|part| part.trim() == MANAGE_SCOPE)
}

fn guarded_write_identifiers(
    request: &RestWriteRequestArgs,
    plan: &RestWritePlan,
) -> Result<(String, String, String), AdManagerError> {
    let target = format!(
        "{}:{}:{}",
        request.resource.as_str(),
        request.operation.as_str(),
        plan.target
    );
    let seed = GuardedActionPlanSeed::new("gam_rest_write", &request.network_code, &target)
        .map_err(|err| AdManagerError::invalid("plan_seed", err.to_string()))?;
    let fingerprint_input = json!({
        "request": request,
        "method": plan.method,
        "path": plan.path,
        "query": plan.query,
        "target": plan.target,
    });
    let fingerprint = stable_fingerprint(&fingerprint_input.to_string());
    let plan_id = format!("{}.{}", seed.stable_plan_id(), fingerprint);
    let confirmation_token = format!("confirm-gam-{fingerprint}");
    Ok((plan_id, confirmation_token, fingerprint))
}

fn guarded_soap_identifiers(
    request: &SoapTraffickingRequestArgs,
    plan: &SoapTraffickingPlan,
) -> Result<(String, String, String), AdManagerError> {
    let target = format!(
        "{}:{}:{}",
        request.operation.as_str(),
        plan.api_version,
        plan.target
    );
    let seed = GuardedActionPlanSeed::new("gam_soap_trafficking", &request.network_code, &target)
        .map_err(|err| AdManagerError::invalid("plan_seed", err.to_string()))?;
    let fingerprint_input = json!({
        "request": request,
        "endpoint": plan.endpoint,
        "namespace": plan.namespace,
        "service": plan.service,
        "method": plan.method,
        "target": plan.target,
    });
    let fingerprint = stable_fingerprint(&fingerprint_input.to_string());
    let plan_id = format!("{}.{}", seed.stable_plan_id(), fingerprint);
    let confirmation_token = format!("confirm-gam-soap-{fingerprint}");
    Ok((plan_id, confirmation_token, fingerprint))
}

fn stable_fingerprint(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn rest_write_plan_to_json(plan: &RestWritePlan) -> Value {
    json!({
        "resource": plan.resource.as_str(),
        "operation": plan.operation.as_str(),
        "network_code": plan.network_code,
        "method": plan.method,
        "path": plan.path,
        "query": plan.query,
        "body": plan.body,
        "target": plan.target,
        "readback_path": plan.readback_path,
        "destructive": plan.destructive,
        "send_adjacent": plan.send_adjacent,
        "request_hint": plan.operation.request_hint(),
    })
}

fn soap_trafficking_plan_to_json(plan: &SoapTraffickingPlan) -> Value {
    json!({
        "operation": plan.operation.as_str(),
        "network_code": plan.network_code,
        "api_version": plan.api_version,
        "service": plan.service,
        "method": plan.method,
        "endpoint": plan.endpoint,
        "namespace": plan.namespace,
        "payload_xml": plan.payload_xml,
        "envelope_xml": plan.envelope_xml,
        "target": plan.target,
        "mutating": plan.mutating,
        "destructive": plan.destructive,
        "send_adjacent": plan.send_adjacent,
        "request_hint": plan.request_hint,
    })
}

fn write_plan_warnings(
    write_mode: GuardedActionRuntimeMode,
    scope: &str,
    request: &RestWriteRequestArgs,
    plan: &RestWritePlan,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if write_mode != GuardedActionRuntimeMode::Enabled {
        warnings.push(format!(
            "Apply is disabled while GOOGLE_AD_MANAGER_MCP_WRITE_MODE is {}.",
            write_mode.as_str()
        ));
    }
    if !scope_allows_write(scope) {
        warnings.push(format!(
            "Apply requires the Google Ad Manager manage scope: {MANAGE_SCOPE}."
        ));
    }
    if request
        .expected_impact
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        warnings.push("Apply requires expected_impact to be set.".to_string());
    }
    if request
        .rollback_note
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        warnings.push("Apply requires rollback_note to be set.".to_string());
    }
    if plan.readback_path.is_none()
        && !matches!(
            plan.operation,
            RestWriteOperation::Create | RestWriteOperation::Patch
        )
    {
        warnings.push(
            "Batch operation readback depends on the upstream response; use catalog/list or get tools for post-apply verification."
                .to_string(),
        );
    }
    warnings
}

fn soap_trafficking_plan_warnings(
    write_mode: GuardedActionRuntimeMode,
    scope: &str,
    request: &SoapTraffickingRequestArgs,
    plan: &SoapTraffickingPlan,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if plan.mutating && write_mode != GuardedActionRuntimeMode::Enabled {
        warnings.push(format!(
            "Mutating SOAP apply is disabled while GOOGLE_AD_MANAGER_MCP_WRITE_MODE is {}.",
            write_mode.as_str()
        ));
    }
    if !scope_allows_write(scope) {
        warnings.push(format!(
            "SOAP live calls require the Google Ad Manager manage scope: {MANAGE_SCOPE}. The legacy SOAP API does not accept the Ad Manager read-only scope."
        ));
    }
    if plan.mutating
        && request
            .expected_impact
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        warnings.push("Mutating SOAP apply requires expected_impact to be set.".to_string());
    }
    if plan.mutating
        && request
            .rollback_note
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        warnings.push("Mutating SOAP apply requires rollback_note to be set.".to_string());
    }
    if plan.destructive {
        warnings.push(
            "This SOAP operation is classified as destructive from the operation payload; review the action and PQL filter carefully."
                .to_string(),
        );
    } else if plan.send_adjacent {
        warnings.push(
            "This SOAP operation is adjacent to delivery, approval, reservation, or creative association behavior."
                .to_string(),
        );
    }
    warnings
}

fn provider_safety_contract_json() -> Value {
    json!({
        "defaults": {
            "read_tools": "enabled",
            "write_preview": "enabled unless runtime mode is read_only",
            "write_apply": "disabled unless runtime mode is enabled",
        },
        "apply_requirements": [
            "explicit write-enabled runtime mode",
            "Google Ad Manager manage OAuth scope",
            "confirmation token from a matching dry-run plan",
            "human-readable reason",
            "expected impact",
            "rollback or reversal note",
            "post-apply readback or explicit verification instruction"
        ],
        "never_default": [
            "live mutation",
            "production campaign launch",
            "implicit order or line-item trafficking",
            "generic SOAP proxying"
        ]
    })
}

fn rest_supported_resource_matrix() -> Value {
    json!([
        { "resource": "ad_spots", "operations": ["create", "patch", "batch_create", "batch_update"] },
        { "resource": "ad_units", "operations": ["create", "patch", "batch_create", "batch_update", "batch_activate", "batch_deactivate", "batch_archive"] },
        { "resource": "applications", "operations": ["create", "patch", "batch_create", "batch_update", "batch_archive", "batch_unarchive"] },
        { "resource": "cms_metadata_keys", "operations": ["batch_activate", "batch_deactivate"] },
        { "resource": "cms_metadata_values", "operations": ["batch_activate", "batch_deactivate"] },
        { "resource": "contacts", "operations": ["create", "patch", "batch_create", "batch_update"] },
        { "resource": "custom_fields", "operations": ["create", "patch", "batch_create", "batch_update", "batch_activate", "batch_deactivate"] },
        { "resource": "custom_targeting_keys", "operations": ["create", "patch", "batch_create", "batch_update", "batch_activate", "batch_deactivate"] },
        { "resource": "entity_signals_mappings", "operations": ["create", "patch", "batch_create", "batch_update"] },
        { "resource": "labels", "operations": ["create", "patch", "batch_create", "batch_update", "batch_activate", "batch_deactivate"] },
        { "resource": "placements", "operations": ["create", "patch", "batch_create", "batch_update", "batch_activate", "batch_deactivate", "batch_archive"] },
        { "resource": "private_auction_deals", "operations": ["create", "patch"] },
        { "resource": "private_auctions", "operations": ["create", "patch"] },
        { "resource": "reports", "operations": ["create", "patch"] },
        { "resource": "sites", "operations": ["create", "patch", "batch_create", "batch_update", "batch_deactivate", "batch_submit_for_approval"] },
        { "resource": "suggested_ad_unit", "operations": ["batch_approve"] },
        { "resource": "teams", "operations": ["create", "patch", "batch_create", "batch_update", "batch_activate", "batch_deactivate"] }
    ])
}

fn soap_trafficking_supported_operation_matrix() -> Value {
    json!([
        {
            "service": "OrderService",
            "operations": ["create_orders", "get_orders_by_statement", "perform_order_action", "update_orders"],
            "mutating_operations": ["create_orders", "perform_order_action", "update_orders"]
        },
        {
            "service": "LineItemService",
            "operations": ["create_line_items", "get_line_items_by_statement", "perform_line_item_action", "update_line_items"],
            "mutating_operations": ["create_line_items", "perform_line_item_action", "update_line_items"]
        },
        {
            "service": "CreativeService",
            "operations": ["create_creatives", "get_creatives_by_statement", "perform_creative_action", "update_creatives"],
            "mutating_operations": ["create_creatives", "perform_creative_action", "update_creatives"]
        },
        {
            "service": "LineItemCreativeAssociationService",
            "operations": [
                "create_line_item_creative_associations",
                "get_line_item_creative_associations_by_statement",
                "get_line_item_creative_association_preview_url",
                "get_line_item_creative_association_native_style_preview_urls",
                "perform_line_item_creative_association_action",
                "update_line_item_creative_associations"
            ],
            "mutating_operations": [
                "create_line_item_creative_associations",
                "perform_line_item_creative_association_action",
                "update_line_item_creative_associations"
            ]
        },
        {
            "service": "ForecastService",
            "operations": [
                "get_availability_forecast",
                "get_availability_forecast_by_id",
                "get_delivery_forecast",
                "get_delivery_forecast_by_ids",
                "get_traffic_data"
            ],
            "mutating_operations": []
        }
    ])
}

fn trafficking_gap_matrix() -> Value {
    json!([
        {
            "surface": "high_level_builders",
            "status": "not hand-modeled",
            "impact": "operators must provide the official SOAP payload XML for each typed operation",
            "follow_up": "add ergonomic builders/templates for common order, line item, creative, LICA, and forecast payloads"
        },
        {
            "surface": "typed_soap_response_models",
            "status": "raw bounded XML response",
            "impact": "agents can execute end-to-end but must inspect XML response fields directly",
            "follow_up": "parse common rval/update result/page response shapes into structured JSON alongside raw XML"
        },
        {
            "surface": "post_apply_readback_automation",
            "status": "manual follow-up operation",
            "impact": "mutating SOAP apply returns upstream response; operators should run get-by-statement or forecast checks for delivery proof",
            "follow_up": "support optional readback_request payloads on SOAP apply"
        }
    ])
}

fn network_catalog_ingest_columns() -> Vec<ScratchpadIngestColumn> {
    [
        ("collection", "string"),
        ("network_code", "string"),
        ("resource_name", "string"),
        ("resource_id", "string"),
        ("display_name", "string"),
        ("status", "string"),
        ("upstream_json", "string"),
    ]
    .into_iter()
    .map(|(name, logical_type)| ScratchpadIngestColumn {
        name: name.to_string(),
        logical_type: logical_type.to_string(),
    })
    .collect()
}

fn report_rows_ingest_columns() -> Vec<ScratchpadIngestColumn> {
    [
        ("row_index", "integer"),
        ("result_name", "string"),
        ("dimension_values_json", "string"),
        ("metric_values_json", "string"),
        ("values_json", "string"),
        ("upstream_json", "string"),
    ]
    .into_iter()
    .map(|(name, logical_type)| ScratchpadIngestColumn {
        name: name.to_string(),
        logical_type: logical_type.to_string(),
    })
    .collect()
}

fn network_catalog_rows_for_scratchpad(
    upstream: &Value,
    collection: CatalogCollection,
    network_code: &str,
) -> Vec<Map<String, Value>> {
    upstream
        .get(collection.response_field())
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| network_catalog_row_for_scratchpad(row, collection, network_code))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn network_catalog_row_for_scratchpad(
    row: &Value,
    collection: CatalogCollection,
    network_code: &str,
) -> Map<String, Value> {
    let resource_name = row
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut out = Map::new();
    out.insert(
        "collection".to_string(),
        Value::String(collection.as_str().to_string()),
    );
    out.insert(
        "network_code".to_string(),
        Value::String(network_code.to_string()),
    );
    out.insert(
        "resource_name".to_string(),
        Value::String(resource_name.clone()),
    );
    out.insert(
        "resource_id".to_string(),
        Value::String(resource_id_from_name(&resource_name)),
    );
    out.insert(
        "display_name".to_string(),
        row.get("displayName").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "status".to_string(),
        row.get("status").cloned().unwrap_or(Value::Null),
    );
    out.insert("upstream_json".to_string(), Value::String(row.to_string()));
    out
}

fn report_result_rows_for_scratchpad(
    upstream: &Value,
    result_name: &str,
) -> Vec<Map<String, Value>> {
    upstream
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .enumerate()
                .map(|(index, row)| report_result_row_for_scratchpad(row, index, result_name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn report_result_row_for_scratchpad(
    row: &Value,
    index: usize,
    result_name: &str,
) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("row_index".to_string(), json!(index));
    out.insert(
        "result_name".to_string(),
        Value::String(result_name.to_string()),
    );
    out.insert(
        "dimension_values_json".to_string(),
        Value::String(
            row.get("dimensionValues")
                .or_else(|| row.get("dimension_values"))
                .cloned()
                .unwrap_or(Value::Null)
                .to_string(),
        ),
    );
    out.insert(
        "metric_values_json".to_string(),
        Value::String(
            row.get("metricValues")
                .or_else(|| row.get("metric_values"))
                .cloned()
                .unwrap_or(Value::Null)
                .to_string(),
        ),
    );
    out.insert(
        "values_json".to_string(),
        Value::String(
            row.get("values")
                .cloned()
                .unwrap_or(Value::Null)
                .to_string(),
        ),
    );
    out.insert("upstream_json".to_string(), Value::String(row.to_string()));
    out
}

fn scratchpad_ingest_columns_to_json(columns: Vec<ScratchpadIngestColumn>) -> Vec<Value> {
    columns
        .into_iter()
        .map(|column| {
            json!({
                "name": column.name,
                "logical_type": column.logical_type,
            })
        })
        .collect()
}

fn scratchpad_session_info_to_json(info: ScratchpadSessionInfo) -> Value {
    json!({
        "session_id": info.session_id,
        "tables_used": info.tables_used,
        "tables_remaining": info.tables_remaining,
        "rows_used": info.rows_used,
        "rows_remaining": info.rows_remaining,
        "ttl_seconds_remaining": info.ttl_seconds_remaining,
    })
}

fn scratchpad_snapshot_to_json(snapshot: ScratchpadSessionSnapshot) -> Value {
    json!({
        "tables_used": snapshot.tables_used,
        "tables_remaining": snapshot.tables_remaining,
        "rows_used": snapshot.rows_used,
        "rows_remaining": snapshot.rows_remaining,
    })
}

fn scratchpad_table_info_to_json(table: ScratchpadTableInfo) -> Value {
    json!({
        "schema": table.schema,
        "name": table.name,
        "table_type": table.table_type,
        "column_count": table.column_count,
        "columns": table.columns
            .into_iter()
            .map(|column| json!({
                "name": column.name,
                "logical_type": column.logical_type,
                "nullable": column.nullable,
            }))
            .collect::<Vec<_>>(),
        "columns_truncated": table.columns_truncated,
    })
}

fn scratchpad_query_projection_to_json(
    projection: ScratchpadQueryProjection,
    offset: u64,
    page_size: u64,
) -> Value {
    json!({
        "rows": projection.rows,
        "row_count_total": projection.row_count_total,
        "columns": projection.columns
            .into_iter()
            .map(|column| json!({
                "name": column.name,
                "logical_type": column.logical_type,
                "nullable": column.nullable,
            }))
            .collect::<Vec<_>>(),
        "offset": offset,
        "page_size": page_size,
        "has_more": offset.saturating_add(page_size) < projection.row_count_total as u64,
        "pagination_mode": projection.pagination_mode,
        "query_hints": projection.query_hints,
    })
}

fn ingest_mode_label(mode: ScratchpadIngestMode) -> &'static str {
    match mode {
        ScratchpadIngestMode::Create => "create",
        ScratchpadIngestMode::Append => "append",
    }
}

fn resource_id_from_name(resource_name: &str) -> String {
    resource_name
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn quote_scratchpad_ident(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn append_evidence_table_error(
    bundle: &mut String,
    table_name: &str,
    err: &mcp_toolkit_scratchpad::ScratchpadError,
) {
    bundle.push_str(&format!("## `{table_name}`\n\n"));
    bundle.push_str(&format!(
        "- Error: `{}`\n\n",
        escape_markdown_cell(&err.to_string())
    ));
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| value.as_f64().map(|number| number as u64))
}

fn markdown_table(projection: &ScratchpadQueryProjection) -> String {
    if projection.columns.is_empty() {
        return "_No columns returned._\n".to_string();
    }
    let headers = projection
        .columns
        .iter()
        .map(|column| escape_markdown_cell(&column.name))
        .collect::<Vec<_>>();
    let mut out = String::new();
    out.push('|');
    out.push_str(&headers.join("|"));
    out.push_str("|\n|");
    out.push_str(&vec!["---"; headers.len()].join("|"));
    out.push_str("|\n");
    for row in &projection.rows {
        out.push('|');
        let values = projection
            .columns
            .iter()
            .map(|column| {
                row.get(&column.name)
                    .map(markdown_value)
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        out.push_str(&values.join("|"));
        out.push_str("|\n");
    }
    out
}

fn markdown_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => escape_markdown_cell(text),
        other => escape_markdown_cell(&other.to_string()),
    }
}

fn escape_markdown_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace('\r', "")
        .replace('\n', " ")
}

fn auth_next_steps(scope: &str, access_checked: bool) -> Vec<String> {
    let setup_plan = ad_manager_provider_auth_config(scope).adc_setup_plan();
    let mut steps = vec![
        format!(
            "Run `google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID>` if no credential source is configured, or run `{}`. This helper requests both {GCLOUD_ADC_REQUIRED_SCOPE} and {scope}.",
            setup_plan.headless_login.shell
        ),
        format!("If Google reports a quota-project problem, run `{}` and enable {AD_MANAGER_PROVIDER_API_SERVICE} on that project.", setup_plan.quota_project.shell),
        "Restart stdio MCP clients that keep long-lived server child processes after changing credentials or environment.".to_string(),
    ];
    if let Some(command) = setup_plan.api_enable {
        steps.push(format!(
            "Enable the Google Ad Manager API with `{}` if the quota project has not used it before.",
            command.shell
        ));
    }
    if !access_checked {
        steps.push("Call gam_auth_status with verify_access=true when you are ready to prove Ad Manager access.".to_string());
    }
    steps.push("Call gam_networks_list to discover the exact network code before using gam_network_catalog_list or gam_report_run.".to_string());
    steps
}

fn ad_manager_provider_auth_config(scope: &str) -> GoogleProviderAuthConfig {
    GoogleProviderAuthConfig::new(AD_MANAGER_PROVIDER_API_NAME, split_scopes(scope))
        .with_api_service_name(AD_MANAGER_PROVIDER_API_SERVICE)
}

fn split_scopes(scope: &str) -> Vec<String> {
    scope
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::split_scopes;

    #[test]
    fn split_scopes_accepts_common_delimiters() {
        assert_eq!(
            split_scopes("scope.a, scope.b\tscope.c\nscope.d"),
            vec![
                "scope.a".to_string(),
                "scope.b".to_string(),
                "scope.c".to_string(),
                "scope.d".to_string(),
            ]
        );
    }
}
