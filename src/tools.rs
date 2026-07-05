use std::time::Instant;

use mcp_toolkit::rmcp::handler::server::wrapper::Parameters;
use mcp_toolkit::rmcp::model::CallToolResult;
use mcp_toolkit::rmcp::{self, tool, tool_router};
use mcp_toolkit_scratchpad::{
    ScratchpadIngestColumn, ScratchpadIngestMode, ScratchpadQueryProjection, ScratchpadSessionInfo,
    ScratchpadSessionSnapshot, ScratchpadTableInfo,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::process::Command;

use crate::auth_ux::{gcloud_adc_login_command, shell_join};
use crate::client::CatalogCollection;
use crate::config::{GCLOUD_ADC_REQUIRED_SCOPE, adc_credentials_path};
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
                "goal": "Inspect Google Ad Manager networks, inventory, delivery catalog data, and saved report results through a read-only MCP surface.",
                "recommended_steps": [
                    "Run google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> for the easiest local browser login.",
                    format!("The login helper requests both {GCLOUD_ADC_REQUIRED_SCOPE} and {scope}, matching gcloud ADC requirements."),
                    "Restart any stdio MCP client that keeps a long-lived child process.",
                    "Call gam_auth_status with verify_access=true.",
                    "Call gam_networks_list to discover the exact network code.",
                    "Call gam_network_catalog_list for ad_units, orders, line_items, or reports.",
                    "Call gam_report_run for saved reports and gam_report_result_rows for large paginated results.",
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
                    "gam_scratchpad_open_session"
                ],
                "notes": [
                    "The server is read-only in the initial public release.",
                    "The official Google Ad Manager Beta REST API is the primary upstream surface.",
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
        let command = gcloud_adc_login_command(
            self.client().scope(),
            args.client_id_file.as_deref().map(std::path::Path::new),
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

    #[tool(
        name = "gam_scratchpad_open_session",
        description = "Open or refresh a bounded local DuckDB scratchpad session for Google Ad Manager evidence work."
    )]
    async fn gam_scratchpad_open_session(
        &self,
        Parameters(args): Parameters<ScratchpadSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        match self.scratchpad_sessions().open_session(&args.session_id) {
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
        match self.scratchpad_sessions().release_session(&args.session_id) {
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
        match self.scratchpad_sessions().list_sessions(limit) {
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
        match self
            .scratchpad_sessions()
            .list_tables(&args.session_id, limit)
        {
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
        match self.scratchpad_sessions().drop_table(
            &args.session_id,
            &args.table_name,
            args.if_exists,
        ) {
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
        match self
            .scratchpad_sessions()
            .query_rows(&args.session_id, &args.sql, offset, page_size)
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
        match self.scratchpad_sessions().ingest_rows_with_mode(
            &args.session_id,
            &args.table_name,
            &columns,
            &rows,
            ingest_mode,
        ) {
            Ok(stats) => Ok(contract::success(
                json!({
                    "session_id": args.session_id,
                    "table_name": args.table_name,
                    "mode": ingest_mode_label(ingest_mode),
                    "rows_inserted": stats.rows_inserted,
                    "columns_inserted": stats.columns_inserted,
                    "columns": scratchpad_ingest_columns_to_json(columns),
                    "session": scratchpad_snapshot_to_json(stats.session_snapshot),
                    "upstream_summary": {
                        "network_code": args.network_code,
                        "collection": args.collection.as_str(),
                        "response_field": args.collection.response_field(),
                        "row_count": rows.len(),
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
        match self.scratchpad_sessions().ingest_rows_with_mode(
            &args.session_id,
            &args.table_name,
            &columns,
            &rows,
            ingest_mode,
        ) {
            Ok(stats) => Ok(contract::success(
                json!({
                    "session_id": args.session_id,
                    "table_name": args.table_name,
                    "mode": ingest_mode_label(ingest_mode),
                    "rows_inserted": stats.rows_inserted,
                    "columns_inserted": stats.columns_inserted,
                    "columns": scratchpad_ingest_columns_to_json(columns),
                    "session": scratchpad_snapshot_to_json(stats.session_snapshot),
                    "upstream_summary": {
                        "result_name": args.result_name,
                        "row_count": rows.len(),
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
        let table_names = match args.tables {
            Some(tables) => tables,
            None => match self
                .scratchpad_sessions()
                .list_tables(&args.session_id, 100)
            {
                Ok(tables) => tables.into_iter().map(|table| table.name).collect(),
                Err(err) => return Ok(contract::scratchpad_error(err, started)),
            },
        };

        let mut bundle = format!(
            "# Google Ad Manager Scratchpad Evidence Bundle\n\n- Session: `{}`\n- Tables: `{}`\n- Sample rows per table: `{}`\n\n",
            args.session_id,
            table_names.len(),
            sample_rows,
        );
        let mut summaries = Vec::new();
        for table_name in table_names {
            let quoted = quote_scratchpad_ident(&table_name);
            let count_sql = format!("SELECT COUNT(*) AS row_count FROM {quoted}");
            let count_projection =
                match self
                    .scratchpad_sessions()
                    .query_rows(&args.session_id, &count_sql, 0, 1)
                {
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
            let sample_projection = match self.scratchpad_sessions().query_rows(
                &args.session_id,
                &sample_sql,
                0,
                sample_rows,
            ) {
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
    let mut steps = vec![
        format!("Run google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> if no credential source is configured. This helper requests both {GCLOUD_ADC_REQUIRED_SCOPE} and {scope}."),
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
        || adc_credentials_path().is_some_and(|path| path.is_file())
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
