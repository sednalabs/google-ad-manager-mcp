use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
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

use crate::auth_ux::{gcloud_adc_login_command, shell_join_with_cloudsdk_config};
use crate::client::{
    CatalogCollection, DEFAULT_SOAP_API_VERSION, RestWriteOperation, RestWritePlan,
    RestWriteResource, SoapTraffickingApplyResult, SoapTraffickingOperation, SoapTraffickingPlan,
    YieldGroupUpdateSoapRequest, soap_error_message_with_truncation, validate_soap_api_version,
};
use crate::config::{
    GCLOUD_ADC_REQUIRED_SCOPE, server_adc_credentials_path, server_cloudsdk_config_dir,
};
use crate::contract;
use crate::evidence::{EvidenceSource, EvidenceState, evidence_receipt_template};
use crate::fingerprint::stable_fingerprint;
use crate::{AdManagerError, AdManagerServer, MANAGE_SCOPE, McpError};
use mcp_toolkit_core::guarded_action::{
    GuardedActionApply, GuardedActionError, GuardedActionNoMutationProof, GuardedActionPlanSeed,
    GuardedActionPosture, GuardedActionPreview, GuardedActionRuntimeMode,
};
use mcp_toolkit_core::tool_inventory::{ToolOperation, ToolSearchFilter, ToolSearchResponse};

const AD_MANAGER_PROVIDER_API_NAME: &str = "Google Ad Manager API";
const AD_MANAGER_PROVIDER_API_SERVICE: &str = "admanager.googleapis.com";
const YIELD_GROUP_EXCLUSION_INCLUDE_DESCENDANTS: bool = true;
const DEPENDENCY_PLACEMENT_MATCH_SAMPLE_LIMIT: usize = 50;
const DEPENDENCY_PLACEMENT_MEMBER_SAMPLE_LIMIT: usize = 50;
const DEPENDENCY_TARGET_PLACEMENT_ID_LIMIT: usize = 200;
const DEPENDENCY_LINE_ITEM_MATCH_SAMPLE_LIMIT: usize = 50;
const DEPENDENCY_LINE_ITEM_XML_SAMPLE_BYTES: usize = 4096;
const PROBE_DIAGNOSTIC_SAMPLE_BYTES: usize = 800;
const PROBE_TRANSPORT_METADATA_SAMPLE_LIMIT: usize = 50;

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
    /// Use the conventional shared gcloud ADC file instead of a server-specific file.
    #[serde(default)]
    pub shared_adc: Option<bool>,
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
pub struct ExchangeProtectionProbeArgs {
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// Exact ad-unit codes to prove, for example Home_Page_LS or Search_Page_RS.
    pub ad_unit_codes: Vec<String>,
    /// Maximum rows to inspect from private auction, private deal, and yield group reads. Defaults to 100.
    pub page_size: Option<u32>,
    /// SOAP API version for YieldGroupService reads. Defaults to the current server default.
    pub api_version: Option<String>,
    /// Include bounded raw SOAP response XML in the yield-group summary. Defaults to false.
    #[serde(default)]
    pub include_raw: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AdUnitDependencyProbeArgs {
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// Exact ad-unit codes to resolve through the REST adUnits collection.
    #[serde(default)]
    pub ad_unit_codes: Vec<String>,
    /// Exact numeric ad-unit ids to include even when a code row is unavailable.
    #[serde(default)]
    pub ad_unit_ids: Vec<String>,
    /// SOAP API version for LineItemService reads. Defaults to the current server default.
    pub api_version: Option<String>,
    /// Number of LineItemService rows to fetch per SOAP page. Defaults to 500, max 1000.
    pub line_item_page_size: Option<u32>,
    /// Maximum LineItemService rows to inspect before reporting a capped proof. Defaults to 1000, max 5000.
    pub max_line_items: Option<u32>,
    /// Number of placement rows to inspect from REST. Defaults to 500, max 1000.
    pub placement_page_size: Option<u32>,
    /// Include bounded raw line-item XML snippets for matched line items. Defaults to false.
    #[serde(default)]
    pub include_line_item_xml: bool,
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
    /// Inner XML payload for the selected operation, excluding SOAP Envelope, Header, and operation wrapper. Empty string is accepted only for no-body reads such as get_yield_partners.
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

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct YieldGroupExclusionRequestArgs {
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// Numeric YieldGroupService yield group identifier.
    pub yield_group_id: String,
    /// Ad-unit IDs to ensure in YieldGroupService InventoryTargeting.excludedAdUnits. Requested entries are written descendant-safe with includeDescendants=true.
    pub excluded_ad_unit_ids: Vec<String>,
    /// SOAP API version, for example v202605. Defaults to the current server default.
    pub api_version: Option<String>,
    /// Include generated SOAP payload fragments in the response. Defaults to false.
    #[serde(default)]
    pub include_payload_xml: bool,
    /// Human-readable reason for the proposed yield-group exclusion change.
    pub reason: String,
    /// Expected advertiser, campaign, delivery, or operational impact. Required for apply.
    pub expected_impact: Option<String>,
    /// Rollback or reversal note. Required by policy for live apply review.
    pub rollback_note: Option<String>,
    /// Optional caller-supplied idempotency or ticket reference.
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct YieldGroupExclusionPreviewArgs {
    pub request: YieldGroupExclusionRequestArgs,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct YieldGroupExclusionApplyArgs {
    pub request: YieldGroupExclusionRequestArgs,
    /// Confirmation token returned by gam_yield_group_exclusions_preview for this exact request and readback fingerprint.
    pub confirmation_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoapPayloadTemplate {
    OrderById,
    LineItemById,
    LineItemsByOrderId,
    CreativesByAdvertiserName,
    LicasByLineItemId,
    LicaPreviewUrl,
    CreateLica,
    PauseLineItem,
    ResumeLineItem,
    ArchiveLineItem,
    DeliveryForecastByLineItemIds,
    AvailabilityForecastByLineItemId,
    YieldGroupsByStatement,
    YieldGroupsAll,
    YieldPartners,
}

impl SoapPayloadTemplate {
    fn as_str(self) -> &'static str {
        match self {
            Self::OrderById => "order_by_id",
            Self::LineItemById => "line_item_by_id",
            Self::LineItemsByOrderId => "line_items_by_order_id",
            Self::CreativesByAdvertiserName => "creatives_by_advertiser_name",
            Self::LicasByLineItemId => "licas_by_line_item_id",
            Self::LicaPreviewUrl => "lica_preview_url",
            Self::CreateLica => "create_lica",
            Self::PauseLineItem => "pause_line_item",
            Self::ResumeLineItem => "resume_line_item",
            Self::ArchiveLineItem => "archive_line_item",
            Self::DeliveryForecastByLineItemIds => "delivery_forecast_by_line_item_ids",
            Self::AvailabilityForecastByLineItemId => "availability_forecast_by_line_item_id",
            Self::YieldGroupsByStatement => "yield_groups_by_statement",
            Self::YieldGroupsAll => "yield_groups_all",
            Self::YieldPartners => "yield_partners",
        }
    }

    fn operation(self) -> SoapTraffickingOperation {
        match self {
            Self::OrderById => SoapTraffickingOperation::GetOrdersByStatement,
            Self::LineItemById | Self::LineItemsByOrderId => {
                SoapTraffickingOperation::GetLineItemsByStatement
            }
            Self::CreativesByAdvertiserName => SoapTraffickingOperation::GetCreativesByStatement,
            Self::LicasByLineItemId => {
                SoapTraffickingOperation::GetLineItemCreativeAssociationsByStatement
            }
            Self::LicaPreviewUrl => {
                SoapTraffickingOperation::GetLineItemCreativeAssociationPreviewUrl
            }
            Self::CreateLica => SoapTraffickingOperation::CreateLineItemCreativeAssociations,
            Self::PauseLineItem | Self::ResumeLineItem | Self::ArchiveLineItem => {
                SoapTraffickingOperation::PerformLineItemAction
            }
            Self::DeliveryForecastByLineItemIds => {
                SoapTraffickingOperation::GetDeliveryForecastByIds
            }
            Self::AvailabilityForecastByLineItemId => {
                SoapTraffickingOperation::GetAvailabilityForecastById
            }
            Self::YieldGroupsByStatement | Self::YieldGroupsAll => {
                SoapTraffickingOperation::GetYieldGroupsByStatement
            }
            Self::YieldPartners => SoapTraffickingOperation::GetYieldPartners,
        }
    }

    fn required_values(self) -> &'static [&'static str] {
        match self {
            Self::OrderById | Self::LineItemsByOrderId => &["order_id"],
            Self::LineItemById
            | Self::LicasByLineItemId
            | Self::PauseLineItem
            | Self::ResumeLineItem
            | Self::ArchiveLineItem
            | Self::AvailabilityForecastByLineItemId => &["line_item_id"],
            Self::CreativesByAdvertiserName => &["advertiser_id", "name_contains"],
            Self::LicaPreviewUrl | Self::CreateLica => &["line_item_id", "creative_id"],
            Self::DeliveryForecastByLineItemIds => &["line_item_ids"],
            Self::YieldGroupsByStatement => &["query"],
            Self::YieldGroupsAll | Self::YieldPartners => &[],
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SoapPayloadBuildArgs {
    /// Safe helper template to render into an inner SOAP payload_xml fragment.
    pub template: SoapPayloadTemplate,
    /// Template values such as order_id, line_item_id, creative_id, advertiser_id, name_contains, or line_item_ids.
    #[serde(default)]
    pub values: Map<String, Value>,
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
pub struct ScratchpadIngestSoapLineItemsArgs {
    /// Scratchpad session identifier.
    pub session_id: String,
    /// Scratchpad table name to create or append to.
    pub table_name: String,
    /// Raw Ad Manager network code, for example 1234567.
    pub network_code: String,
    /// Bounded PQL statement for LineItemService.getLineItemsByStatement.
    /// Must start with WHERE, ORDER BY, or LIMIT. Queries without LIMIT are capped automatically.
    pub query: String,
    /// SOAP API version, for example v202605. Defaults to the current server default.
    pub api_version: Option<String>,
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
                    "The login helper writes a Google Ad Manager-specific ADC file by default so other Google MCPs keep their own tokens and scopes.",
                    format!("The login helper requests both {GCLOUD_ADC_REQUIRED_SCOPE} and {scope}, matching gcloud ADC requirements."),
                    "Restart any stdio MCP client that keeps a long-lived child process.",
                    "Call gam_auth_status with verify_access=true.",
                    "Call gam_networks_list to discover the exact network code.",
                    "Call gam_network_catalog_list for ad_units, orders, line_items, placements, private_auctions, private_auction_deals, or reports.",
                    "Call gam_exchange_protection_probe when you need explicit partial-proof states for Exchange, private auction, private deal, or yield-group exposure.",
                    "Call gam_ad_unit_dependency_probe before ad-unit cleanup, archive, or retargeting work so placement and line-item dependencies are explicit.",
                    "Call gam_report_run for saved reports and gam_report_result_rows for large paginated results.",
                    "Call gam_trafficking_tool_matrix before planning writes so the REST and SOAP trafficking surfaces are explicit.",
                    "Use gam_rest_write_plan for dry-run write plans; gam_rest_write_apply only works when the server is explicitly started with GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled and the manage scope.",
                    "Use gam_soap_trafficking_plan and gam_soap_trafficking_apply for order, line-item, creative, LICA, and forecast SOAP workflows.",
                    "Use gam_yield_group_exclusions_preview and gam_yield_group_exclusions_apply when an existing yield group needs descendant-safe ad-unit exclusions with post-apply readback proof.",
                    "For local operator apply testing, rerun auth with google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID> --manage-scope.",
                    "Use gam_scratchpad_open_session plus the gam_scratchpad_ingest_* tools when you need local joins, filtering, evidence bundles, or larger result review."
                ],
                "supported_credential_sources": [
                    {
                        "name": "Server-specific Application Default Credentials",
                        "env": [],
                        "notes": "Recommended for local use through google-ad-manager-mcp auth login."
                    },
                    {
                        "name": "Conventional shared Application Default Credentials",
                        "env": [],
                        "notes": "Available for compatibility; prefer the server-specific auth login when multiple Google MCPs share one OS user."
                    },
                    {
                        "name": "Standard Google credential file",
                        "env": ["GOOGLE_APPLICATION_CREDENTIALS"],
                        "notes": "Useful when your Google auth library supports the file type; for this server, service account files are the portable unattended choice."
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
                    "gam_exchange_protection_probe",
                    "gam_ad_unit_dependency_probe",
                    "gam_trafficking_tool_matrix",
                    "gam_rest_write_plan",
                    "gam_soap_trafficking_plan",
                    "gam_yield_group_exclusions_preview",
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
        let headless_command = gcloud_adc_login_command(
            requested_scope,
            args.client_id_file.as_deref().map(std::path::Path::new),
            true,
        );
        let shared_adc = args.shared_adc.unwrap_or(false);
        let cloudsdk_config = if shared_adc {
            None
        } else {
            server_cloudsdk_config_dir()
        };
        if !shared_adc && cloudsdk_config.is_none() {
            return Ok(contract::error(
                AdManagerError::invalid(
                    "shared_adc",
                    "failed to determine the server-specific gcloud config directory; set HOME/XDG_CONFIG_HOME on Unix or APPDATA on Windows, or pass shared_adc=true to intentionally use conventional shared ADC",
                ),
                started,
            ));
        }
        let credential_file = if shared_adc {
            None
        } else {
            server_adc_credentials_path()
        };
        let quota_project = args
            .quota_project
            .clone()
            .or_else(|| self.settings().quota_project.clone());
        let follow_up_commands = quota_project
            .as_deref()
            .map(|project| {
                vec![shell_join_with_cloudsdk_config(
                    &google_adc_quota_project_command(project),
                    cloudsdk_config.as_deref(),
                )]
            })
            .unwrap_or_default();
        let shell_command = shell_join_with_cloudsdk_config(&command, cloudsdk_config.as_deref());
        let headless_shell_command =
            shell_join_with_cloudsdk_config(&headless_command, cloudsdk_config.as_deref());
        let client_id_file_command = setup_plan.login_with_client_id_file.argv.clone();
        let client_id_file_shell_command =
            shell_join_with_cloudsdk_config(&client_id_file_command, cloudsdk_config.as_deref());
        let client_id_file_headless_command =
            setup_plan.headless_login_with_client_id_file.argv.clone();
        let client_id_file_headless_shell_command = shell_join_with_cloudsdk_config(
            &client_id_file_headless_command,
            cloudsdk_config.as_deref(),
        );
        let quota_project_command = shell_join_with_cloudsdk_config(
            &setup_plan.quota_project.argv,
            cloudsdk_config.as_deref(),
        );
        Ok(contract::success(
            json!({
                "command": command,
                "shell_command": shell_command,
                "headless_command": headless_command,
                "headless_shell_command": headless_shell_command,
                "client_id_file_command": client_id_file_command,
                "client_id_file_shell_command": client_id_file_shell_command,
                "client_id_file_headless_command": client_id_file_headless_command,
                "client_id_file_headless_shell_command": client_id_file_headless_shell_command,
                "quota_project_command": quota_project_command,
                "api_enable_command": setup_plan.api_enable.as_ref().map(|command| command.shell.as_str()),
                "adc_scopes": setup_plan.scopes.clone(),
                "cloudsdk_config": cloudsdk_config.as_ref().map(|path| path.display().to_string()),
                "credential_file": credential_file.as_ref().map(|path| path.display().to_string()),
                "shared_adc": shared_adc,
                "follow_up_commands": follow_up_commands,
                "setup_next_steps": setup_plan.next_steps.clone(),
                "scope": requested_scope,
                "manage_scope": requested_scope == MANAGE_SCOPE,
                "next_steps": setup_plan.next_steps.clone(),
                "notes": [
                    "By default this command writes a Google Ad Manager-specific ADC file for this OS user.",
                    "Set shared_adc=true only when you intentionally want the conventional shared gcloud ADC file.",
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
        description = "List a curated Google Ad Manager network collection such as ad units, orders, line items, placements, private auctions, private auction deals, or reports."
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
        name = "gam_exchange_protection_probe",
        description = "Read-only proof for whether exact Ad Manager ad units appear exposed to AdSense, private auctions, private deals, or yield groups, while naming unsupported protection surfaces explicitly."
    )]
    async fn gam_exchange_protection_probe(
        &self,
        Parameters(args): Parameters<ExchangeProtectionProbeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let network_code = match parse_positive_id_string("network_code", &args.network_code) {
            Ok(network_code) => network_code,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let api_version = match validate_soap_api_version(args.api_version.as_deref()) {
            Ok(api_version) => api_version,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let page_size = args.page_size.unwrap_or(100).clamp(1, 1_000);
        let ad_unit_codes = match validate_probe_ad_unit_codes(&args.ad_unit_codes) {
            Ok(codes) => codes,
            Err(err) => return Ok(contract::error(err, started)),
        };

        let mut attention_reasons = Vec::new();
        let mut partial_reasons = vec![
            "GAM protections, inventory rules, and unified pricing rules are not fully exposed through the current API surface; this probe cannot prove UI-only surfaces.".to_string(),
        ];
        let mut ad_unit_summaries = Vec::new();
        let mut target_ad_units = Vec::new();

        for code in &ad_unit_codes {
            let payload = match self
                .client()
                .list_network_catalog(
                    &network_code,
                    CatalogCollection::AdUnits,
                    Some(10),
                    None,
                    Some(format!("adUnitCode = \"{code}\"")),
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
            let summary = summarize_probe_ad_unit(&network_code, code, &payload);
            if summary
                .get("decision")
                .and_then(Value::as_str)
                .is_some_and(|decision| decision == "attention_required")
            {
                attention_reasons.push(format!("ad unit {code} needs review"));
            }
            if summary
                .get("proof_complete")
                .and_then(Value::as_bool)
                .is_some_and(|value| !value)
            {
                partial_reasons.push(format!("ad unit {code} did not produce complete proof"));
            }
            if let Some(target) = probe_ad_unit_target_from_summary(&summary, &network_code) {
                target_ad_units.push(target);
            }
            ad_unit_summaries.push(summary);
        }

        let private_auctions = match self
            .client()
            .list_network_catalog(
                &network_code,
                CatalogCollection::PrivateAuctions,
                Some(page_size),
                None,
                None,
                None,
            )
            .await
        {
            Ok(value) => {
                let summary = summarize_probe_collection(
                    &value,
                    CatalogCollection::PrivateAuctions,
                    page_size,
                );
                apply_probe_collection_decision(
                    "private auctions",
                    &summary,
                    &mut attention_reasons,
                    &mut partial_reasons,
                );
                summary
            }
            Err(err) => {
                partial_reasons.push("private auction collection could not be read".to_string());
                blocked_probe_surface("private_auctions", err)
            }
        };

        let private_auction_deals = match self
            .client()
            .list_network_catalog(
                &network_code,
                CatalogCollection::PrivateAuctionDeals,
                Some(page_size),
                None,
                None,
                None,
            )
            .await
        {
            Ok(value) => {
                let summary = summarize_probe_collection(
                    &value,
                    CatalogCollection::PrivateAuctionDeals,
                    page_size,
                );
                apply_probe_collection_decision(
                    "private auction deals",
                    &summary,
                    &mut attention_reasons,
                    &mut partial_reasons,
                );
                summary
            }
            Err(err) => {
                partial_reasons
                    .push("private auction deals collection could not be read".to_string());
                blocked_probe_surface("private_auction_deals", err)
            }
        };

        let yield_groups = match probe_yield_groups(
            self,
            &network_code,
            Some(&api_version),
            page_size,
            &target_ad_units,
            args.include_raw,
        )
        .await
        {
            Ok(summary) => {
                match summary.get("decision").and_then(Value::as_str) {
                    Some("targeted_exposed") => {
                        attention_reasons.push(
                            "one or more active yield groups target a requested ad unit without a covering exclusion".to_string(),
                        );
                    }
                    Some("targeted_activity_unknown") => {
                        partial_reasons.push(
                            "one or more yield groups target a requested ad unit but activity status was not proven".to_string(),
                        );
                    }
                    Some("blocked") | Some("sample_only") | Some("skipped") => {
                        partial_reasons
                            .push("yield group proof is unavailable or incomplete".to_string());
                    }
                    _ => {}
                }
                summary
            }
            Err(err) => {
                partial_reasons.push("yield group proof failed before upstream call".to_string());
                blocked_probe_surface("yield_groups", err)
            }
        };

        let rest_discovery = match self.client().get_rest_discovery_document().await {
            Ok(value) => summarize_rest_discovery(&value),
            Err(err) => {
                partial_reasons.push("REST discovery document could not be read".to_string());
                blocked_probe_surface("rest_discovery", err)
            }
        };

        let unsupported_surfaces = unsupported_exchange_surfaces(&rest_discovery);
        let overall_decision = if !attention_reasons.is_empty() {
            "attention_required"
        } else if !partial_reasons.is_empty() {
            "partial_api_proof"
        } else {
            "api_exposed_surfaces_clear"
        };

        let response = json!({
            "network_code": &network_code,
            "overall_decision": overall_decision,
            "ad_units": ad_unit_summaries,
            "private_auctions": private_auctions,
            "private_auction_deals": private_auction_deals,
            "yield_groups": yield_groups,
            "rest_discovery": rest_discovery,
            "unsupported_or_unintegrated_surfaces": unsupported_surfaces,
            "attention_reasons": attention_reasons,
            "partial_reasons": partial_reasons,
            "certainty": {
                "can_prove_requested_ad_unit_flags": ad_unit_summaries.iter().all(|summary| summary.get("proof_complete").and_then(Value::as_bool).unwrap_or(false)),
                "can_prove_private_auction_absence_or_presence": private_auctions.get("proof_state").and_then(Value::as_str).is_some_and(|state| state == "complete_empty" || state == "complete_present"),
                "can_prove_private_deal_absence_or_presence": private_auction_deals.get("proof_state").and_then(Value::as_str).is_some_and(|state| state == "complete_empty" || state == "complete_present"),
                "can_prove_yield_group_targeting": yield_groups.get("proof_state").and_then(Value::as_str).is_some_and(|state| state == "complete"),
                "cannot_prove_via_current_api": [
                    "protections",
                    "inventory_rules",
                    "unified_pricing_rules"
                ]
            }
        });
        let response = match finalize_exchange_probe_response(response, &network_code) {
            Ok(response) => response,
            Err(err) => return Ok(contract::error(err, started)),
        };
        Ok(contract::success_with_meta(
            response,
            json!({
                "mutation_performed": false,
                "upstream_called": true,
                "page_size": page_size,
                "soap_default_api_version": DEFAULT_SOAP_API_VERSION,
                "required_yield_group_scope": MANAGE_SCOPE,
                "policy": provider_safety_contract_json(),
            }),
            started,
        ))
    }

    #[tool(
        name = "gam_ad_unit_dependency_probe",
        description = "Read-only dependency proof for exact Ad Manager ad units across placements and SOAP line-item inventory targeting."
    )]
    async fn gam_ad_unit_dependency_probe(
        &self,
        Parameters(args): Parameters<AdUnitDependencyProbeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        let network_code = match parse_positive_id_string("network_code", &args.network_code) {
            Ok(network_code) => network_code,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let api_version = match validate_soap_api_version(args.api_version.as_deref()) {
            Ok(api_version) => api_version,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let ad_unit_codes = match validate_optional_probe_ad_unit_codes(&args.ad_unit_codes) {
            Ok(codes) => codes,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let ad_unit_ids = match validate_probe_ad_unit_ids(&args.ad_unit_ids) {
            Ok(ids) => ids,
            Err(err) => return Ok(contract::error(err, started)),
        };
        if ad_unit_codes.len() + ad_unit_ids.len() > 50 {
            return Ok(contract::error(
                AdManagerError::invalid(
                    "ad_unit_codes",
                    "must include at most 50 combined ad-unit codes and ids",
                ),
                started,
            ));
        }
        if ad_unit_codes.is_empty() && ad_unit_ids.is_empty() {
            return Ok(contract::error(
                AdManagerError::invalid(
                    "ad_unit_codes",
                    "must include at least one exact ad-unit code or ad-unit id",
                ),
                started,
            ));
        }

        let line_item_page_size = args.line_item_page_size.unwrap_or(500).clamp(1, 1_000);
        let max_line_items = args.max_line_items.unwrap_or(1_000).clamp(1, 5_000);
        let placement_page_size = args.placement_page_size.unwrap_or(500).clamp(1, 1_000);

        let mut target_rows = Vec::new();
        let mut targets_by_id: BTreeMap<String, DependencyProbeTarget> = BTreeMap::new();
        let mut target_resolution_issues = Vec::new();

        for code in &ad_unit_codes {
            let payload = match self
                .client()
                .list_network_catalog(
                    &network_code,
                    CatalogCollection::AdUnits,
                    Some(10),
                    None,
                    Some(format!("adUnitCode = \"{code}\"")),
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
            let summary = summarize_dependency_ad_unit_code(&network_code, code, &payload);
            if let Some(issue) = dependency_target_resolution_issue(code, &summary) {
                target_resolution_issues.push(issue);
            }
            if let Some(target) = dependency_target_from_ad_unit_summary(&summary, &network_code) {
                targets_by_id
                    .entry(target.ad_unit_id.clone())
                    .and_modify(|existing| existing.merge_code_target(&target))
                    .or_insert(target);
            }
            target_rows.push(summary);
        }

        for ad_unit_id in &ad_unit_ids {
            if targets_by_id.contains_key(ad_unit_id) {
                continue;
            }
            let target = DependencyProbeTarget::id_only(ad_unit_id.clone());
            target_resolution_issues.push(format!(
                "ad unit id {ad_unit_id} was supplied without a resolved code row; ancestor targeting proof is incomplete"
            ));
            target_rows.push(target.to_summary_json());
            targets_by_id.insert(ad_unit_id.clone(), target);
        }

        let targets = targets_by_id.into_values().collect::<Vec<_>>();
        let placement_summary = match self
            .client()
            .list_network_catalog(
                &network_code,
                CatalogCollection::Placements,
                Some(placement_page_size),
                None,
                None,
                None,
            )
            .await
        {
            Ok(payload) => summarize_dependency_placements(
                &payload,
                &network_code,
                placement_page_size,
                &targets,
            ),
            Err(err) => blocked_probe_surface("placements", err),
        };

        let line_item_summary = match probe_ad_unit_line_item_dependencies(
            self,
            LineItemDependencyProbeOptions {
                network_code: &network_code,
                api_version: Some(&api_version),
                line_item_page_size,
                max_line_items,
                include_line_item_xml: args.include_line_item_xml,
            },
            &targets,
            &placement_summary,
        )
        .await
        {
            Ok(summary) => summary,
            Err(err) => blocked_probe_surface("line_items", err),
        };

        let proof_flags = dependency_proof_flags(
            &targets,
            &placement_summary,
            &line_item_summary,
            !target_resolution_issues.is_empty(),
        );
        let dependency_decision = dependency_probe_decision(
            &target_resolution_issues,
            &placement_summary,
            &line_item_summary,
        );
        let response = dependency_probe_response_json(
            &network_code,
            target_rows,
            placement_summary,
            line_item_summary,
            target_resolution_issues,
            proof_flags,
            dependency_decision,
        );
        let response = match finalize_dependency_probe_response(
            response,
            &network_code,
            dependency_decision,
        ) {
            Ok(response) => response,
            Err(err) => return Ok(contract::error(err, started)),
        };
        Ok(contract::success_with_meta(
            response,
            json!({
                "mutation_performed": false,
                "upstream_called": true,
                "line_item_page_size": line_item_page_size,
                "max_line_items": max_line_items,
                "placement_page_size": placement_page_size,
                "soap_default_api_version": DEFAULT_SOAP_API_VERSION,
                "required_line_item_scope": MANAGE_SCOPE,
                "policy": provider_safety_contract_json(),
            }),
            started,
        ))
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
        name = "gam_soap_payload_build",
        description = "Build a safe inner payload_xml fragment for common Google Ad Manager SOAP trafficking templates without calling upstream."
    )]
    async fn gam_soap_payload_build(
        &self,
        Parameters(args): Parameters<SoapPayloadBuildArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        match build_soap_payload_template(args.template, &args.values) {
            Ok(built) => Ok(contract::success(built, started)),
            Err(err) => Ok(contract::error(err, started)),
        }
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
        if plan.mutating
            && let Err(err) = validate_soap_apply_context(&args.request)
        {
            return Ok(contract::error(err, started));
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
        name = "gam_yield_group_exclusions_preview",
        description = "Read one YieldGroupService yield group and preview descendant-safe ad-unit IDs in InventoryTargeting.excludedAdUnits without mutating Google Ad Manager."
    )]
    async fn gam_yield_group_exclusions_preview(
        &self,
        Parameters(args): Parameters<YieldGroupExclusionPreviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        if let Err(err) = self.settings().write_mode.assert_allowed(
            "gam_yield_group_exclusions_preview",
            GuardedActionPosture::preview(),
        ) {
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
        if let Err(err) = validate_yield_group_exclusion_context(&args.request) {
            return Ok(contract::error(err, started));
        }

        let draft = match build_yield_group_exclusion_draft(self, &args.request).await {
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let (plan_id, confirmation_token, fingerprint) =
            match guarded_yield_group_exclusion_identifiers(&args.request, &draft) {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
        let preview = GuardedActionPreview::new(
            plan_id,
            self.settings().write_mode,
            GuardedActionPosture::preview(),
            json!({
                "request": yield_group_exclusion_request_summary(&args.request, &draft),
                "yield_group": yield_group_exclusion_draft_summary(&draft, args.request.include_payload_xml),
                "confirmation_token": confirmation_token,
                "fingerprint": fingerprint,
                "warnings": yield_group_exclusion_warnings(self.settings().write_mode, self.client().scope(), &args.request, &draft),
                "next_step": if draft.noop {
                    "No update is needed because every requested ad-unit ID is already excluded with includeDescendants=true by the yield group readback."
                } else {
                    "Review added_excluded_ad_unit_ids, updated_excluded_ad_unit_ids, requested_exclusion_include_descendants, current targeting summary, and payload hash. To apply, restart or run the server with GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled, use the manage scope, and call gam_yield_group_exclusions_apply with this exact request and confirmation_token."
                },
            }),
            json!({
                "mutation_performed": false,
                "upstream_called": true,
                "soap_default_api_version": DEFAULT_SOAP_API_VERSION,
                "required_soap_scope": MANAGE_SCOPE,
                "policy": provider_safety_contract_json(),
            }),
        );
        Ok(contract::success(json!(preview), started))
    }

    #[tool(
        name = "gam_yield_group_exclusions_apply",
        description = "Apply a previewed YieldGroupService descendant-safe ad-unit exclusion update after write-mode, manage-scope, confirmation-token, and readback gates."
    )]
    async fn gam_yield_group_exclusions_apply(
        &self,
        Parameters(args): Parameters<YieldGroupExclusionApplyArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        if let Err(err) = self.settings().write_mode.assert_allowed(
            "gam_yield_group_exclusions_apply",
            GuardedActionPosture::guarded_apply(),
        ) {
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
        if let Err(err) = validate_yield_group_exclusion_context(&args.request)
            .and_then(|_| validate_yield_group_exclusion_apply_context(&args.request))
        {
            return Ok(contract::error(err, started));
        }

        let draft = match build_yield_group_exclusion_draft(self, &args.request).await {
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let (plan_id, expected_token, fingerprint) =
            match guarded_yield_group_exclusion_identifiers(&args.request, &draft) {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
        if args.confirmation_token.trim() != expected_token {
            return Ok(contract::error(
                AdManagerError::ConfirmationTokenMismatch,
                started,
            ));
        }

        let mut apply_evidence = json!({
            "request": yield_group_exclusion_request_summary(&args.request, &draft),
            "preview_readback": yield_group_exclusion_draft_summary(&draft, args.request.include_payload_xml),
            "finish_state": "noop_proven",
            "update_response": null,
            "post_apply_readback": null,
        });
        if !draft.noop {
            let update_request = match self.client().build_yield_group_update_request(
                &args.request.network_code,
                args.request.api_version.as_deref(),
                &draft.update_payload_xml,
            ) {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
            let applied = match self
                .client()
                .execute_yield_group_update_request(&update_request)
                .await
            {
                Ok(value) => value,
                Err(err) => return Ok(contract::error(err, started)),
            };
            if applied.upstream_status >= 400 || applied.soap_fault.is_some() {
                return Ok(contract::error_with_detail(
                    AdManagerError::UpstreamApi {
                        status: applied.upstream_status,
                        message: crate::client::soap_error_message(&applied),
                    },
                    json!({
                        "yield_group_update_request": yield_group_update_request_to_json(&update_request, args.request.include_payload_xml),
                        "upstream_status": applied.upstream_status,
                        "response_truncated": applied.response_truncated,
                        "request_id": applied.request_id,
                        "response_time": applied.response_time,
                        "soap_fault": applied.soap_fault,
                    }),
                    started,
                ));
            }

            let readback = match build_yield_group_exclusion_draft(self, &args.request).await {
                Ok(value) => value,
                Err(err) => {
                    return Ok(contract::error_with_detail(
                        err,
                        json!({
                            "yield_group_update_request": yield_group_update_request_to_json(&update_request, args.request.include_payload_xml),
                            "upstream_status": applied.upstream_status,
                            "response_truncated": applied.response_truncated,
                            "request_id": applied.request_id,
                            "response_time": applied.response_time,
                            "soap_fault": applied.soap_fault,
                            "readback_state": "blocked_after_update",
                        }),
                        started,
                    ));
                }
            };
            if !readback.all_requested_ids_currently_excluded() {
                return Ok(contract::error_with_detail(
                    AdManagerError::UpstreamApi {
                        status: 500,
                        message: "yield group update response did not prove the requested exclusions on readback".to_string(),
                    },
                    json!({
                        "yield_group_update_request": yield_group_update_request_to_json(&update_request, args.request.include_payload_xml),
                        "upstream_status": applied.upstream_status,
                        "response_truncated": applied.response_truncated,
                        "request_id": applied.request_id,
                        "response_time": applied.response_time,
                        "soap_fault": applied.soap_fault,
                        "post_apply_readback": yield_group_exclusion_draft_summary(&readback, args.request.include_payload_xml),
                    }),
                    started,
                ));
            }

            apply_evidence["finish_state"] = json!("applied_proven");
            apply_evidence["update_response"] = json!({
                "yield_group_update_request": yield_group_update_request_to_json(&update_request, args.request.include_payload_xml),
                "upstream_status": applied.upstream_status,
                "response_truncated": applied.response_truncated,
                "request_id": applied.request_id,
                "response_time": applied.response_time,
                "soap_fault": applied.soap_fault,
            });
            apply_evidence["post_apply_readback"] =
                yield_group_exclusion_draft_summary(&readback, args.request.include_payload_xml);
        }

        let apply = GuardedActionApply::new(
            plan_id,
            self.settings().write_mode,
            GuardedActionPosture::guarded_apply(),
            apply_evidence,
            json!({
                "mutation_performed": !draft.noop,
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
                "soap_payload_builder": "gam_soap_payload_build",
                "soap_trafficking_tools": [
                    "gam_soap_trafficking_plan",
                    "gam_soap_trafficking_apply"
                ],
                "yield_group_exclusion_tools": [
                    "gam_yield_group_exclusions_preview",
                    "gam_yield_group_exclusions_apply"
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
                    "The yield-group exclusion tools provide a typed read-modify-write path for descendant-safe YieldGroupService ad-unit exclusions with post-apply readback proof.",
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
        name = "gam_scratchpad_ingest_soap_line_items",
        description = "Run a bounded read-only LineItemService SOAP query and ingest parsed line-item delivery rows into a Google Ad Manager scratchpad table."
    )]
    async fn gam_scratchpad_ingest_soap_line_items(
        &self,
        Parameters(args): Parameters<ScratchpadIngestSoapLineItemsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let started = Instant::now();
        if !scope_allows_write(self.client().scope()) {
            return Ok(contract::error(
                AdManagerError::WriteScopeRequired {
                    scope: self.client().scope().to_string(),
                },
                started,
            ));
        }
        let query = match bounded_line_item_pql_query(&args.query) {
            Ok(query) => query,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let payload_xml = pql_payload(&query);
        let plan = match self.client().build_soap_trafficking_plan(
            &args.network_code,
            args.api_version.as_deref(),
            SoapTraffickingOperation::GetLineItemsByStatement,
            &payload_xml,
        ) {
            Ok(plan) => plan,
            Err(err) => return Ok(contract::error(err, started)),
        };
        let applied = match self.client().execute_soap_trafficking_plan(&plan).await {
            Ok(value) => value,
            Err(err) => return Ok(contract::error(err, started)),
        };
        if applied.upstream_status >= 400 || applied.soap_fault.is_some() {
            return Ok(contract::error_with_detail(
                AdManagerError::UpstreamApi {
                    status: applied.upstream_status,
                    message: crate::client::soap_error_message(&applied),
                },
                json!({
                    "upstream_status": applied.upstream_status,
                    "request_id": applied.request_id,
                    "response_time": applied.response_time,
                    "soap_fault": applied.soap_fault,
                    "response_truncated": applied.response_truncated,
                }),
                started,
            ));
        }

        let columns = soap_line_items_ingest_columns();
        let rows = soap_line_item_rows_for_scratchpad(
            &applied.upstream_response_xml,
            &args.network_code,
            plan.api_version.as_str(),
            applied.request_id.as_deref(),
            applied.response_time.as_deref(),
            applied.response_truncated,
        );
        let ingest_mode = if args.append {
            ScratchpadIngestMode::Append
        } else {
            ScratchpadIngestMode::Create
        };
        let total_result_set_size =
            extract_xml_tag_text(&applied.upstream_response_xml, "totalResultSetSize")
                .and_then(|value| value.parse::<u64>().ok());
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
                        "api_version": plan.api_version,
                        "operation": "get_line_items_by_statement",
                        "query": args.query,
                        "effective_query": query,
                        "row_count": row_count,
                        "total_result_set_size": total_result_set_size,
                        "sample_only": total_result_set_size.map(|total| total > row_count as u64).unwrap_or(applied.response_truncated),
                        "response_truncated": applied.response_truncated,
                        "request_id": applied.request_id,
                        "response_time": applied.response_time,
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

fn validate_probe_ad_unit_codes(codes: &[String]) -> Result<Vec<String>, AdManagerError> {
    if codes.is_empty() {
        return Err(AdManagerError::invalid(
            "ad_unit_codes",
            "must contain at least one exact ad-unit code",
        ));
    }
    if codes.len() > 50 {
        return Err(AdManagerError::invalid(
            "ad_unit_codes",
            "must contain at most 50 ad-unit codes",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut cleaned = Vec::with_capacity(codes.len());
    for code in codes {
        let trimmed = code.trim();
        if trimmed.is_empty() || trimmed.len() > 128 {
            return Err(AdManagerError::invalid(
                "ad_unit_codes",
                "each code must be between 1 and 128 characters",
            ));
        }
        if !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':'))
        {
            return Err(AdManagerError::invalid(
                "ad_unit_codes",
                "codes may only contain ASCII letters, digits, underscore, hyphen, dot, slash, or colon",
            ));
        }
        if !seen.insert(trimmed.to_string()) {
            return Err(AdManagerError::invalid(
                "ad_unit_codes",
                format!("duplicate ad-unit code `{trimmed}`"),
            ));
        }
        cleaned.push(trimmed.to_string());
    }
    Ok(cleaned)
}

fn validate_optional_probe_ad_unit_codes(codes: &[String]) -> Result<Vec<String>, AdManagerError> {
    if codes.is_empty() {
        return Ok(Vec::new());
    }
    validate_probe_ad_unit_codes(codes)
}

fn validate_probe_ad_unit_ids(ids: &[String]) -> Result<Vec<String>, AdManagerError> {
    if ids.len() > 50 {
        return Err(AdManagerError::invalid(
            "ad_unit_ids",
            "must contain at most 50 ad-unit ids",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut cleaned = Vec::with_capacity(ids.len());
    for id in ids {
        let trimmed = id.trim();
        let canonical_id = match trimmed.parse::<u64>() {
            Ok(value) if value > 0 => value.to_string(),
            _ => {
                return Err(AdManagerError::invalid(
                    "ad_unit_ids",
                    "each ad-unit id must be a positive numeric identifier",
                ));
            }
        };
        if trimmed.is_empty() || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(AdManagerError::invalid(
                "ad_unit_ids",
                "each ad-unit id must be a positive numeric identifier",
            ));
        }
        if !seen.insert(canonical_id.clone()) {
            return Err(AdManagerError::invalid(
                "ad_unit_ids",
                format!("duplicate ad-unit id `{canonical_id}`"),
            ));
        }
        cleaned.push(canonical_id);
    }
    Ok(cleaned)
}

#[derive(Debug, Clone)]
struct DependencyProbeTarget {
    ad_unit_id: String,
    ad_unit_codes: BTreeSet<String>,
    resource_name: Option<String>,
    display_name: Value,
    status: Value,
    ad_unit_sizes: Value,
    ancestor_ad_unit_ids: BTreeSet<String>,
    proof_state: &'static str,
    proof_notes: Vec<String>,
}

impl DependencyProbeTarget {
    fn id_only(ad_unit_id: String) -> Self {
        Self {
            ad_unit_id,
            ad_unit_codes: BTreeSet::new(),
            resource_name: None,
            display_name: Value::Null,
            status: Value::Null,
            ad_unit_sizes: Value::Null,
            ancestor_ad_unit_ids: BTreeSet::new(),
            proof_state: "id_only",
            proof_notes: vec![
                "ancestor targeting cannot be proven for an id-only target unless a code row is also resolved"
                    .to_string(),
            ],
        }
    }

    fn to_probe_target(&self) -> ProbeAdUnitTarget {
        ProbeAdUnitTarget {
            ad_unit_id: self.ad_unit_id.clone(),
            ancestor_ad_unit_ids: self.ancestor_ad_unit_ids.clone(),
        }
    }

    fn merge_code_target(&mut self, other: &Self) {
        self.ad_unit_codes
            .extend(other.ad_unit_codes.iter().cloned());
        self.ancestor_ad_unit_ids
            .extend(other.ancestor_ad_unit_ids.iter().cloned());
        if self.resource_name.is_none() {
            self.resource_name = other.resource_name.clone();
        }
        if self.display_name.is_null() {
            self.display_name = other.display_name.clone();
        }
        if self.status.is_null() {
            self.status = other.status.clone();
        }
        if self.ad_unit_sizes.is_null() {
            self.ad_unit_sizes = other.ad_unit_sizes.clone();
        }
        if self.proof_state == "id_only" && other.proof_state == "resolved_exact" {
            self.proof_state = "resolved_exact";
        }
        self.proof_notes.extend(other.proof_notes.iter().cloned());
        self.proof_notes.sort();
        self.proof_notes.dedup();
    }

    fn id_only_without_ancestors(&self) -> bool {
        self.ad_unit_codes.is_empty() && self.ancestor_ad_unit_ids.is_empty()
    }

    fn to_summary_json(&self) -> Value {
        json!({
            "ad_unit_id": self.ad_unit_id,
            "ad_unit_codes": self.ad_unit_codes.iter().cloned().collect::<Vec<_>>(),
            "resource_name": self.resource_name,
            "display_name": self.display_name,
            "status": self.status,
            "ad_unit_sizes": self.ad_unit_sizes,
            "ancestor_ad_unit_ids": self.ancestor_ad_unit_ids.iter().cloned().collect::<Vec<_>>(),
            "proof_state": self.proof_state,
            "proof_notes": self.proof_notes,
        })
    }
}

fn summarize_dependency_ad_unit_code(network_code: &str, code: &str, payload: &Value) -> Value {
    let rows = catalog_rows(payload, CatalogCollection::AdUnits);
    let matches = rows
        .into_iter()
        .filter(|row| {
            row.get("adUnitCode")
                .and_then(Value::as_str)
                .is_some_and(|value| value == code)
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return json!({
            "ad_unit_code": code,
            "proof_state": "missing",
            "reason": "exact ad-unit code was not returned by GAM",
            "matches": 0,
        });
    }
    if matches.len() > 1 {
        return json!({
            "ad_unit_code": code,
            "proof_state": "ambiguous",
            "reason": "GAM returned multiple rows for the exact ad-unit code",
            "matches": matches.len(),
        });
    }
    let row = matches[0];
    let resource_name = row.get("name").and_then(Value::as_str).unwrap_or_default();
    let (ancestor_ad_unit_ids, ancestor_identity_complete) =
        ad_unit_ancestor_ids(row, network_code);
    let Some(ad_unit_id) = exact_ad_unit_id_from_resource_name(network_code, resource_name) else {
        return json!({
            "ad_unit_code": code,
            "ad_unit_id": Value::Null,
            "resource_name": resource_name,
            "display_name": row.get("displayName").cloned().unwrap_or(Value::Null),
            "status": row.get("status").cloned().unwrap_or(Value::Null),
            "ad_unit_sizes": row.get("adUnitSizes").or_else(|| row.get("sizes")).cloned().unwrap_or(Value::Null),
            "ancestor_ad_unit_ids": ancestor_ad_unit_ids,
            "ancestor_identity_complete": ancestor_identity_complete,
            "proof_state": "invalid_resource_name",
            "reason": "exact ad-unit code resolved outside the requested canonical network/resource scope",
        });
    };
    json!({
        "ad_unit_code": code,
        "ad_unit_id": ad_unit_id,
        "resource_name": resource_name,
        "display_name": row.get("displayName").cloned().unwrap_or(Value::Null),
        "status": row.get("status").cloned().unwrap_or(Value::Null),
        "ad_unit_sizes": row.get("adUnitSizes").or_else(|| row.get("sizes")).cloned().unwrap_or(Value::Null),
        "ancestor_ad_unit_ids": ancestor_ad_unit_ids,
        "ancestor_identity_complete": ancestor_identity_complete,
        "proof_state": "resolved_exact",
    })
}

fn dependency_target_from_ad_unit_summary(
    summary: &Value,
    network_code: &str,
) -> Option<DependencyProbeTarget> {
    if summary.get("proof_state").and_then(Value::as_str) != Some("resolved_exact") {
        return None;
    }
    let ad_unit_id = summary.get("ad_unit_id").and_then(Value::as_str)?;
    let resource_name = summary.get("resource_name").and_then(Value::as_str)?;
    if exact_ad_unit_id_from_resource_name(network_code, resource_name).as_deref()
        != Some(ad_unit_id)
    {
        return None;
    }
    let mut codes = BTreeSet::new();
    if let Some(code) = summary.get("ad_unit_code").and_then(Value::as_str) {
        codes.insert(code.to_string());
    }
    let ancestor_ad_unit_ids = summary
        .get("ancestor_ad_unit_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|value| exact_ad_unit_id_from_candidate(network_code, value))
        .collect::<BTreeSet<_>>();
    let mut proof_notes = Vec::new();
    if summary
        .get("ancestor_identity_complete")
        .and_then(Value::as_bool)
        == Some(false)
    {
        proof_notes.push(
            "ancestor identities were malformed or outside the requested network".to_string(),
        );
    }
    Some(DependencyProbeTarget {
        ad_unit_id: ad_unit_id.to_string(),
        ad_unit_codes: codes,
        resource_name: Some(resource_name.to_string()),
        display_name: summary.get("display_name").cloned().unwrap_or(Value::Null),
        status: summary.get("status").cloned().unwrap_or(Value::Null),
        ad_unit_sizes: summary.get("ad_unit_sizes").cloned().unwrap_or(Value::Null),
        ancestor_ad_unit_ids,
        proof_state: "resolved_exact",
        proof_notes,
    })
}

fn dependency_target_resolution_issue(code: &str, summary: &Value) -> Option<String> {
    if summary.get("proof_state").and_then(Value::as_str) != Some("resolved_exact") {
        Some(format!("ad unit code {code} did not resolve exactly"))
    } else if summary
        .get("ancestor_identity_complete")
        .and_then(Value::as_bool)
        == Some(false)
    {
        Some(format!(
            "ad unit code {code} returned malformed or foreign ancestor identities"
        ))
    } else {
        None
    }
}

fn summarize_probe_ad_unit(network_code: &str, code: &str, payload: &Value) -> Value {
    let rows = catalog_rows(payload, CatalogCollection::AdUnits);
    let matches = rows
        .into_iter()
        .filter(|row| {
            row.get("adUnitCode")
                .and_then(Value::as_str)
                .is_some_and(|value| value == code)
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return json!({
            "ad_unit_code": code,
            "decision": "attention_required",
            "proof_state": "missing",
            "proof_complete": false,
            "reason": "exact ad-unit code was not returned by GAM",
            "matches": 0,
        });
    }
    if matches.len() > 1 {
        return json!({
            "ad_unit_code": code,
            "decision": "attention_required",
            "proof_state": "ambiguous",
            "proof_complete": false,
            "reason": "GAM returned multiple rows for the exact ad-unit code",
            "matches": matches.len(),
        });
    }
    let row = matches[0];
    let resource_name = row.get("name").and_then(Value::as_str).unwrap_or_default();
    let ad_unit_id = exact_ad_unit_id_from_resource_name(network_code, resource_name);
    let target_resolved_exact = ad_unit_id.is_some();
    let (ancestor_ad_unit_ids, ancestor_identity_complete) =
        ad_unit_ancestor_ids(row, network_code);
    let applied_adsense = row.get("appliedAdsenseEnabled").and_then(Value::as_bool);
    let effective_adsense = row.get("effectiveAdsenseEnabled").and_then(Value::as_bool);
    let proof_complete = target_resolved_exact
        && ancestor_identity_complete
        && applied_adsense.is_some()
        && effective_adsense.is_some();
    let decision = if !target_resolved_exact {
        "partial_api_proof"
    } else if applied_adsense == Some(true) || effective_adsense == Some(true) {
        "attention_required"
    } else if proof_complete {
        "clear_on_exposed_flags"
    } else {
        "partial_api_proof"
    };
    json!({
        "ad_unit_code": code,
        "ad_unit_id": ad_unit_id,
        "proof_state": if target_resolved_exact { "resolved_exact" } else { "invalid_resource_name" },
        "ancestor_ad_unit_ids": ancestor_ad_unit_ids,
        "ancestor_identity_complete": ancestor_identity_complete,
        "resource_name": resource_name,
        "display_name": row.get("displayName").cloned().unwrap_or(Value::Null),
        "status": row.get("status").cloned().unwrap_or(Value::Null),
        "ad_unit_sizes": row.get("adUnitSizes").or_else(|| row.get("sizes")).cloned().unwrap_or(Value::Null),
        "applied_adsense_enabled": applied_adsense,
        "effective_adsense_enabled": effective_adsense,
        "explicitly_targeted": row.get("explicitlyTargeted").and_then(Value::as_bool),
        "decision": decision,
        "proof_complete": proof_complete,
    })
}

fn ad_unit_ancestor_ids(row: &Value, network_code: &str) -> (Vec<String>, bool) {
    let mut ids = BTreeSet::new();
    let mut identity_complete = true;
    for key in [
        "parentAdUnit",
        "parentAdUnits",
        "parentPath",
        "ancestorAdUnits",
        "adUnitParent",
    ] {
        if let Some(value) = row.get(key) {
            identity_complete &= collect_network_bound_ad_unit_ids(value, network_code, &mut ids);
        }
    }
    (ids.into_iter().collect(), identity_complete)
}

fn collect_network_bound_ad_unit_ids(
    value: &Value,
    network_code: &str,
    ids: &mut BTreeSet<String>,
) -> bool {
    match value {
        Value::String(raw) => {
            if let Some(id) = exact_ad_unit_id_from_candidate(network_code, raw) {
                ids.insert(id);
                true
            } else {
                false
            }
        }
        Value::Number(number) => number.as_u64().is_some_and(|id| {
            if id == 0 {
                return false;
            }
            ids.insert(id.to_string());
            true
        }),
        Value::Array(values) => {
            let mut complete = true;
            for value in values {
                complete &= collect_network_bound_ad_unit_ids(value, network_code, ids);
            }
            complete
        }
        Value::Object(object) => {
            let mut candidate_ids = BTreeSet::new();
            let mut complete = true;
            let mut recognized = false;
            for key in ["adUnitId", "adUnit", "name", "parentAdUnit", "id"] {
                if let Some(value) = object.get(key) {
                    recognized = true;
                    complete &=
                        collect_network_bound_ad_unit_ids(value, network_code, &mut candidate_ids);
                }
            }
            let valid = recognized && complete && candidate_ids.len() == 1;
            if valid {
                ids.extend(candidate_ids);
            }
            valid
        }
        _ => false,
    }
}

fn exact_ad_unit_id_from_candidate(network_code: &str, value: &str) -> Option<String> {
    if value.contains('/') {
        return exact_ad_unit_id_from_resource_name(network_code, value);
    }
    let id = value.parse::<u64>().ok()?;
    (id > 0 && id.to_string() == value).then(|| id.to_string())
}

fn catalog_rows(payload: &Value, collection: CatalogCollection) -> Vec<&Value> {
    payload
        .get(collection.response_field())
        .and_then(Value::as_array)
        .map(|rows| rows.iter().collect())
        .unwrap_or_default()
}

fn summarize_probe_collection(
    payload: &Value,
    collection: CatalogCollection,
    page_size: u32,
) -> Value {
    let rows = catalog_rows(payload, collection);
    let next_page_token = payload
        .get("nextPageToken")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let capped = next_page_token.is_some() || rows.len() as u32 >= page_size;
    let proof_state = if capped {
        "sample_only"
    } else if rows.is_empty() {
        "complete_empty"
    } else {
        "complete_present"
    };
    json!({
        "collection": collection.as_str(),
        "proof_state": proof_state,
        "row_count_in_page": rows.len(),
        "page_size": page_size,
        "next_page_token_present": next_page_token.is_some(),
        "capped_or_possibly_more": capped,
        "sample": rows.iter().take(5).map(|row| {
            let resource_name = row.get("name").and_then(Value::as_str).unwrap_or_default();
            json!({
                "resource_name": resource_name,
                "resource_id": resource_id_from_name(resource_name),
                "display_name": row.get("displayName").or_else(|| row.get("name")).cloned().unwrap_or(Value::Null),
                "status": row.get("status").cloned().unwrap_or(Value::Null),
            })
        }).collect::<Vec<_>>(),
    })
}

fn summarize_dependency_placements(
    payload: &Value,
    network_code: &str,
    page_size: u32,
    targets: &[DependencyProbeTarget],
) -> Value {
    let rows = catalog_rows(payload, CatalogCollection::Placements);
    let next_page_token = payload
        .get("nextPageToken")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let capped = next_page_token.is_some() || rows.len() as u32 >= page_size;
    let mut target_placement_ids: BTreeMap<String, BTreeSet<String>> = targets
        .iter()
        .map(|target| (target.ad_unit_id.clone(), BTreeSet::new()))
        .collect();
    let mut placement_matches = Vec::new();
    let mut placement_match_count = 0_usize;
    let mut placement_matches_truncated = false;
    let mut target_placement_ids_truncated = false;
    let mut unknown_membership_rows = Vec::new();

    let row_count = rows.len();
    for row in rows {
        let resource_name = row.get("name").and_then(Value::as_str).unwrap_or_default();
        let Some(placement_id) =
            exact_resource_id_from_name(network_code, "placements", resource_name)
        else {
            unknown_membership_rows.push(json!({
                "placement_id": Value::Null,
                "resource_name": resource_name,
                "display_name": row.get("displayName").or_else(|| row.get("name")).cloned().unwrap_or(Value::Null),
                "reason": "placement resource identity was malformed or outside the requested network",
            }));
            continue;
        };
        let (member_ad_unit_ids, membership_shape_seen, membership_identity_complete) =
            placement_member_ad_unit_ids(row, network_code);
        if !membership_shape_seen || !membership_identity_complete {
            unknown_membership_rows.push(json!({
                "placement_id": placement_id,
                "resource_name": resource_name,
                "display_name": row.get("displayName").or_else(|| row.get("name")).cloned().unwrap_or(Value::Null),
                "reason": if membership_shape_seen {
                    "placement membership contained malformed or foreign ad-unit identities"
                } else {
                    "placement membership shape was not exposed"
                },
            }));
        }
        let matched_target_ids = targets
            .iter()
            .filter(|target| member_ad_unit_ids.contains(&target.ad_unit_id))
            .map(|target| target.ad_unit_id.clone())
            .collect::<Vec<_>>();
        if matched_target_ids.is_empty() {
            continue;
        }
        for ad_unit_id in &matched_target_ids {
            let ids = target_placement_ids.entry(ad_unit_id.clone()).or_default();
            if ids.len() < DEPENDENCY_TARGET_PLACEMENT_ID_LIMIT {
                ids.insert(placement_id.clone());
            } else {
                target_placement_ids_truncated = true;
            }
        }
        placement_match_count += 1;
        if placement_matches.len() < DEPENDENCY_PLACEMENT_MATCH_SAMPLE_LIMIT {
            let member_ad_unit_count = member_ad_unit_ids.len();
            let member_ad_unit_ids_sample = member_ad_unit_ids
                .into_iter()
                .take(DEPENDENCY_PLACEMENT_MEMBER_SAMPLE_LIMIT)
                .collect::<Vec<_>>();
            placement_matches.push(json!({
                "placement_id": placement_id,
                "resource_name": resource_name,
                "display_name": row.get("displayName").or_else(|| row.get("name")).cloned().unwrap_or(Value::Null),
                "status": row.get("status").cloned().unwrap_or(Value::Null),
                "matched_ad_unit_ids": matched_target_ids,
                "member_ad_unit_count": member_ad_unit_count,
                "member_ad_unit_ids_sample": member_ad_unit_ids_sample,
                "member_ad_unit_ids_truncated": member_ad_unit_count > DEPENDENCY_PLACEMENT_MEMBER_SAMPLE_LIMIT,
                "membership_identity_complete": membership_identity_complete,
            }));
        } else {
            placement_matches_truncated = true;
        }
    }

    let proof_state =
        if capped || !unknown_membership_rows.is_empty() || target_placement_ids_truncated {
            "sample_or_shape_incomplete"
        } else {
            "complete_for_page"
        };
    let mut target_map = Map::new();
    for (ad_unit_id, placement_ids) in target_placement_ids {
        target_map.insert(
            ad_unit_id,
            Value::Array(placement_ids.into_iter().map(Value::String).collect()),
        );
    }
    json!({
        "surface": "placements",
        "proof_state": proof_state,
        "row_count_in_page": row_count,
        "page_size": page_size,
        "next_page_token_present": next_page_token.is_some(),
        "capped_or_possibly_more": capped,
        "membership_shape_unknown_count": unknown_membership_rows.len(),
        "membership_shape_unknown_sample": unknown_membership_rows.into_iter().take(10).collect::<Vec<_>>(),
        "target_placement_match_count": placement_match_count,
        "target_placement_matches_truncated": placement_matches_truncated,
        "target_placement_id_limit_per_ad_unit": DEPENDENCY_TARGET_PLACEMENT_ID_LIMIT,
        "target_placement_ids_truncated": target_placement_ids_truncated,
        "target_placement_ids_by_ad_unit_id": target_map,
        "target_placement_matches_sample": placement_matches,
        "mutation_performed": false,
    })
}

fn placement_member_ad_unit_ids(row: &Value, network_code: &str) -> (BTreeSet<String>, bool, bool) {
    let mut ids = BTreeSet::new();
    let mut membership_shape_seen = false;
    let mut identity_complete = true;
    if let Some(object) = row.as_object() {
        for key in [
            "adUnitAssignments",
            "assignedAdUnits",
            "adUnits",
            "targetedAdUnits",
            "adUnitIds",
        ] {
            if let Some(value) = object.get(key) {
                membership_shape_seen = true;
                identity_complete &=
                    collect_network_bound_ad_unit_ids(value, network_code, &mut ids);
            }
        }
    }
    (ids, membership_shape_seen, identity_complete)
}

fn apply_probe_collection_decision(
    label: &str,
    summary: &Value,
    attention_reasons: &mut Vec<String>,
    partial_reasons: &mut Vec<String>,
) {
    match summary.get("proof_state").and_then(Value::as_str) {
        Some("complete_present") => attention_reasons.push(format!(
            "{label} are present; review whether they can target the requested inventory"
        )),
        Some("sample_only") => partial_reasons.push(format!(
            "{label} read is capped or paginated; full absence/presence is not proven"
        )),
        Some("blocked") => partial_reasons.push(format!("{label} read is blocked")),
        _ => {}
    }
}

fn blocked_probe_surface(surface: &str, err: AdManagerError) -> Value {
    let block_class = probe_error_block_class(&err);
    let (error, error_truncated) = bounded_redacted_probe_text(&err.to_string());
    let (hint, hint_truncated) = bounded_redacted_probe_text(err.hint());
    json!({
        "surface": surface,
        "proof_state": "blocked",
        "block_class": block_class,
        "error": error,
        "error_truncated": error_truncated,
        "hint": hint,
        "hint_truncated": hint_truncated,
    })
}

fn probe_error_block_class(err: &AdManagerError) -> &'static str {
    match err {
        AdManagerError::AuthBootstrap(_)
        | AdManagerError::UpstreamApi {
            status: 401 | 403, ..
        }
        | AdManagerError::WriteScopeRequired { .. } => "permission",
        _ => "upstream",
    }
}

async fn probe_yield_groups(
    server: &AdManagerServer,
    network_code: &str,
    api_version: Option<&str>,
    page_size: u32,
    target_ad_units: &[ProbeAdUnitTarget],
    include_raw: bool,
) -> Result<Value, AdManagerError> {
    if target_ad_units.is_empty() {
        return Ok(json!({
            "surface": "yield_groups",
            "decision": "skipped",
            "proof_state": "skipped",
            "reason": "no target ad-unit ids were available from exact ad-unit reads",
            "mutation_performed": false,
        }));
    }
    if !scope_allows_write(server.client().scope()) {
        let (current_scope, current_scope_truncated) =
            bounded_redacted_probe_text(server.client().scope());
        return Ok(json!({
            "surface": "yield_groups",
            "decision": "blocked",
            "proof_state": "blocked",
            "block_class": "permission",
            "reason": "YieldGroupService SOAP reads require the Google Ad Manager manage scope",
            "required_scope": MANAGE_SCOPE,
            "current_scope": current_scope,
            "current_scope_truncated": current_scope_truncated,
            "mutation_performed": false,
        }));
    }
    let payload_xml = pql_statement_payload(&format!("LIMIT {}", page_size.min(1_000)));
    let plan = server.client().build_soap_trafficking_plan(
        network_code,
        api_version,
        SoapTraffickingOperation::GetYieldGroupsByStatement,
        &payload_xml,
    )?;
    let applied = server.client().execute_soap_trafficking_plan(&plan).await?;
    if applied.upstream_status >= 400 || applied.soap_fault.is_some() {
        return Ok(blocked_yield_group_response(applied));
    }

    Ok(summarize_yield_groups(
        &applied.upstream_response_xml,
        applied.response_truncated,
        applied.request_id,
        applied.response_time,
        target_ad_units,
        include_raw,
    ))
}

fn blocked_yield_group_response(applied: SoapTraffickingApplyResult) -> Value {
    let block_class = soap_probe_block_class(
        applied.upstream_status,
        applied.soap_fault.as_deref(),
        &applied.upstream_response_xml,
    );
    let (message_source, message_source_truncated) =
        soap_error_message_with_truncation(&applied);
    let (message, message_projection_truncated) = bounded_redacted_probe_text(&message_source);
    let message_truncated = message_source_truncated || message_projection_truncated;
    let (request_id, request_id_truncated) =
        bounded_redacted_probe_text_option(applied.request_id);
    let (response_time, response_time_truncated) =
        bounded_redacted_probe_text_option(applied.response_time);
    let (soap_fault, soap_fault_truncated) =
        bounded_redacted_probe_text_option(applied.soap_fault);
    json!({
        "surface": "yield_groups",
        "decision": "blocked",
        "proof_state": "blocked",
        "block_class": block_class,
        "upstream_status": applied.upstream_status,
        "request_id": request_id,
        "request_id_truncated": request_id_truncated,
        "response_time": response_time,
        "response_time_truncated": response_time_truncated,
        "soap_fault": soap_fault,
        "soap_fault_truncated": soap_fault_truncated,
        "message": message,
        "message_truncated": message_truncated,
        "mutation_performed": false,
    })
}

fn summarize_yield_groups(
    xml: &str,
    response_truncated: bool,
    request_id: Option<String>,
    response_time: Option<String>,
    target_ad_units: &[ProbeAdUnitTarget],
    include_raw: bool,
) -> Value {
    let target_ad_unit_ids = target_ad_units
        .iter()
        .map(|target| target.ad_unit_id.clone())
        .collect::<Vec<_>>();
    let results = extract_xml_blocks(xml, "results");
    let total_result_set_size =
        extract_xml_tag_text(xml, "totalResultSetSize").and_then(|value| value.parse::<u64>().ok());
    let mut matches = Vec::new();
    let mut targeted_exposed = Vec::new();
    let mut targeted_and_excluded = Vec::new();
    let mut targeted_inactive = Vec::new();
    let mut targeted_activity_unknown = Vec::new();
    for result in &results {
        let yield_group_id = yield_group_id_from_xml(result);
        let yield_group_name = yield_group_name(result);
        let status = extract_xml_tag_text(result, "exchangeStatus")
            .or_else(|| extract_xml_tag_text(result, "status"));
        let activity_state = yield_group_activity_state(status.as_deref());
        let format = extract_xml_tag_text(result, "format");
        let environment_type = extract_xml_tag_text(result, "environmentType");
        let targeting_block = extract_xml_first_block(result, "targeting").unwrap_or_default();
        let inventory_block = extract_xml_first_block(&targeting_block, "inventoryTargeting")
            .unwrap_or_else(|| "<inventoryTargeting />".to_string());
        let targeted_ad_units = ad_unit_targeting_values(&inventory_block, "targetedAdUnits");
        let excluded_ad_units = ad_unit_targeting_values(&inventory_block, "excludedAdUnits");
        let mut matched_ids = Vec::new();
        let mut result_targeted_exposed = Vec::new();
        let mut result_targeted_and_excluded = Vec::new();
        let mut result_targeted_inactive = Vec::new();
        let mut result_targeted_activity_unknown = Vec::new();

        for target in target_ad_units {
            let direct_or_ancestor_targeting_match =
                targeting_coverage_for_target(&targeted_ad_units, target);
            let exclusion_match = targeting_coverage_for_target(&excluded_ad_units, target);
            let broad_targeting_match =
                if direct_or_ancestor_targeting_match.is_none() && exclusion_match.is_some() {
                    broad_descendant_targeting_context(&targeted_ad_units)
                } else {
                    None
                };
            let targeting_match = direct_or_ancestor_targeting_match.or(broad_targeting_match);
            let Some(classification) = yield_group_match_classification(
                targeting_match.as_ref(),
                exclusion_match.as_ref(),
                activity_state,
            ) else {
                continue;
            };

            let match_entry = json!({
                "yield_group_id": yield_group_id.clone(),
                "yield_group_name": yield_group_name.clone(),
                "status": status.clone(),
                "activity_state": activity_state,
                "format": format.clone(),
                "environment_type": environment_type.clone(),
                "requested_ad_unit_id": target.ad_unit_id.clone(),
                "classification": classification,
                "targeting_match": targeting_match.as_ref().map(targeting_coverage_json).unwrap_or(Value::Null),
                "exclusion_match": exclusion_match.as_ref().map(targeting_coverage_json).unwrap_or(Value::Null),
            });

            matched_ids.push(target.ad_unit_id.clone());
            match classification {
                "targeted_exposed" => {
                    result_targeted_exposed.push(target.ad_unit_id.clone());
                    targeted_exposed.push(match_entry);
                }
                "targeted_and_excluded" => {
                    result_targeted_and_excluded.push(target.ad_unit_id.clone());
                    targeted_and_excluded.push(match_entry);
                }
                "targeted_inactive" => {
                    result_targeted_inactive.push(target.ad_unit_id.clone());
                    targeted_inactive.push(match_entry);
                }
                "targeted_activity_unknown" => {
                    result_targeted_activity_unknown.push(target.ad_unit_id.clone());
                    targeted_activity_unknown.push(match_entry);
                }
                _ => {}
            }
        }

        if matched_ids.is_empty() {
            continue;
        }
        matches.push(json!({
            "yield_group_id": yield_group_id,
            "yield_group_name": yield_group_name,
            "status": status,
            "activity_state": activity_state,
            "format": format,
            "environment_type": environment_type,
            "matched_ad_unit_ids": matched_ids,
            "targeted_exposed_ad_unit_ids": result_targeted_exposed,
            "targeted_and_excluded_ad_unit_ids": result_targeted_and_excluded,
            "targeted_inactive_ad_unit_ids": result_targeted_inactive,
            "targeted_activity_unknown_ad_unit_ids": result_targeted_activity_unknown,
            "targeted_ad_units": targeted_ad_units,
            "excluded_ad_units": excluded_ad_units,
        }));
    }
    let sample_only = response_truncated
        || total_result_set_size
            .map(|total| total > results.len() as u64)
            .unwrap_or(false);
    let decision = if !targeted_exposed.is_empty() {
        "targeted_exposed"
    } else if !targeted_and_excluded.is_empty() {
        "targeted_and_excluded"
    } else if !targeted_activity_unknown.is_empty() {
        "targeted_activity_unknown"
    } else if !targeted_inactive.is_empty() {
        "targeted_inactive"
    } else if sample_only {
        "sample_only"
    } else {
        "no_target_matches"
    };
    let proof_state = if sample_only {
        "sample_only"
    } else {
        "complete"
    };
    let (request_id, request_id_truncated) = bounded_redacted_probe_text_option(request_id);
    let (response_time, response_time_truncated) =
        bounded_redacted_probe_text_option(response_time);
    let mut response = json!({
        "surface": "yield_groups",
        "decision": decision,
        "proof_state": proof_state,
        "request_id": request_id,
        "request_id_truncated": request_id_truncated,
        "response_time": response_time,
        "response_time_truncated": response_time_truncated,
        "total_result_set_size": total_result_set_size,
        "inspected_results": results.len(),
        "response_truncated": response_truncated,
        "target_ad_unit_ids": target_ad_unit_ids,
        "target_ad_unit_matches": matches,
        "targeted_exposed": targeted_exposed,
        "targeted_and_excluded": targeted_and_excluded,
        "targeted_inactive": targeted_inactive,
        "targeted_activity_unknown": targeted_activity_unknown,
        "mutation_performed": false,
    });
    if include_raw && let Some(object) = response.as_object_mut() {
        object.insert(
            "upstream_response_xml".to_string(),
            Value::String(xml.to_string()),
        );
    }
    response
}

struct LineItemDependencyProbeOptions<'a> {
    network_code: &'a str,
    api_version: Option<&'a str>,
    line_item_page_size: u32,
    max_line_items: u32,
    include_line_item_xml: bool,
}

#[derive(Default)]
struct LineItemDependencyScanState {
    offset: u32,
    inspected_results: u32,
    total_result_set_size: Option<u64>,
    response_truncated: bool,
    request_ids: Vec<String>,
    request_id_count: usize,
    request_ids_truncated: bool,
    response_times: Vec<String>,
    response_time_count: usize,
    response_times_truncated: bool,
    dependency_matches_sample: Vec<Value>,
    dependency_match_count: usize,
    dependency_matches_truncated: bool,
    status_counts: BTreeMap<String, u64>,
    missing_total_result_set_size: bool,
}

struct SuccessfulLineItemPage<'a> {
    upstream_response_xml: &'a str,
    response_truncated: bool,
    request_id: Option<String>,
    response_time: Option<String>,
}

enum LineItemProbeBlock {
    Error {
        block_class: &'static str,
        error: String,
        error_truncated: bool,
        hint: String,
        hint_truncated: bool,
    },
    Soap {
        block_class: &'static str,
        upstream_status: u16,
        request_id: Option<String>,
        request_id_truncated: bool,
        response_time: Option<String>,
        response_time_truncated: bool,
        soap_fault: Option<String>,
        soap_fault_truncated: bool,
        message: String,
        message_truncated: bool,
    },
}

impl LineItemProbeBlock {
    fn from_error(err: &AdManagerError) -> Self {
        let (error, error_truncated) = bounded_redacted_probe_text(&err.to_string());
        let (hint, hint_truncated) = bounded_redacted_probe_text(err.hint());
        Self::Error {
            block_class: probe_error_block_class(err),
            error,
            error_truncated,
            hint,
            hint_truncated,
        }
    }

    fn insert_into(self, response: &mut Map<String, Value>) {
        match self {
            Self::Error {
                block_class,
                error,
                error_truncated,
                hint,
                hint_truncated,
            } => {
                response.insert("block_class".to_string(), json!(block_class));
                response.insert("error".to_string(), json!(error));
                response.insert("error_truncated".to_string(), json!(error_truncated));
                response.insert("hint".to_string(), json!(hint));
                response.insert("hint_truncated".to_string(), json!(hint_truncated));
            }
            Self::Soap {
                block_class,
                upstream_status,
                request_id,
                request_id_truncated,
                response_time,
                response_time_truncated,
                soap_fault,
                soap_fault_truncated,
                message,
                message_truncated,
            } => {
                response.insert("block_class".to_string(), json!(block_class));
                response.insert("upstream_status".to_string(), json!(upstream_status));
                response.insert("request_id".to_string(), json!(request_id));
                response.insert(
                    "request_id_truncated".to_string(),
                    json!(request_id_truncated),
                );
                response.insert("response_time".to_string(), json!(response_time));
                response.insert(
                    "response_time_truncated".to_string(),
                    json!(response_time_truncated),
                );
                response.insert("soap_fault".to_string(), json!(soap_fault));
                response.insert(
                    "soap_fault_truncated".to_string(),
                    json!(soap_fault_truncated),
                );
                response.insert("message".to_string(), json!(message));
                response.insert("message_truncated".to_string(), json!(message_truncated));
            }
        }
    }
}

impl LineItemDependencyScanState {
    fn record_successful_page(
        &mut self,
        page: SuccessfulLineItemPage<'_>,
        targets: &[DependencyProbeTarget],
        placement_summary: &Value,
        include_line_item_xml: bool,
    ) -> u32 {
        if let Some(request_id) = page.request_id {
            self.request_id_count = self.request_id_count.saturating_add(1);
            let (request_id, value_truncated) = bounded_redacted_probe_text(&request_id);
            self.request_ids_truncated |= value_truncated
                || self.request_ids.len() >= PROBE_TRANSPORT_METADATA_SAMPLE_LIMIT;
            if self.request_ids.len() < PROBE_TRANSPORT_METADATA_SAMPLE_LIMIT {
                self.request_ids.push(request_id);
            }
        }
        if let Some(response_time) = page.response_time {
            self.response_time_count = self.response_time_count.saturating_add(1);
            let (response_time, value_truncated) = bounded_redacted_probe_text(&response_time);
            self.response_times_truncated |= value_truncated
                || self.response_times.len() >= PROBE_TRANSPORT_METADATA_SAMPLE_LIMIT;
            if self.response_times.len() < PROBE_TRANSPORT_METADATA_SAMPLE_LIMIT {
                self.response_times.push(response_time);
            }
        }
        self.response_truncated |= page.response_truncated;
        let page_total = extract_xml_tag_text(page.upstream_response_xml, "totalResultSetSize")
            .and_then(|value| value.parse::<u64>().ok());
        if self.total_result_set_size.is_none() {
            self.total_result_set_size = page_total;
        }
        if page_total.is_none() {
            self.missing_total_result_set_size = true;
        }

        let results = extract_xml_blocks(page.upstream_response_xml, "results");
        for result in &results {
            let status = extract_xml_tag_text(result, "status").unwrap_or_else(|| "UNKNOWN".into());
            *self.status_counts.entry(status).or_insert(0) += 1;
            if let Some(entry) = line_item_dependency_entry(
                result,
                targets,
                placement_summary,
                include_line_item_xml,
            ) {
                self.dependency_match_count += 1;
                if self.dependency_matches_sample.len() < DEPENDENCY_LINE_ITEM_MATCH_SAMPLE_LIMIT {
                    self.dependency_matches_sample.push(entry);
                } else {
                    self.dependency_matches_truncated = true;
                }
            }
        }

        let result_count = results.len() as u32;
        self.inspected_results = self.inspected_results.saturating_add(result_count);
        self.offset = self.offset.saturating_add(result_count);
        result_count
    }

    fn into_response(
        self,
        options: &LineItemDependencyProbeOptions<'_>,
        decision: &'static str,
        proof_state: &'static str,
    ) -> Value {
        json!({
            "surface": "line_items",
            "decision": decision,
            "proof_state": proof_state,
            "total_result_set_size": self.total_result_set_size,
            "inspected_results": self.inspected_results,
            "max_line_items": options.max_line_items,
            "line_item_page_size": options.line_item_page_size,
            "response_truncated": self.response_truncated,
            "missing_total_result_set_size": self.missing_total_result_set_size,
            "request_ids": self.request_ids,
            "request_id_count": self.request_id_count,
            "request_ids_truncated": self.request_ids_truncated,
            "response_times": self.response_times,
            "response_time_count": self.response_time_count,
            "response_times_truncated": self.response_times_truncated,
            "transport_metadata_sample_limit": PROBE_TRANSPORT_METADATA_SAMPLE_LIMIT,
            "status_counts": self.status_counts,
            "dependency_match_count": self.dependency_match_count,
            "dependency_matches_sample": self.dependency_matches_sample,
            "dependency_matches_truncated": self.dependency_matches_truncated,
            "dependency_match_sample_limit": DEPENDENCY_LINE_ITEM_MATCH_SAMPLE_LIMIT,
            "mutation_performed": false,
        })
    }

    fn into_blocked_response(
        self,
        options: &LineItemDependencyProbeOptions<'_>,
        block: LineItemProbeBlock,
    ) -> Value {
        let decision = if self.dependency_match_count > 0 {
            "dependencies_found"
        } else {
            "blocked"
        };
        let mut response = self.into_response(options, decision, "blocked");
        block.insert_into(
            response
                .as_object_mut()
                .expect("line-item scan response is an object"),
        );
        response
    }

    fn into_error_blocked_response(
        self,
        options: &LineItemDependencyProbeOptions<'_>,
        err: &AdManagerError,
    ) -> Value {
        self.into_blocked_response(options, LineItemProbeBlock::from_error(err))
    }

    fn into_soap_blocked_response(
        mut self,
        options: &LineItemDependencyProbeOptions<'_>,
        applied: SoapTraffickingApplyResult,
    ) -> Value {
        self.response_truncated |= applied.response_truncated;
        let block_class = soap_probe_block_class(
            applied.upstream_status,
            applied.soap_fault.as_deref(),
            &applied.upstream_response_xml,
        );
        let (message_source, message_source_truncated) =
            soap_error_message_with_truncation(&applied);
        let (message, message_projection_truncated) =
            bounded_redacted_probe_text(&message_source);
        let message_truncated = message_source_truncated || message_projection_truncated;
        let (request_id, request_id_truncated) =
            bounded_redacted_probe_text_option(applied.request_id);
        let (response_time, response_time_truncated) =
            bounded_redacted_probe_text_option(applied.response_time);
        let (soap_fault, soap_fault_truncated) =
            bounded_redacted_probe_text_option(applied.soap_fault);
        self.into_blocked_response(
            options,
            LineItemProbeBlock::Soap {
                block_class,
                upstream_status: applied.upstream_status,
                request_id,
                request_id_truncated,
                response_time,
                response_time_truncated,
                soap_fault,
                soap_fault_truncated,
                message,
                message_truncated,
            },
        )
    }

    fn into_completed_response(self, options: &LineItemDependencyProbeOptions<'_>) -> Value {
        let capped = self.response_truncated
            || self.missing_total_result_set_size
            || self
                .total_result_set_size
                .map(|total| total > u64::from(self.inspected_results))
                .unwrap_or(self.inspected_results >= options.max_line_items);
        let proof_state = if capped { "sample_only" } else { "complete" };
        let decision = if self.dependency_match_count > 0 {
            "dependencies_found"
        } else if capped {
            "no_dependencies_in_sample"
        } else {
            "no_dependencies_observed"
        };
        self.into_response(options, decision, proof_state)
    }
}

async fn scan_ad_unit_line_item_dependencies<F, Fut>(
    options: &LineItemDependencyProbeOptions<'_>,
    targets: &[DependencyProbeTarget],
    placement_summary: &Value,
    mut fetch_page: F,
) -> Value
where
    F: FnMut(u32, u32) -> Fut,
    Fut: Future<Output = Result<SoapTraffickingApplyResult, AdManagerError>>,
{
    let mut state = LineItemDependencyScanState::default();

    while state.inspected_results < options.max_line_items {
        let remaining = options
            .max_line_items
            .saturating_sub(state.inspected_results);
        let page_limit = options.line_item_page_size.min(remaining).max(1);
        let applied = match fetch_page(state.offset, page_limit).await {
            Ok(applied) => applied,
            Err(err) => return state.into_error_blocked_response(options, &err),
        };
        if applied.upstream_status >= 400 || applied.soap_fault.is_some() {
            return state.into_soap_blocked_response(options, applied);
        }

        let result_count = state.record_successful_page(
            SuccessfulLineItemPage {
                upstream_response_xml: &applied.upstream_response_xml,
                response_truncated: applied.response_truncated,
                request_id: applied.request_id,
                response_time: applied.response_time,
            },
            targets,
            placement_summary,
            options.include_line_item_xml,
        );
        if result_count == 0 {
            break;
        }
        if state
            .total_result_set_size
            .map(|total| u64::from(state.offset) >= total)
            .unwrap_or(false)
        {
            break;
        }
    }

    state.into_completed_response(options)
}

async fn probe_ad_unit_line_item_dependencies(
    server: &AdManagerServer,
    options: LineItemDependencyProbeOptions<'_>,
    targets: &[DependencyProbeTarget],
    placement_summary: &Value,
) -> Result<Value, AdManagerError> {
    if targets.is_empty() {
        return Ok(json!({
            "surface": "line_items",
            "decision": "skipped",
            "proof_state": "skipped",
            "reason": "no resolved ad-unit ids were available",
            "mutation_performed": false,
        }));
    }
    if !scope_allows_write(server.client().scope()) {
        let (current_scope, current_scope_truncated) =
            bounded_redacted_probe_text(server.client().scope());
        return Ok(json!({
            "surface": "line_items",
            "decision": "blocked",
            "proof_state": "blocked",
            "block_class": "permission",
            "reason": "LineItemService SOAP reads require the Google Ad Manager manage scope",
            "required_scope": MANAGE_SCOPE,
            "current_scope": current_scope,
            "current_scope_truncated": current_scope_truncated,
            "mutation_performed": false,
        }));
    }

    let client = server.client();
    let network_code = options.network_code;
    let api_version = options.api_version;
    Ok(scan_ad_unit_line_item_dependencies(
        &options,
        targets,
        placement_summary,
        |offset, page_limit| async move {
            let query = format!("ORDER BY id ASC LIMIT {page_limit} OFFSET {offset}");
            let payload_xml = pql_payload(&query);
            let plan = client.build_soap_trafficking_plan(
                network_code,
                api_version,
                SoapTraffickingOperation::GetLineItemsByStatement,
                &payload_xml,
            )?;
            client.execute_soap_trafficking_plan(&plan).await
        },
    )
    .await)
}

fn line_item_dependency_entry(
    result: &str,
    targets: &[DependencyProbeTarget],
    placement_summary: &Value,
    include_line_item_xml: bool,
) -> Option<Value> {
    let targeting_block = extract_xml_first_block(result, "targeting").unwrap_or_default();
    let inventory_block = extract_xml_first_block(&targeting_block, "inventoryTargeting")
        .unwrap_or_else(|| "<inventoryTargeting />".to_string());
    let targeted_ad_units = ad_unit_targeting_values(&inventory_block, "targetedAdUnits");
    let excluded_ad_units = ad_unit_targeting_values(&inventory_block, "excludedAdUnits");
    let targeted_placement_ids = extract_xml_tag_texts(&inventory_block, "targetedPlacementIds")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let root_or_network_targeting =
        targeted_ad_units.is_empty() && targeted_placement_ids.is_empty();

    let mut target_matches = Vec::new();
    for target in targets {
        let probe_target = target.to_probe_target();
        let targeting_match = targeting_coverage_for_target(&targeted_ad_units, &probe_target);
        let exclusion_match = targeting_coverage_for_target(&excluded_ad_units, &probe_target);
        let target_placement_ids =
            dependency_placement_ids_for_target(placement_summary, &target.ad_unit_id);
        let matched_placement_ids = target_placement_ids
            .iter()
            .filter(|placement_id| targeted_placement_ids.contains(*placement_id))
            .cloned()
            .collect::<Vec<_>>();
        if targeting_match.is_none()
            && matched_placement_ids.is_empty()
            && !root_or_network_targeting
        {
            continue;
        }
        let classification = line_item_dependency_classification(
            targeting_match.as_ref(),
            !matched_placement_ids.is_empty(),
            root_or_network_targeting,
            exclusion_match.as_ref(),
        );
        target_matches.push(json!({
            "ad_unit_id": target.ad_unit_id,
            "ad_unit_codes": target.ad_unit_codes.iter().cloned().collect::<Vec<_>>(),
            "classification": classification,
            "targeting_match": targeting_match.as_ref().map(targeting_coverage_json).unwrap_or(Value::Null),
            "exclusion_match": exclusion_match.as_ref().map(targeting_coverage_json).unwrap_or(Value::Null),
            "matched_placement_ids": matched_placement_ids,
            "root_or_network_targeting": root_or_network_targeting,
            "dependency_excluded": exclusion_match.is_some(),
        }));
    }
    if target_matches.is_empty() {
        return None;
    }

    let is_archived = extract_xml_tag_text(result, "isArchived")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let status = extract_xml_tag_text(result, "status");
    let activity_state = line_item_activity_state(status.as_deref(), is_archived);
    let primary_goal = extract_xml_first_block(result, "primaryGoal").unwrap_or_default();
    let stats = extract_xml_first_block(result, "stats").unwrap_or_default();
    let creative_placeholders = extract_xml_blocks(result, "creativePlaceholders");
    let custom_targeting_key_ids = unique_xml_texts(result, "keyId");
    let custom_targeting_value_ids = unique_xml_texts(result, "valueIds");
    let mut entry = json!({
        "line_item_id": extract_xml_tag_text(result, "id"),
        "line_item_name": extract_xml_tag_text(result, "name"),
        "order_id": extract_xml_tag_text(result, "orderId"),
        "order_name": extract_xml_tag_text(result, "orderName"),
        "status": status,
        "reservation_status": extract_xml_tag_text(result, "reservationStatus"),
        "activity_state": activity_state,
        "is_archived": is_archived,
        "line_item_type": extract_xml_tag_text(result, "lineItemType"),
        "priority": extract_xml_tag_text(result, "priority"),
        "start_date_time": extract_xml_date_time(result, "startDateTime"),
        "end_date_time": extract_xml_date_time(result, "endDateTime"),
        "creative_sizes": creative_sizes_from_placeholders(&creative_placeholders),
        "primary_goal_type": extract_xml_tag_text(&primary_goal, "goalType"),
        "primary_goal_unit_type": extract_xml_tag_text(&primary_goal, "unitType"),
        "primary_goal_units": extract_xml_tag_text(&primary_goal, "units"),
        "impressions_delivered": extract_xml_tag_text(&stats, "impressionsDelivered"),
        "clicks_delivered": extract_xml_tag_text(&stats, "clicksDelivered"),
        "targeted_ad_units": targeted_ad_units,
        "excluded_ad_units": excluded_ad_units,
        "targeted_placement_ids": targeted_placement_ids.into_iter().collect::<Vec<_>>(),
        "custom_targeting_key_ids": custom_targeting_key_ids,
        "custom_targeting_value_ids": custom_targeting_value_ids,
        "target_matches": target_matches,
    });
    if include_line_item_xml && let Some(object) = entry.as_object_mut() {
        let (xml_sample, xml_truncated) =
            bounded_text_sample(result, DEPENDENCY_LINE_ITEM_XML_SAMPLE_BYTES);
        object.insert("upstream_xml_sample".to_string(), Value::String(xml_sample));
        object.insert(
            "upstream_xml_truncated".to_string(),
            Value::Bool(xml_truncated),
        );
        object.insert("upstream_xml_bytes".to_string(), json!(result.len()));
    }
    Some(entry)
}

fn bounded_text_sample(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (value[..end].to_string(), true)
}

fn bounded_redacted_probe_text(value: &str) -> (String, bool) {
    let redacted = contract::redact_secret_text(value);
    bounded_text_sample(&redacted, PROBE_DIAGNOSTIC_SAMPLE_BYTES)
}

fn bounded_redacted_probe_text_option(value: Option<String>) -> (Option<String>, bool) {
    match value {
        Some(value) => {
            let (value, truncated) = bounded_redacted_probe_text(&value);
            (Some(value), truncated)
        }
        None => (None, false),
    }
}

fn unique_xml_texts(value: &str, tag: &str) -> Vec<String> {
    extract_xml_tag_texts(value, tag)
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn dependency_placement_ids_for_target(
    placement_summary: &Value,
    ad_unit_id: &str,
) -> BTreeSet<String> {
    placement_summary
        .get("target_placement_ids_by_ad_unit_id")
        .and_then(Value::as_object)
        .and_then(|map| map.get(ad_unit_id))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn line_item_dependency_classification(
    targeting_match: Option<&TargetingCoverage>,
    placement_match: bool,
    root_or_network_targeting: bool,
    exclusion_match: Option<&TargetingCoverage>,
) -> &'static str {
    if exclusion_match.is_some() {
        if targeting_match.is_some() {
            return "targeted_but_excluded";
        }
        if placement_match {
            return "placement_targeted_but_excluded";
        }
        if root_or_network_targeting {
            return "root_or_network_targeted_but_excluded";
        }
        return "excluded_without_target_match";
    }
    if let Some(match_value) = targeting_match {
        return match match_value.match_type {
            "exact" => "exact_target",
            "ancestor_descendant" => "ancestor_descendant_target",
            other => other,
        };
    }
    if placement_match {
        return "placement_target";
    }
    if root_or_network_targeting {
        return "root_or_network_target";
    }
    "unclassified"
}

fn line_item_activity_state(status: Option<&str>, is_archived: bool) -> &'static str {
    if is_archived {
        return "archived";
    }
    match status.map(|value| value.trim().to_ascii_uppercase()) {
        Some(value) if value == "DELIVERING" => "delivering",
        Some(value)
            if matches!(
                value.as_str(),
                "READY" | "RESERVING" | "PENDING_APPROVAL" | "PENDING_INVENTORY_RELEASE"
            ) =>
        {
            "future_or_ready"
        }
        Some(value) if value == "PAUSED" => "paused_or_resumable",
        Some(value)
            if matches!(
                value.as_str(),
                "COMPLETED" | "INACTIVE" | "ARCHIVED" | "DRAFT" | "DISAPPROVED" | "CANCELED"
            ) =>
        {
            "inactive"
        }
        _ => "unknown",
    }
}

fn dependency_probe_decision(
    target_resolution_issues: &[String],
    placement_summary: &Value,
    line_item_summary: &Value,
) -> &'static str {
    let placement_dependencies = placement_summary
        .get("target_placement_match_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0;
    let line_item_dependencies = line_item_summary
        .get("dependency_match_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0;
    if placement_dependencies || line_item_dependencies {
        "dependencies_found"
    } else if line_item_summary
        .get("proof_state")
        .and_then(Value::as_str)
        .is_some_and(|state| state == "blocked")
        || placement_summary
            .get("proof_state")
            .and_then(Value::as_str)
            .is_some_and(|state| state == "blocked")
    {
        "blocked"
    } else if !target_resolution_issues.is_empty() {
        "missing_or_ambiguous_targets"
    } else if dependency_proof_incomplete(placement_summary, line_item_summary) {
        "incomplete_no_dependencies_observed"
    } else {
        "no_dependencies_observed"
    }
}

fn dependency_probe_response_json(
    network_code: &str,
    target_rows: Vec<Value>,
    placement_summary: Value,
    line_item_summary: Value,
    target_resolution_issues: Vec<String>,
    proof_flags: Value,
    dependency_decision: &str,
) -> Value {
    json!({
        "network_code": network_code,
        "dependency_decision": dependency_decision,
        "ad_units": target_rows,
        "placements": placement_summary,
        "line_items": line_item_summary,
        "target_resolution_issues": target_resolution_issues,
        "proof_flags": proof_flags,
        "mutation_performed": false,
        "cleanup_decision": {
            "safe_to_archive_or_retire": false,
            "reason": "This read-only helper reports dependencies and proof gaps; archive, deactivate, or retarget decisions require a separate reviewed workflow."
        }
    })
}

fn finalize_exchange_probe_response(
    mut response: Value,
    network_code: &str,
) -> Result<Value, AdManagerError> {
    attach_result_fingerprint(&mut response);
    let state = exchange_evidence_state(&response);
    attach_evidence_receipt_template(
        &mut response,
        network_code,
        EvidenceSource::ExchangeProtectionReview,
        state,
    )?;
    Ok(response)
}

fn finalize_dependency_probe_response(
    mut response: Value,
    network_code: &str,
    dependency_decision: &str,
) -> Result<Value, AdManagerError> {
    attach_result_fingerprint(&mut response);
    let state = dependency_evidence_state(dependency_decision, &response);
    attach_evidence_receipt_template(
        &mut response,
        network_code,
        EvidenceSource::DependencyProbe,
        state,
    )?;
    Ok(response)
}

fn attach_result_fingerprint(response: &mut Value) {
    let fingerprint = stable_fingerprint(&response.to_string());
    response
        .as_object_mut()
        .expect("probe response is an object")
        .insert("result_fingerprint".to_string(), Value::String(fingerprint));
}

fn attach_evidence_receipt_template(
    response: &mut Value,
    network_code: &str,
    source: EvidenceSource,
    state: EvidenceState,
) -> Result<(), AdManagerError> {
    let template = match evidence_receipt_target_ids(response, network_code) {
        Some(target_ids) => {
            let result_fingerprint = response
                .get("result_fingerprint")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AdManagerError::invalid(
                        "result_hash",
                        "probe response did not contain its producer fingerprint",
                    )
                })?;
            evidence_receipt_template(network_code, source, state, result_fingerprint, target_ids)?
        }
        None => {
            json!({
                "source": source.as_str(),
                "source_version": crate::evidence::EVIDENCE_PRODUCER_CONTRACT_VERSION,
                "state": "not_generated",
                "reason": "evidence receipts require a canonical network, result fingerprint, and one to ten fully resolved exact ad-unit ids"
            })
        }
    };
    response
        .as_object_mut()
        .expect("probe response is an object")
        .insert("evidence_receipt_template".to_string(), template);
    Ok(())
}

fn evidence_receipt_target_ids(response: &Value, network_code: &str) -> Option<Vec<String>> {
    if response
        .get("target_resolution_issues")
        .and_then(Value::as_array)
        .is_some_and(|issues| !issues.is_empty())
    {
        return None;
    }
    let rows = response.get("ad_units")?.as_array()?;
    if rows.is_empty()
        || rows.len() > 10
        || rows.iter().any(|row| !exact_target_row(row, network_code))
    {
        return None;
    }
    let target_ids = rows
        .iter()
        .filter_map(|row| row.get("ad_unit_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    (target_ids.len() == rows.len()).then_some(target_ids)
}

fn exact_target_row(row: &Value, network_code: &str) -> bool {
    let Some(ad_unit_id) = row.get("ad_unit_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(resource_name) = row.get("resource_name").and_then(Value::as_str) else {
        return false;
    };
    row.get("proof_state").and_then(Value::as_str) == Some("resolved_exact")
        && exact_ad_unit_id_from_resource_name(network_code, resource_name).as_deref()
            == Some(ad_unit_id)
}

fn exact_ad_unit_id_from_resource_name(network_code: &str, resource_name: &str) -> Option<String> {
    exact_resource_id_from_name(network_code, "adUnits", resource_name)
}

fn exact_resource_id_from_name(
    network_code: &str,
    resource_collection: &str,
    resource_name: &str,
) -> Option<String> {
    let prefix = format!("networks/{network_code}/{resource_collection}/");
    let raw_id = resource_name.strip_prefix(&prefix)?;
    if raw_id.is_empty() || raw_id.contains('/') {
        return None;
    }
    let canonical_id = parse_positive_id_string("ad_unit_id", raw_id).ok()?;
    (canonical_id == raw_id).then_some(canonical_id)
}

fn dependency_evidence_state(decision: &str, response: &Value) -> EvidenceState {
    match decision {
        "dependencies_found" => EvidenceState::CompleteBlocked,
        "no_dependencies_observed" => EvidenceState::CompleteClear,
        "incomplete_no_dependencies_observed" | "missing_or_ambiguous_targets" => {
            EvidenceState::PartialCapped
        }
        "blocked" => blocked_evidence_state(response),
        _ => EvidenceState::NotRun,
    }
}

fn exchange_evidence_state(response: &Value) -> EvidenceState {
    let target_exposed = response
        .get("ad_units")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|row| row.get("decision").and_then(Value::as_str) == Some("attention_required"))
        || response
            .get("yield_groups")
            .and_then(|value| value.get("decision"))
            .and_then(Value::as_str)
            == Some("targeted_exposed");
    if target_exposed {
        return EvidenceState::CompleteBlocked;
    }

    let blocked = [
        "private_auctions",
        "private_auction_deals",
        "yield_groups",
        "rest_discovery",
    ]
    .into_iter()
    .any(|surface| {
        response
            .get(surface)
            .and_then(|value| value.get("proof_state"))
            .and_then(Value::as_str)
            == Some("blocked")
    });
    if blocked {
        return blocked_evidence_state(response);
    }

    let yield_group_activity_unknown = response.get("yield_groups").is_some_and(|yield_groups| {
        yield_groups.get("decision").and_then(Value::as_str) == Some("targeted_activity_unknown")
            || yield_groups
                .get("targeted_activity_unknown")
                .and_then(Value::as_array)
                .is_some_and(|matches| !matches.is_empty())
    });
    if yield_group_activity_unknown {
        return EvidenceState::PartialCapped;
    }

    let api_complete = response
        .get("certainty")
        .and_then(Value::as_object)
        .is_some_and(|certainty| {
            [
                "can_prove_requested_ad_unit_flags",
                "can_prove_private_auction_absence_or_presence",
                "can_prove_private_deal_absence_or_presence",
                "can_prove_yield_group_targeting",
            ]
            .into_iter()
            .all(|field| certainty.get(field).and_then(Value::as_bool) == Some(true))
        });
    if api_complete {
        EvidenceState::ManualUiProofRequired
    } else {
        EvidenceState::PartialCapped
    }
}

fn blocked_evidence_state(response: &Value) -> EvidenceState {
    if contains_permission_block(response) {
        EvidenceState::BlockedPermission
    } else {
        EvidenceState::BlockedRead
    }
}

fn contains_permission_block(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            (object.get("proof_state").and_then(Value::as_str) == Some("blocked")
                && object.get("block_class").and_then(Value::as_str) == Some("permission"))
                || object.values().any(contains_permission_block)
        }
        Value::Array(values) => values.iter().any(contains_permission_block),
        _ => false,
    }
}

fn soap_probe_block_class(
    status: u16,
    soap_fault: Option<&str>,
    upstream_response_xml: &str,
) -> &'static str {
    let permission_fault = [soap_fault.unwrap_or_default(), upstream_response_xml]
        .into_iter()
        .any(|value| {
            value.contains("PermissionError.") || authentication_fault_is_permission(value)
        });
    if matches!(status, 401 | 403) || permission_fault {
        "permission"
    } else {
        "upstream"
    }
}

fn authentication_fault_is_permission(value: &str) -> bool {
    const PERMISSION_REASONS: &[&str] = &[
        "AuthenticationError.AMBIGUOUS_SOAP_REQUEST_HEADER",
        "AuthenticationError.INVALID_EMAIL",
        "AuthenticationError.AUTHENTICATION_FAILED",
        "AuthenticationError.INVALID_OAUTH_SIGNATURE",
        "AuthenticationError.MISSING_SOAP_REQUEST_HEADER",
        "AuthenticationError.MISSING_AUTHENTICATION_HTTP_HEADER",
        "AuthenticationError.MISSING_AUTHENTICATION",
        "AuthenticationError.NETWORK_API_ACCESS_DISABLED",
        "AuthenticationError.NO_NETWORKS_TO_ACCESS",
        "AuthenticationError.NETWORK_NOT_FOUND",
        "AuthenticationError.NETWORK_CODE_REQUIRED",
        "AuthenticationError.UNDER_INVESTIGATION",
    ];
    PERMISSION_REASONS
        .iter()
        .any(|reason| value.contains(reason))
}

fn dependency_proof_incomplete(placement_summary: &Value, line_item_summary: &Value) -> bool {
    placement_summary
        .get("proof_state")
        .and_then(Value::as_str)
        .map(|state| state != "complete_for_page")
        .unwrap_or(true)
        || line_item_summary
            .get("proof_state")
            .and_then(Value::as_str)
            .map(|state| state != "complete")
            .unwrap_or(true)
}

fn dependency_proof_flags(
    targets: &[DependencyProbeTarget],
    placement_summary: &Value,
    line_item_summary: &Value,
    target_resolution_incomplete: bool,
) -> Value {
    let placement_state = placement_summary
        .get("proof_state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let line_item_state = line_item_summary
        .get("proof_state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let line_item_progress_incomplete = line_item_summary
        .get("total_result_set_size")
        .and_then(Value::as_u64)
        .zip(
            line_item_summary
                .get("inspected_results")
                .and_then(Value::as_u64),
        )
        .is_some_and(|(total, inspected)| total > inspected);
    let line_items_capped_or_truncated = line_item_state == "sample_only"
        || line_item_summary
            .get("response_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || line_item_summary
            .get("missing_total_result_set_size")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || line_item_progress_incomplete;
    json!({
        "target_resolution_incomplete": target_resolution_incomplete,
        "id_only_targets_have_unknown_ancestors": targets.iter().any(DependencyProbeTarget::id_only_without_ancestors),
        "placements_capped_or_shape_unknown": placement_state == "sample_or_shape_incomplete" || placement_state == "blocked",
        "line_items_capped_or_truncated": line_items_capped_or_truncated,
        "soap_manage_scope_required": line_item_summary.get("required_scope").is_some(),
        "line_items_blocked": line_item_state == "blocked",
    })
}

fn probe_ad_unit_target_from_summary(
    summary: &Value,
    network_code: &str,
) -> Option<ProbeAdUnitTarget> {
    if summary.get("proof_state").and_then(Value::as_str) != Some("resolved_exact") {
        return None;
    }
    let ad_unit_id = summary.get("ad_unit_id").and_then(Value::as_str)?;
    let resource_name = summary.get("resource_name").and_then(Value::as_str)?;
    if exact_ad_unit_id_from_resource_name(network_code, resource_name).as_deref()
        != Some(ad_unit_id)
    {
        return None;
    }
    let mut target = ProbeAdUnitTarget::exact(ad_unit_id.to_string());
    if let Some(values) = summary
        .get("ancestor_ad_unit_ids")
        .and_then(Value::as_array)
    {
        target.ancestor_ad_unit_ids.extend(
            values
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|value| exact_ad_unit_id_from_candidate(network_code, value)),
        );
    }
    Some(target)
}

fn yield_group_activity_state(status: Option<&str>) -> &'static str {
    match status.map(|value| value.trim().to_ascii_uppercase()) {
        Some(value) if value == "ACTIVE" => "active",
        Some(value)
            if matches!(
                value.as_str(),
                "INACTIVE" | "ARCHIVED" | "DELETED" | "PAUSED" | "DRAFT"
            ) =>
        {
            "inactive"
        }
        _ => "unknown",
    }
}

fn yield_group_match_classification(
    targeting_match: Option<&TargetingCoverage>,
    exclusion_match: Option<&TargetingCoverage>,
    activity_state: &str,
) -> Option<&'static str> {
    if targeting_match.is_some() && exclusion_match.is_some() {
        return Some("targeted_and_excluded");
    }
    targeting_match?;
    if activity_state == "inactive" {
        Some("targeted_inactive")
    } else if activity_state == "active" {
        Some("targeted_exposed")
    } else {
        Some("targeted_activity_unknown")
    }
}

fn targeting_coverage_for_target(
    values: &[AdUnitTargetingValue],
    target: &ProbeAdUnitTarget,
) -> Option<TargetingCoverage> {
    values
        .iter()
        .find(|value| value.ad_unit_id == target.ad_unit_id)
        .map(|value| TargetingCoverage {
            ad_unit_id: value.ad_unit_id.clone(),
            include_descendants: value.include_descendants,
            match_type: "exact",
        })
        .or_else(|| {
            values
                .iter()
                .find(|value| {
                    value.include_descendants == Some(true)
                        && target.ancestor_ad_unit_ids.contains(&value.ad_unit_id)
                })
                .map(|value| TargetingCoverage {
                    ad_unit_id: value.ad_unit_id.clone(),
                    include_descendants: value.include_descendants,
                    match_type: "ancestor_descendant",
                })
        })
}

fn broad_descendant_targeting_context(
    values: &[AdUnitTargetingValue],
) -> Option<TargetingCoverage> {
    values
        .iter()
        .find(|value| value.include_descendants == Some(true))
        .map(|value| TargetingCoverage {
            ad_unit_id: value.ad_unit_id.clone(),
            include_descendants: value.include_descendants,
            match_type: "broad_descendant_target_unresolved_hierarchy",
        })
}

fn targeting_coverage_json(coverage: &TargetingCoverage) -> Value {
    json!({
        "ad_unit_id": coverage.ad_unit_id.clone(),
        "include_descendants": coverage.include_descendants,
        "match_type": coverage.match_type,
    })
}

fn summarize_rest_discovery(document: &Value) -> Value {
    let mut resources = Vec::new();
    collect_discovery_resources("", document, &mut resources);
    let interesting = resources
        .iter()
        .filter(|resource| {
            let lower = resource.to_ascii_lowercase();
            lower.contains("auction")
                || lower.contains("yield")
                || lower.contains("protection")
                || lower.contains("pricing")
                || lower.contains("inventoryrule")
                || lower.contains("inventory_rule")
        })
        .cloned()
        .collect::<Vec<_>>();
    json!({
        "proof_state": "metadata_read",
        "resource_count": resources.len(),
        "interesting_resources": interesting,
    })
}

fn collect_discovery_resources(prefix: &str, value: &Value, out: &mut Vec<String>) {
    let Some(resources) = value.get("resources").and_then(Value::as_object) else {
        return;
    };
    for (name, resource) in resources {
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        out.push(path.clone());
        collect_discovery_resources(&path, resource, out);
    }
}

fn unsupported_exchange_surfaces(rest_discovery: &Value) -> Vec<Value> {
    let resources = rest_discovery
        .get("interesting_resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    [
        (
            "protections",
            "protection",
            "GAM protection objects are not implemented as a current MCP read surface.",
        ),
        (
            "inventory_rules",
            "inventoryrule",
            "GAM inventory-rule objects are not implemented as a current MCP read surface.",
        ),
        (
            "unified_pricing_rules",
            "pricing",
            "GAM unified pricing rules are not implemented as a current MCP read surface.",
        ),
    ]
    .into_iter()
    .map(|(surface, needle, note)| {
        let exposure = if resources.iter().any(|resource| {
            resource.contains(needle) || resource.contains(&needle.replace('_', ""))
        }) {
            "resource_seen_but_not_integrated"
        } else {
            "not_seen_in_rest_discovery"
        };
        json!({
            "surface": surface,
            "proof_state": "not_proven",
            "api_exposure": exposure,
            "note": note,
        })
    })
    .collect()
}

fn extract_xml_blocks(value: &str, tag: &str) -> Vec<String> {
    extract_xml_elements(value, tag, true)
}

fn extract_xml_first_block(value: &str, tag: &str) -> Option<String> {
    extract_xml_blocks(value, tag).into_iter().next()
}

fn extract_xml_tag_texts(value: &str, tag: &str) -> Vec<String> {
    extract_xml_elements(value, tag, false)
}

fn extract_xml_tag_text(value: &str, tag: &str) -> Option<String> {
    extract_xml_tag_texts(value, tag).into_iter().next()
}

fn extract_xml_elements(value: &str, tag: &str, include_outer: bool) -> Vec<String> {
    let mut out = Vec::new();
    for prefix in ["", "gam:", "soapenv:", "soap:"] {
        let full_tag = format!("{prefix}{tag}");
        let open = format!("<{full_tag}");
        let close = format!("</{prefix}{tag}>");
        let mut search_start = 0;
        while let Some(relative_start) = value[search_start..].find(&open) {
            let start = search_start + relative_start;
            let after_tag = &value[start + open.len()..];
            let starts_with_tag_close = after_tag.starts_with('>');
            let starts_with_space = after_tag.chars().next().is_some_and(char::is_whitespace);
            if !(starts_with_tag_close || starts_with_space) {
                search_start = start + open.len();
                continue;
            }
            let Some(open_end) = after_tag.find('>') else {
                break;
            };
            let content_start = start + open.len() + open_end + 1;
            let Some(relative_end) = value[content_start..].find(&close) else {
                break;
            };
            let content_end = content_start + relative_end;
            if include_outer {
                let end = content_end + close.len();
                out.push(value[start..end].trim().to_string());
                search_start = end;
            } else {
                out.push(value[content_start..content_end].trim().to_string());
                search_start = content_end + close.len();
            }
        }
    }
    out
}

fn extract_xml_date_time(value: &str, tag: &str) -> Option<String> {
    let block = extract_xml_first_block(value, tag)?;
    let date_block = extract_xml_first_block(&block, "date")?;
    let year = extract_xml_tag_text(&date_block, "year")?
        .parse::<u32>()
        .ok()?;
    let month = extract_xml_tag_text(&date_block, "month")?
        .parse::<u32>()
        .ok()?;
    let day = extract_xml_tag_text(&date_block, "day")?
        .parse::<u32>()
        .ok()?;
    let hour = extract_xml_tag_text(&block, "hour")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let minute = extract_xml_tag_text(&block, "minute")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let second = extract_xml_tag_text(&block, "second")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let time_zone = extract_xml_tag_text(&block, "timeZoneId").unwrap_or_default();
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02} {time_zone}"
    ))
}

fn creative_sizes_from_placeholders(blocks: &[String]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| {
            let size = extract_xml_first_block(block, "size")?;
            let width = extract_xml_tag_text(&size, "width")?;
            let height = extract_xml_tag_text(&size, "height")?;
            Some(format!("{width}x{height}"))
        })
        .collect()
}

fn option_string_value(value: Option<String>) -> Value {
    value.map(Value::String).unwrap_or(Value::Null)
}

fn option_u64_value(value: Option<String>) -> Value {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .map(Value::from)
        .unwrap_or(Value::Null)
}

fn option_f64_value(value: Option<String>) -> Value {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .map(Value::from)
        .unwrap_or(Value::Null)
}

fn option_bool_value(value: Option<String>) -> Value {
    value
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
        .map(Value::from)
        .unwrap_or(Value::Null)
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

fn build_soap_payload_template(
    template: SoapPayloadTemplate,
    values: &Map<String, Value>,
) -> Result<Value, AdManagerError> {
    let operation = template.operation();
    let mut warnings = Vec::new();
    let payload_xml = match template {
        SoapPayloadTemplate::OrderById => {
            let order_id = required_id(values, "order_id")?;
            pql_payload(&format!("WHERE id = {order_id}"))
        }
        SoapPayloadTemplate::LineItemById => {
            let line_item_id = required_id(values, "line_item_id")?;
            pql_payload(&format!("WHERE id = {line_item_id}"))
        }
        SoapPayloadTemplate::LineItemsByOrderId => {
            let order_id = required_id(values, "order_id")?;
            pql_payload(&format!("WHERE orderId = {order_id} ORDER BY id ASC"))
        }
        SoapPayloadTemplate::CreativesByAdvertiserName => {
            let advertiser_id = required_id(values, "advertiser_id")?;
            let name_contains = required_safe_name_fragment(values, "name_contains")?;
            let pql_name = escape_pql_single_quoted_like_fragment(&name_contains);
            pql_payload(&format!(
                "WHERE advertiserId = {advertiser_id} AND name LIKE '%{pql_name}%' ORDER BY id ASC"
            ))
        }
        SoapPayloadTemplate::LicasByLineItemId => {
            let line_item_id = required_id(values, "line_item_id")?;
            pql_payload(&format!(
                "WHERE lineItemId = {line_item_id} ORDER BY creativeId ASC"
            ))
        }
        SoapPayloadTemplate::LicaPreviewUrl => {
            let line_item_id = required_id(values, "line_item_id")?;
            let creative_id = required_id(values, "creative_id")?;
            format!(
                "<lineItemCreativeAssociation>\n  <lineItemId>{line_item_id}</lineItemId>\n  <creativeId>{creative_id}</creativeId>\n</lineItemCreativeAssociation>"
            )
        }
        SoapPayloadTemplate::CreateLica => {
            warnings.push(
                "This payload creates a line-item creative association when applied through gam_soap_trafficking_apply."
                    .to_string(),
            );
            let line_item_id = required_id(values, "line_item_id")?;
            let creative_id = required_id(values, "creative_id")?;
            format!(
                "<lineItemCreativeAssociations>\n  <lineItemId>{line_item_id}</lineItemId>\n  <creativeId>{creative_id}</creativeId>\n</lineItemCreativeAssociations>"
            )
        }
        SoapPayloadTemplate::PauseLineItem => {
            warnings.push(
                "This payload pauses delivery for the matching line item when applied.".to_string(),
            );
            line_item_action_payload("PauseLineItems", required_id(values, "line_item_id")?)
        }
        SoapPayloadTemplate::ResumeLineItem => {
            warnings.push(
                "This payload resumes delivery for the matching line item when applied."
                    .to_string(),
            );
            line_item_action_payload("ResumeLineItems", required_id(values, "line_item_id")?)
        }
        SoapPayloadTemplate::ArchiveLineItem => {
            warnings.push(
                "This payload archives the matching line item when applied; use only with explicit operator approval."
                    .to_string(),
            );
            line_item_action_payload("ArchiveLineItems", required_id(values, "line_item_id")?)
        }
        SoapPayloadTemplate::DeliveryForecastByLineItemIds => {
            let line_item_ids = required_id_list(values, "line_item_ids", 50)?;
            let line_item_ids_xml = line_item_ids
                .into_iter()
                .map(|line_item_id| format!("<lineItemIds>{line_item_id}</lineItemIds>"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{line_item_ids_xml}\n<forecastOptions />")
        }
        SoapPayloadTemplate::AvailabilityForecastByLineItemId => {
            let line_item_id = required_id(values, "line_item_id")?;
            format!("<lineItemId>{line_item_id}</lineItemId>")
        }
        SoapPayloadTemplate::YieldGroupsByStatement => {
            let query = required_safe_pql_query(values, "query")?;
            pql_statement_payload(&query)
        }
        SoapPayloadTemplate::YieldGroupsAll => pql_statement_payload("LIMIT 500"),
        SoapPayloadTemplate::YieldPartners => String::new(),
    };

    Ok(json!({
        "template": template.as_str(),
        "operation": operation.as_str(),
        "payload_xml": payload_xml,
        "warnings": warnings,
        "required_values": template.required_values(),
        "next_tool": "gam_soap_trafficking_plan",
        "next_request_shape": {
            "network_code": "<network code>",
            "operation": operation.as_str(),
            "payload_xml": payload_xml,
            "reason": "<why this SOAP operation is being planned>"
        },
        "mutation_performed": false,
        "upstream_called": false,
    }))
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AdUnitTargetingValue {
    ad_unit_id: String,
    include_descendants: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeAdUnitTarget {
    ad_unit_id: String,
    ancestor_ad_unit_ids: BTreeSet<String>,
}

impl ProbeAdUnitTarget {
    fn exact(ad_unit_id: String) -> Self {
        Self {
            ad_unit_id,
            ancestor_ad_unit_ids: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetingCoverage {
    ad_unit_id: String,
    include_descendants: Option<bool>,
    match_type: &'static str,
}

#[derive(Debug, Clone)]
struct YieldGroupExclusionDraft {
    network_code: String,
    api_version: String,
    yield_group_id: String,
    yield_group_name: Option<String>,
    exchange_status: Option<String>,
    format: Option<String>,
    environment_type: Option<String>,
    total_result_set_size: Option<u64>,
    read_request_id: Option<String>,
    read_response_time: Option<String>,
    read_payload_xml: String,
    current_yield_group_xml: String,
    update_payload_xml: String,
    current_yield_group_fingerprint: String,
    update_payload_fingerprint: String,
    targeted_ad_units: Vec<AdUnitTargetingValue>,
    current_excluded_ad_units: Vec<AdUnitTargetingValue>,
    requested_excluded_ad_unit_ids: Vec<String>,
    requested_exclusion_include_descendants: bool,
    already_excluded_ad_unit_ids: Vec<String>,
    added_excluded_ad_unit_ids: Vec<String>,
    updated_excluded_ad_unit_ids: Vec<String>,
    noop: bool,
}

impl YieldGroupExclusionDraft {
    fn all_requested_ids_currently_excluded(&self) -> bool {
        self.requested_excluded_ad_unit_ids.iter().all(|id| {
            self.current_excluded_ad_units.iter().any(|value| {
                value.ad_unit_id == *id
                    && value.include_descendants
                        == Some(self.requested_exclusion_include_descendants)
            })
        })
    }
}

fn validate_yield_group_exclusion_context(
    request: &YieldGroupExclusionRequestArgs,
) -> Result<(), AdManagerError> {
    if request.reason.trim().is_empty() {
        return Err(AdManagerError::invalid(
            "reason",
            "must explain why the yield-group exclusion change is being planned",
        ));
    }
    let _ = parse_positive_id_string("yield_group_id", &request.yield_group_id)?;
    let _ = validated_excluded_ad_unit_ids(&request.excluded_ad_unit_ids)?;
    Ok(())
}

fn validate_yield_group_exclusion_apply_context(
    request: &YieldGroupExclusionRequestArgs,
) -> Result<(), AdManagerError> {
    if request
        .expected_impact
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        return Err(AdManagerError::invalid(
            "expected_impact",
            "is required before applying yield-group exclusion updates",
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
            "is required before applying yield-group exclusion updates",
        ));
    }
    Ok(())
}

async fn build_yield_group_exclusion_draft(
    server: &AdManagerServer,
    request: &YieldGroupExclusionRequestArgs,
) -> Result<YieldGroupExclusionDraft, AdManagerError> {
    let yield_group_id = parse_positive_id_string("yield_group_id", &request.yield_group_id)?;
    let requested_excluded_ad_unit_ids =
        validated_excluded_ad_unit_ids(&request.excluded_ad_unit_ids)?;
    let read_payload_xml = pql_statement_payload(&format!("WHERE id = {yield_group_id}"));
    let read_plan = server.client().build_soap_trafficking_plan(
        &request.network_code,
        request.api_version.as_deref(),
        SoapTraffickingOperation::GetYieldGroupsByStatement,
        &read_payload_xml,
    )?;
    let read_result = server
        .client()
        .execute_soap_trafficking_plan(&read_plan)
        .await?;
    if read_result.upstream_status >= 400 || read_result.soap_fault.is_some() {
        return Err(AdManagerError::UpstreamApi {
            status: read_result.upstream_status,
            message: crate::client::soap_error_message(&read_result),
        });
    }

    let current_yield_group_xml =
        exact_yield_group_from_readback(&read_result.upstream_response_xml, &yield_group_id)?;
    let total_result_set_size =
        extract_xml_tag_text(&read_result.upstream_response_xml, "totalResultSetSize")
            .and_then(|value| value.parse::<u64>().ok());
    let update = build_yield_group_exclusion_update(
        &current_yield_group_xml,
        &yield_group_id,
        &requested_excluded_ad_unit_ids,
    )?;
    let update_payload_fingerprint = stable_fingerprint(&update.payload_xml);
    let current_yield_group_fingerprint = stable_fingerprint(&current_yield_group_xml);

    Ok(YieldGroupExclusionDraft {
        network_code: read_plan.network_code,
        api_version: read_plan.api_version,
        yield_group_id: yield_group_id.clone(),
        yield_group_name: yield_group_name(&current_yield_group_xml),
        exchange_status: extract_xml_tag_text(&current_yield_group_xml, "exchangeStatus")
            .or_else(|| extract_xml_tag_text(&current_yield_group_xml, "status")),
        format: extract_xml_tag_text(&current_yield_group_xml, "format"),
        environment_type: extract_xml_tag_text(&current_yield_group_xml, "environmentType"),
        total_result_set_size,
        read_request_id: read_result.request_id,
        read_response_time: read_result.response_time,
        read_payload_xml,
        current_yield_group_xml,
        update_payload_xml: update.payload_xml,
        current_yield_group_fingerprint,
        update_payload_fingerprint,
        targeted_ad_units: update.targeted_ad_units,
        current_excluded_ad_units: update.current_excluded_ad_units,
        requested_excluded_ad_unit_ids,
        requested_exclusion_include_descendants: YIELD_GROUP_EXCLUSION_INCLUDE_DESCENDANTS,
        already_excluded_ad_unit_ids: update.already_excluded_ad_unit_ids,
        added_excluded_ad_unit_ids: update.added_excluded_ad_unit_ids.clone(),
        updated_excluded_ad_unit_ids: update.updated_excluded_ad_unit_ids.clone(),
        noop: update.added_excluded_ad_unit_ids.is_empty()
            && update.updated_excluded_ad_unit_ids.is_empty(),
    })
}

#[derive(Debug, Clone)]
struct YieldGroupExclusionUpdate {
    payload_xml: String,
    targeted_ad_units: Vec<AdUnitTargetingValue>,
    current_excluded_ad_units: Vec<AdUnitTargetingValue>,
    already_excluded_ad_unit_ids: Vec<String>,
    added_excluded_ad_unit_ids: Vec<String>,
    updated_excluded_ad_unit_ids: Vec<String>,
}

fn build_yield_group_exclusion_update(
    current_yield_group_xml: &str,
    yield_group_id: &str,
    requested_excluded_ad_unit_ids: &[String],
) -> Result<YieldGroupExclusionUpdate, AdManagerError> {
    let observed_yield_group_id =
        yield_group_id_from_xml(current_yield_group_xml).ok_or_else(|| {
            AdManagerError::invalid(
                "yield_group_readback",
                "readback did not include a yield group id",
            )
        })?;
    if observed_yield_group_id != yield_group_id {
        return Err(AdManagerError::invalid(
            "yield_group_readback",
            "readback yield group id did not match the requested target",
        ));
    }

    let targeting_block = extract_xml_first_block(current_yield_group_xml, "targeting")
        .ok_or_else(|| {
            AdManagerError::invalid(
                "yield_group_readback",
                "yield group readback did not include targeting; refusing to synthesize targeting from scratch",
            )
        })?;
    let inventory_block = extract_xml_first_block(&targeting_block, "inventoryTargeting")
        .unwrap_or_else(|| "<inventoryTargeting />".to_string());
    let targeted_ad_units = ad_unit_targeting_values(&inventory_block, "targetedAdUnits");
    let current_excluded_ad_units = ad_unit_targeting_values(&inventory_block, "excludedAdUnits");
    let targeted_ids = targeted_ad_units
        .iter()
        .map(|value| value.ad_unit_id.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(conflicting_id) = requested_excluded_ad_unit_ids
        .iter()
        .find(|id| targeted_ids.contains(id.as_str()))
    {
        return Err(AdManagerError::invalid(
            "excluded_ad_unit_ids",
            format!(
                "ad unit {conflicting_id} is directly targeted by this yield group; refusing to target and exclude the same exact ad unit"
            ),
        ));
    }

    let requested_exclusion_include_descendants = YIELD_GROUP_EXCLUSION_INCLUDE_DESCENDANTS;
    let mut already_excluded_ad_unit_ids = Vec::new();
    let mut added_excluded_ad_unit_ids = Vec::new();
    let mut updated_excluded_ad_unit_ids = Vec::new();
    for id in requested_excluded_ad_unit_ids {
        match current_excluded_ad_units
            .iter()
            .find(|value| value.ad_unit_id == *id)
        {
            Some(value)
                if value.include_descendants == Some(requested_exclusion_include_descendants) =>
            {
                already_excluded_ad_unit_ids.push(id.clone());
            }
            Some(_) => {
                updated_excluded_ad_unit_ids.push(id.clone());
            }
            None => {
                added_excluded_ad_unit_ids.push(id.clone());
            }
        }
    }

    let updated_inventory_xml = yield_group_inventory_targeting_with_exclusions(
        &inventory_block,
        requested_excluded_ad_unit_ids,
        requested_exclusion_include_descendants,
    );
    let updated_yield_group_xml =
        if extract_xml_first_block(&targeting_block, "inventoryTargeting").is_some() {
            replace_first_xml_block(
                current_yield_group_xml,
                "inventoryTargeting",
                &updated_inventory_xml,
            )
            .ok_or_else(|| {
                AdManagerError::invalid(
                    "yield_group_readback",
                    "could not replace inventoryTargeting in yield group readback",
                )
            })?
        } else {
            insert_inventory_targeting(current_yield_group_xml, &updated_inventory_xml).ok_or_else(
                || {
                    AdManagerError::invalid(
                        "yield_group_readback",
                        "could not insert inventoryTargeting in yield group targeting",
                    )
                },
            )?
        };
    let payload_xml = format!(
        "<yieldGroups>\n{}\n</yieldGroups>",
        indent_xml_fragment(&updated_yield_group_xml, 2)
    );

    Ok(YieldGroupExclusionUpdate {
        payload_xml,
        targeted_ad_units,
        current_excluded_ad_units,
        already_excluded_ad_unit_ids,
        added_excluded_ad_unit_ids,
        updated_excluded_ad_unit_ids,
    })
}

fn exact_yield_group_from_readback(
    xml: &str,
    yield_group_id: &str,
) -> Result<String, AdManagerError> {
    let results = extract_xml_blocks(xml, "results");
    let mut matching = results
        .into_iter()
        .filter(|result| yield_group_id_from_xml(result).as_deref() == Some(yield_group_id))
        .collect::<Vec<_>>();
    match matching.len() {
        1 => {
            let result = matching.remove(0);
            Ok(strip_outer_xml_block(&result, "results").unwrap_or(result))
        }
        0 => Err(AdManagerError::UpstreamApi {
            status: 404,
            message: format!("yield group {yield_group_id} was not found by statement readback"),
        }),
        _ => Err(AdManagerError::UpstreamApi {
            status: 500,
            message: format!("yield group {yield_group_id} matched multiple readback rows"),
        }),
    }
}

fn yield_group_inventory_targeting_with_exclusions(
    inventory_block: &str,
    requested_excluded_ad_unit_ids: &[String],
    include_descendants: bool,
) -> String {
    let targeted_blocks = extract_xml_blocks(inventory_block, "targetedAdUnits");
    let requested_ids = requested_excluded_ad_unit_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut seen_requested_ids = BTreeSet::new();
    let mut excluded_blocks = Vec::new();
    for block in extract_xml_blocks(inventory_block, "excludedAdUnits") {
        if let Some(ad_unit_id) = extract_xml_tag_text(&block, "adUnitId")
            && requested_ids.contains(ad_unit_id.as_str())
        {
            seen_requested_ids.insert(ad_unit_id.clone());
            excluded_blocks.push(excluded_ad_unit_xml(&ad_unit_id, include_descendants));
            continue;
        }
        excluded_blocks.push(block);
    }
    excluded_blocks.extend(
        requested_excluded_ad_unit_ids
            .iter()
            .filter(|id| !seen_requested_ids.contains(id.as_str()))
            .map(|id| excluded_ad_unit_xml(id, include_descendants)),
    );
    let placement_ids = extract_xml_tag_texts(inventory_block, "targetedPlacementIds");

    let mut children = Vec::new();
    children.extend(targeted_blocks);
    children.extend(excluded_blocks);
    children.extend(placement_ids.into_iter().map(|id| {
        format!(
            "<targetedPlacementIds>{}</targetedPlacementIds>",
            escape_xml_text(&id)
        )
    }));
    if children.is_empty() {
        "<inventoryTargeting />".to_string()
    } else {
        format!(
            "<inventoryTargeting>\n{}\n</inventoryTargeting>",
            indent_xml_fragment(&children.join("\n"), 2)
        )
    }
}

fn excluded_ad_unit_xml(ad_unit_id: &str, include_descendants: bool) -> String {
    format!(
        "<excludedAdUnits>\n  <adUnitId>{}</adUnitId>\n  <includeDescendants>{}</includeDescendants>\n</excludedAdUnits>",
        escape_xml_text(ad_unit_id),
        include_descendants
    )
}

fn insert_inventory_targeting(yield_group_xml: &str, inventory_xml: &str) -> Option<String> {
    let (start, end) = find_first_xml_block_range(yield_group_xml, "targeting")?;
    let targeting_block = &yield_group_xml[start..end];
    let targeting_inner = strip_outer_xml_block(targeting_block, "targeting")?;
    let updated_targeting = if targeting_inner.trim().is_empty() {
        format!(
            "<targeting>\n{}\n</targeting>",
            indent_xml_fragment(inventory_xml, 2)
        )
    } else {
        format!(
            "<targeting>\n{}\n{}\n</targeting>",
            indent_xml_fragment(inventory_xml, 2),
            indent_xml_fragment(&targeting_inner, 2)
        )
    };
    let mut updated = String::with_capacity(
        yield_group_xml.len()
            + updated_targeting
                .len()
                .saturating_sub(targeting_block.len()),
    );
    updated.push_str(&yield_group_xml[..start]);
    updated.push_str(&updated_targeting);
    updated.push_str(&yield_group_xml[end..]);
    Some(updated)
}

fn ad_unit_targeting_values(value: &str, tag: &str) -> Vec<AdUnitTargetingValue> {
    extract_xml_blocks(value, tag)
        .into_iter()
        .filter_map(|block| {
            let ad_unit_id = extract_xml_tag_text(&block, "adUnitId")?;
            Some(AdUnitTargetingValue {
                ad_unit_id,
                include_descendants: extract_xml_tag_text(&block, "includeDescendants").and_then(
                    |value| match value.trim().to_ascii_lowercase().as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    },
                ),
            })
        })
        .collect()
}

fn yield_group_id_from_xml(value: &str) -> Option<String> {
    extract_xml_tag_text(value, "yieldGroupId").or_else(|| extract_xml_tag_text(value, "id"))
}

fn yield_group_name(value: &str) -> Option<String> {
    extract_xml_tag_text(value, "yieldGroupName").or_else(|| extract_xml_tag_text(value, "name"))
}

fn strip_outer_xml_block(value: &str, tag: &str) -> Option<String> {
    let (start, end) = find_first_xml_block_range(value, tag)?;
    let open_end = value[start..end].find('>')?;
    let content_start = start + open_end + 1;
    let close_start = value[..end].rfind("</")?;
    Some(value[content_start..close_start].trim().to_string())
}

fn replace_first_xml_block(value: &str, tag: &str, replacement: &str) -> Option<String> {
    let (start, end) = find_first_xml_block_range(value, tag)?;
    let mut updated =
        String::with_capacity(value.len() + replacement.len().saturating_sub(end - start));
    updated.push_str(&value[..start]);
    updated.push_str(replacement);
    updated.push_str(&value[end..]);
    Some(updated)
}

fn find_first_xml_block_range(value: &str, tag: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for prefix in ["", "gam:", "soapenv:", "soap:"] {
        let full_tag = format!("{prefix}{tag}");
        let open = format!("<{full_tag}");
        let close = format!("</{prefix}{tag}>");
        let mut search_start = 0;
        while let Some(relative_start) = value[search_start..].find(&open) {
            let start = search_start + relative_start;
            let after_tag = &value[start + open.len()..];
            let starts_with_tag_close = after_tag.starts_with('>') || after_tag.starts_with("/>");
            let starts_with_space = after_tag.chars().next().is_some_and(char::is_whitespace);
            if !(starts_with_tag_close || starts_with_space) {
                search_start = start + open.len();
                continue;
            }
            let open_end = after_tag.find('>')?;
            if after_tag[..=open_end].trim_end().ends_with("/>") {
                let end = start + open.len() + open_end + 1;
                best = choose_earlier_range(best, (start, end));
                break;
            }
            let content_start = start + open.len() + open_end + 1;
            let Some(relative_end) = value[content_start..].find(&close) else {
                break;
            };
            let end = content_start + relative_end + close.len();
            best = choose_earlier_range(best, (start, end));
            break;
        }
    }
    best
}

fn choose_earlier_range(
    current: Option<(usize, usize)>,
    candidate: (usize, usize),
) -> Option<(usize, usize)> {
    match current {
        Some(existing) if existing.0 <= candidate.0 => Some(existing),
        _ => Some(candidate),
    }
}

fn indent_xml_fragment(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{prefix}{}", line.trim_end())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_positive_id_string(field: &'static str, value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(AdManagerError::invalid(
            field,
            "must be a positive numeric ID",
        ));
    }
    match trimmed.parse::<u64>() {
        Ok(id) if id > 0 => Ok(id.to_string()),
        _ => Err(AdManagerError::invalid(
            field,
            "must be a positive numeric ID",
        )),
    }
}

fn validated_excluded_ad_unit_ids(values: &[String]) -> Result<Vec<String>, AdManagerError> {
    if values.is_empty() {
        return Err(AdManagerError::invalid(
            "excluded_ad_unit_ids",
            "must contain at least one exact ad-unit ID",
        ));
    }
    if values.len() > 100 {
        return Err(AdManagerError::invalid(
            "excluded_ad_unit_ids",
            "must contain at most 100 exact ad-unit IDs per guarded update",
        ));
    }
    let mut ids = BTreeSet::new();
    for value in values {
        let id = parse_positive_id_string("excluded_ad_unit_ids", value)?;
        if !ids.insert(id.clone()) {
            return Err(AdManagerError::invalid(
                "excluded_ad_unit_ids",
                format!("must not contain duplicate ID {id}"),
            ));
        }
    }
    Ok(ids.into_iter().collect())
}

fn guarded_yield_group_exclusion_identifiers(
    request: &YieldGroupExclusionRequestArgs,
    draft: &YieldGroupExclusionDraft,
) -> Result<(String, String, String), AdManagerError> {
    let target = format!(
        "YieldGroupService.updateYieldGroups:{}:{}",
        draft.api_version, draft.yield_group_id
    );
    let seed =
        GuardedActionPlanSeed::new("gam_yield_group_exclusions", &request.network_code, &target)
            .map_err(|err| AdManagerError::invalid("plan_seed", err.to_string()))?;
    let fingerprint_input = json!({
        "network_code": request.network_code,
        "api_version": draft.api_version,
        "yield_group_id": draft.yield_group_id,
        "requested_excluded_ad_unit_ids": draft.requested_excluded_ad_unit_ids,
        "current_yield_group_fingerprint": draft.current_yield_group_fingerprint,
        "update_payload_fingerprint": draft.update_payload_fingerprint,
    });
    let fingerprint = stable_fingerprint(&fingerprint_input.to_string());
    let plan_id = format!("{}.{}", seed.stable_plan_id(), fingerprint);
    let confirmation_token = format!("confirm-gam-yield-group-exclusions-{fingerprint}");
    Ok((plan_id, confirmation_token, fingerprint))
}

fn yield_group_exclusion_request_summary(
    request: &YieldGroupExclusionRequestArgs,
    draft: &YieldGroupExclusionDraft,
) -> Value {
    json!({
        "network_code": request.network_code,
        "api_version": draft.api_version,
        "yield_group_id": draft.yield_group_id,
        "requested_excluded_ad_unit_ids": draft.requested_excluded_ad_unit_ids,
        "reason": request.reason,
        "idempotency_key": request.idempotency_key,
    })
}

fn yield_group_exclusion_draft_summary(
    draft: &YieldGroupExclusionDraft,
    include_payload_xml: bool,
) -> Value {
    let mut value = json!({
        "network_code": draft.network_code,
        "api_version": draft.api_version,
        "yield_group_id": draft.yield_group_id,
        "yield_group_name": draft.yield_group_name,
        "exchange_status": draft.exchange_status,
        "format": draft.format,
        "environment_type": draft.environment_type,
        "total_result_set_size": draft.total_result_set_size,
        "read_request_id": draft.read_request_id,
        "read_response_time": draft.read_response_time,
        "targeted_ad_units": draft.targeted_ad_units,
        "current_excluded_ad_units": draft.current_excluded_ad_units,
        "requested_excluded_ad_unit_ids": draft.requested_excluded_ad_unit_ids,
        "requested_exclusion_include_descendants": draft.requested_exclusion_include_descendants,
        "already_excluded_ad_unit_ids": draft.already_excluded_ad_unit_ids,
        "added_excluded_ad_unit_ids": draft.added_excluded_ad_unit_ids,
        "updated_excluded_ad_unit_ids": draft.updated_excluded_ad_unit_ids,
        "apply_required": !draft.noop,
        "readback_proves_requested_exclusions": draft.all_requested_ids_currently_excluded(),
        "current_yield_group_xml_bytes": draft.current_yield_group_xml.len(),
        "current_yield_group_fingerprint": draft.current_yield_group_fingerprint,
        "update_payload_xml_bytes": draft.update_payload_xml.len(),
        "update_payload_fingerprint": draft.update_payload_fingerprint,
    });
    if include_payload_xml {
        value["read_payload_xml"] = json!(draft.read_payload_xml);
        value["update_payload_xml"] = json!(draft.update_payload_xml);
    }
    value
}

fn yield_group_update_request_to_json(
    request: &YieldGroupUpdateSoapRequest,
    include_payload_xml: bool,
) -> Value {
    let mut value = json!({
        "network_code": request.network_code,
        "api_version": request.api_version,
        "service": request.service,
        "method": request.method,
        "endpoint": request.endpoint,
        "namespace": request.namespace,
        "target": request.target,
        "payload_xml_bytes": request.payload_xml.len(),
        "payload_fingerprint": stable_fingerprint(&request.payload_xml),
    });
    if include_payload_xml {
        value["payload_xml"] = json!(request.payload_xml);
        value["envelope_xml"] = json!(request.envelope_xml);
    }
    value
}

fn yield_group_exclusion_warnings(
    write_mode: GuardedActionRuntimeMode,
    scope: &str,
    request: &YieldGroupExclusionRequestArgs,
    draft: &YieldGroupExclusionDraft,
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
    if draft.noop {
        warnings.push(
            "No mutation is needed: every requested ad-unit ID is already excluded with includeDescendants=true."
                .to_string(),
        );
    } else {
        warnings.push(
            "Requested excludedAdUnits are written with includeDescendants=true because Google Ad Manager can reject self-only inventory-unit exclusions with InventoryTargetingError.SELF_ONLY_INVENTORY_UNIT_NOT_ALLOWED."
                .to_string(),
        );
        warnings.push(
            "This update changes only the yield-group inventory exclusions; it does not change line-item targeting or sponsorship line items."
                .to_string(),
        );
    }
    warnings
}

fn pql_payload(query: &str) -> String {
    format!(
        "<filterStatement>\n  <query>{}</query>\n</filterStatement>",
        escape_xml_text(query)
    )
}

fn pql_statement_payload(query: &str) -> String {
    format!(
        "<statement>\n  <query>{}</query>\n</statement>",
        escape_xml_text(query)
    )
}

fn line_item_action_payload(action: &str, line_item_id: u64) -> String {
    format!(
        "<lineItemAction xsi:type=\"{action}\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"/>\n<filterStatement>\n  <query>WHERE id = {line_item_id}</query>\n</filterStatement>"
    )
}

fn required_id(values: &Map<String, Value>, field: &'static str) -> Result<u64, AdManagerError> {
    let value = values.get(field).ok_or_else(|| {
        AdManagerError::invalid(field, "is required for this SOAP payload template")
    })?;
    parse_positive_id(field, value)
}

fn required_id_list(
    values: &Map<String, Value>,
    field: &'static str,
    max_len: usize,
) -> Result<Vec<u64>, AdManagerError> {
    let value = values.get(field).ok_or_else(|| {
        AdManagerError::invalid(field, "is required for this SOAP payload template")
    })?;
    let Value::Array(items) = value else {
        return Err(AdManagerError::invalid(
            field,
            "must be an array of positive numeric IDs",
        ));
    };
    if items.is_empty() {
        return Err(AdManagerError::invalid(
            field,
            "must contain at least one positive numeric ID",
        ));
    }
    if items.len() > max_len {
        return Err(AdManagerError::invalid(
            field,
            format!("must contain at most {max_len} IDs"),
        ));
    }

    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let id = parse_positive_id(field, item)?;
        if ids.contains(&id) {
            return Err(AdManagerError::invalid(
                field,
                format!("must not contain duplicate ID {id}"),
            ));
        }
        ids.push(id);
    }
    Ok(ids)
}

fn parse_positive_id(field: &'static str, value: &Value) -> Result<u64, AdManagerError> {
    let parsed = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
                None
            } else {
                trimmed.parse::<u64>().ok()
            }
        }
        _ => None,
    };
    match parsed {
        Some(id) if id > 0 => Ok(id),
        _ => Err(AdManagerError::invalid(
            field,
            "must be a positive numeric ID",
        )),
    }
}

fn required_safe_name_fragment(
    values: &Map<String, Value>,
    field: &'static str,
) -> Result<String, AdManagerError> {
    let value = values.get(field).ok_or_else(|| {
        AdManagerError::invalid(field, "is required for this SOAP payload template")
    })?;
    let Some(text) = value.as_str() else {
        return Err(AdManagerError::invalid(
            field,
            "must be a short string without PQL wildcard characters",
        ));
    };
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 80 {
        return Err(AdManagerError::invalid(
            field,
            "must be between 1 and 80 characters",
        ));
    }
    if trimmed
        .chars()
        .any(|ch| matches!(ch, '%' | '_' | '"' | ';' | '<' | '>' | '\n' | '\r'))
    {
        return Err(AdManagerError::invalid(
            field,
            "must not contain PQL wildcard, double quote, XML, semicolon, or newline characters",
        ));
    }
    if !trimmed.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                ' ' | '-' | '/' | '(' | ')' | '.' | ':' | '+' | '&' | '\''
            )
    }) {
        return Err(AdManagerError::invalid(
            field,
            "contains unsupported characters for a safe PQL LIKE fragment",
        ));
    }
    Ok(trimmed.to_string())
}

fn required_safe_pql_query(
    values: &Map<String, Value>,
    field: &'static str,
) -> Result<String, AdManagerError> {
    let value = values.get(field).ok_or_else(|| {
        AdManagerError::invalid(field, "is required for this SOAP payload template")
    })?;
    let Some(text) = value.as_str() else {
        return Err(AdManagerError::invalid(
            field,
            "must be a bounded PQL statement string",
        ));
    };
    validate_safe_pql_query_text(field, text)
}

fn validate_safe_pql_query_text(field: &'static str, text: &str) -> Result<String, AdManagerError> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 240 {
        return Err(AdManagerError::invalid(
            field,
            "must be between 1 and 240 characters",
        ));
    }
    if trimmed
        .chars()
        .any(|ch| matches!(ch, ';' | '<' | '>' | '\n' | '\r' | '\0'))
    {
        return Err(AdManagerError::invalid(
            field,
            "must not contain XML, semicolon, null, or newline characters",
        ));
    }
    let upper = trimmed.to_ascii_uppercase();
    if !(upper.starts_with("WHERE ")
        || upper.starts_with("ORDER BY ")
        || upper.starts_with("LIMIT "))
    {
        return Err(AdManagerError::invalid(
            field,
            "must start with WHERE, ORDER BY, or LIMIT",
        ));
    }
    Ok(trimmed.to_string())
}

fn bounded_line_item_pql_query(query: &str) -> Result<String, AdManagerError> {
    let mut bounded = validate_safe_pql_query_text("query", query)?;
    let limit = pql_limit_value(&bounded);
    match limit {
        Some(1..=1_000) => {}
        Some(_) => {
            return Err(AdManagerError::invalid(
                "query",
                "LIMIT must be between 1 and 1000 for scratchpad line-item ingests",
            ));
        }
        None => {
            if bounded.len() > 230 {
                return Err(AdManagerError::invalid(
                    "query",
                    "query is too long to append the automatic LIMIT 500 cap",
                ));
            }
            bounded.push_str(" LIMIT 500");
        }
    }
    Ok(bounded)
}

fn pql_limit_value(query: &str) -> Option<u64> {
    let upper = query.to_ascii_uppercase();
    let limit_index = upper.rfind("LIMIT")?;
    let before = upper[..limit_index].chars().next_back();
    let after = upper[limit_index + "LIMIT".len()..].chars().next();
    if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    query[limit_index + "LIMIT".len()..]
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
}

fn escape_pql_single_quoted_like_fragment(input: &str) -> String {
    input.replace('\'', "''")
}

fn escape_xml_text(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
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
        },
        {
            "service": "YieldGroupService",
            "operations": [
                "get_yield_groups_by_statement",
                "get_yield_partners"
            ],
            "mutating_operations": [],
            "typed_helpers": [
                "gam_yield_group_exclusions_preview",
                "gam_yield_group_exclusions_apply"
            ]
        }
    ])
}

fn trafficking_gap_matrix() -> Value {
    json!([
        {
            "surface": "high_level_builders",
            "status": "partial payload templates",
            "impact": "operators can generate common read, LICA, action, and forecast payload_xml fragments; full order, line item, and creative builders are still manual",
            "follow_up": "add richer typed builders for common order, line item, creative, and forecast payloads after validating real campaign traffic"
        },
        {
            "surface": "typed_soap_response_models",
            "status": "raw bounded XML response",
            "impact": "agents can execute end-to-end but must inspect XML response fields directly",
            "follow_up": "parse common rval/update result/page response shapes into structured JSON alongside raw XML"
        },
        {
            "surface": "post_apply_readback_automation",
            "status": "partial typed helpers",
            "impact": "generic mutating SOAP apply returns upstream response and still needs follow-up proof; yield-group descendant-safe exclusion apply has built-in post-apply readback",
            "follow_up": "support optional readback_request payloads on generic SOAP apply"
        },
        {
            "surface": "account_level_protection_surfaces",
            "status": "partial API proof",
            "impact": "exchange/protection probe can prove exposed ad-unit flags, private auctions/deals, and yield groups, but must report protections, inventory rules, and unified pricing rules as unproven until an authoritative API or browser proof surface exists",
            "follow_up": "add authoritative read coverage if Google exposes these surfaces or a supported browser/admin read adapter is approved"
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

fn soap_line_items_ingest_columns() -> Vec<ScratchpadIngestColumn> {
    [
        ("network_code", "string"),
        ("api_version", "string"),
        ("row_index", "integer"),
        ("request_id", "string"),
        ("response_time", "string"),
        ("response_truncated", "boolean"),
        ("order_id", "integer"),
        ("order_name", "string"),
        ("line_item_id", "integer"),
        ("line_item_name", "string"),
        ("status", "string"),
        ("reservation_status", "string"),
        ("line_item_type", "string"),
        ("priority", "integer"),
        ("cost_type", "string"),
        ("delivery_rate_type", "string"),
        ("roadblocking_type", "string"),
        ("start_date_time", "string"),
        ("end_date_time", "string"),
        ("creative_sizes", "string"),
        ("expected_creative_count", "integer"),
        ("impressions_delivered", "integer"),
        ("clicks_delivered", "integer"),
        ("expected_delivery_percentage", "double"),
        ("actual_delivery_percentage", "double"),
        ("primary_goal_type", "string"),
        ("primary_goal_unit_type", "string"),
        ("primary_goal_units", "integer"),
        ("is_missing_creatives", "boolean"),
        ("is_archived", "boolean"),
        ("target_ad_unit_ids_json", "string"),
        ("custom_targeting_key_ids_json", "string"),
        ("custom_targeting_value_ids_json", "string"),
        ("upstream_xml", "string"),
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

fn soap_line_item_rows_for_scratchpad(
    xml: &str,
    network_code: &str,
    api_version: &str,
    request_id: Option<&str>,
    response_time: Option<&str>,
    response_truncated: bool,
) -> Vec<Map<String, Value>> {
    extract_xml_blocks(xml, "results")
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            soap_line_item_row_for_scratchpad(
                &result,
                index,
                network_code,
                api_version,
                request_id,
                response_time,
                response_truncated,
            )
        })
        .collect()
}

fn soap_line_item_row_for_scratchpad(
    result: &str,
    index: usize,
    network_code: &str,
    api_version: &str,
    request_id: Option<&str>,
    response_time: Option<&str>,
    response_truncated: bool,
) -> Map<String, Value> {
    let primary_goal = extract_xml_first_block(result, "primaryGoal").unwrap_or_default();
    let delivery_indicator =
        extract_xml_first_block(result, "deliveryIndicator").unwrap_or_default();
    let stats = extract_xml_first_block(result, "stats").unwrap_or_default();
    let creative_placeholders = extract_xml_blocks(result, "creativePlaceholders");
    let target_ad_unit_ids = extract_xml_tag_texts(result, "adUnitId");
    let custom_targeting_key_ids = extract_xml_tag_texts(result, "keyId");
    let custom_targeting_value_ids = extract_xml_tag_texts(result, "valueIds");

    let mut out = Map::new();
    out.insert(
        "network_code".to_string(),
        Value::String(network_code.to_string()),
    );
    out.insert(
        "api_version".to_string(),
        Value::String(api_version.to_string()),
    );
    out.insert("row_index".to_string(), json!(index));
    out.insert(
        "request_id".to_string(),
        option_string_value(request_id.map(str::to_string)),
    );
    out.insert(
        "response_time".to_string(),
        option_string_value(response_time.map(str::to_string)),
    );
    out.insert(
        "response_truncated".to_string(),
        Value::Bool(response_truncated),
    );
    out.insert(
        "order_id".to_string(),
        option_u64_value(extract_xml_tag_text(result, "orderId")),
    );
    out.insert(
        "order_name".to_string(),
        option_string_value(extract_xml_tag_text(result, "orderName")),
    );
    out.insert(
        "line_item_id".to_string(),
        option_u64_value(extract_xml_tag_text(result, "id")),
    );
    out.insert(
        "line_item_name".to_string(),
        option_string_value(extract_xml_tag_text(result, "name")),
    );
    out.insert(
        "status".to_string(),
        option_string_value(extract_xml_tag_text(result, "status")),
    );
    out.insert(
        "reservation_status".to_string(),
        option_string_value(extract_xml_tag_text(result, "reservationStatus")),
    );
    out.insert(
        "line_item_type".to_string(),
        option_string_value(extract_xml_tag_text(result, "lineItemType")),
    );
    out.insert(
        "priority".to_string(),
        option_u64_value(extract_xml_tag_text(result, "priority")),
    );
    out.insert(
        "cost_type".to_string(),
        option_string_value(extract_xml_tag_text(result, "costType")),
    );
    out.insert(
        "delivery_rate_type".to_string(),
        option_string_value(extract_xml_tag_text(result, "deliveryRateType")),
    );
    out.insert(
        "roadblocking_type".to_string(),
        option_string_value(extract_xml_tag_text(result, "roadblockingType")),
    );
    out.insert(
        "start_date_time".to_string(),
        option_string_value(extract_xml_date_time(result, "startDateTime")),
    );
    out.insert(
        "end_date_time".to_string(),
        option_string_value(extract_xml_date_time(result, "endDateTime")),
    );
    out.insert(
        "creative_sizes".to_string(),
        Value::String(creative_sizes_from_placeholders(&creative_placeholders).join(",")),
    );
    out.insert(
        "expected_creative_count".to_string(),
        option_u64_value(
            creative_placeholders
                .first()
                .and_then(|block| extract_xml_tag_text(block, "expectedCreativeCount")),
        ),
    );
    out.insert(
        "impressions_delivered".to_string(),
        option_u64_value(extract_xml_tag_text(&stats, "impressionsDelivered")),
    );
    out.insert(
        "clicks_delivered".to_string(),
        option_u64_value(extract_xml_tag_text(&stats, "clicksDelivered")),
    );
    out.insert(
        "expected_delivery_percentage".to_string(),
        option_f64_value(extract_xml_tag_text(
            &delivery_indicator,
            "expectedDeliveryPercentage",
        )),
    );
    out.insert(
        "actual_delivery_percentage".to_string(),
        option_f64_value(extract_xml_tag_text(
            &delivery_indicator,
            "actualDeliveryPercentage",
        )),
    );
    out.insert(
        "primary_goal_type".to_string(),
        option_string_value(extract_xml_tag_text(&primary_goal, "goalType")),
    );
    out.insert(
        "primary_goal_unit_type".to_string(),
        option_string_value(extract_xml_tag_text(&primary_goal, "unitType")),
    );
    out.insert(
        "primary_goal_units".to_string(),
        option_u64_value(extract_xml_tag_text(&primary_goal, "units")),
    );
    out.insert(
        "is_missing_creatives".to_string(),
        option_bool_value(extract_xml_tag_text(result, "isMissingCreatives")),
    );
    out.insert(
        "is_archived".to_string(),
        option_bool_value(extract_xml_tag_text(result, "isArchived")),
    );
    out.insert(
        "target_ad_unit_ids_json".to_string(),
        Value::String(json!(target_ad_unit_ids).to_string()),
    );
    out.insert(
        "custom_targeting_key_ids_json".to_string(),
        Value::String(json!(custom_targeting_key_ids).to_string()),
    );
    out.insert(
        "custom_targeting_value_ids_json".to_string(),
        Value::String(json!(custom_targeting_value_ids).to_string()),
    );
    out.insert(
        "upstream_xml".to_string(),
        Value::String(result.to_string()),
    );
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
    let suggested_login = if scope == MANAGE_SCOPE {
        "google-ad-manager-mcp auth login --headless --manage-scope --quota-project <PROJECT_ID>"
            .to_string()
    } else if scope == crate::DEFAULT_READONLY_SCOPE {
        "google-ad-manager-mcp auth login --headless --quota-project <PROJECT_ID>".to_string()
    } else {
        format!(
            "google-ad-manager-mcp --scope {scope} auth login --headless --quota-project <PROJECT_ID>"
        )
    };
    let mut steps = vec![
        format!(
            "Run `{suggested_login}` if no credential source is configured, or run `{}`. This helper requests both {GCLOUD_ADC_REQUIRED_SCOPE} and {scope}.",
            setup_plan.headless_login.shell,
        ),
        format!(
            "If Google reports a quota-project problem, run `{}` and enable {AD_MANAGER_PROVIDER_API_SERVICE} on that project.",
            setup_plan.quota_project.shell
        ),
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
    use super::*;

    fn values(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

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

    #[test]
    fn soap_payload_builder_renders_line_items_by_order_id() {
        let built = build_soap_payload_template(
            SoapPayloadTemplate::LineItemsByOrderId,
            &values(json!({ "order_id": "12345" })),
        )
        .expect("payload");

        assert_eq!(built["operation"], "get_line_items_by_statement");
        assert_eq!(
            built["payload_xml"],
            "<filterStatement>\n  <query>WHERE orderId = 12345 ORDER BY id ASC</query>\n</filterStatement>"
        );
        assert_eq!(built["next_tool"], "gam_soap_trafficking_plan");
        assert_eq!(built["mutation_performed"], false);
        assert_eq!(built["upstream_called"], false);
    }

    #[test]
    fn soap_payload_builder_renders_create_lica_with_warning() {
        let built = build_soap_payload_template(
            SoapPayloadTemplate::CreateLica,
            &values(json!({
                "line_item_id": 12345,
                "creative_id": "67890"
            })),
        )
        .expect("payload");

        assert_eq!(built["operation"], "create_line_item_creative_associations");
        assert_eq!(
            built["payload_xml"],
            "<lineItemCreativeAssociations>\n  <lineItemId>12345</lineItemId>\n  <creativeId>67890</creativeId>\n</lineItemCreativeAssociations>"
        );
        assert!(
            built["warnings"]
                .as_array()
                .expect("warnings")
                .iter()
                .any(|warning| warning.as_str().unwrap_or_default().contains("creates"))
        );
    }

    #[test]
    fn soap_payload_builder_renders_delivery_forecast_options() {
        let built = build_soap_payload_template(
            SoapPayloadTemplate::DeliveryForecastByLineItemIds,
            &values(json!({
                "line_item_ids": [12345, "67890"]
            })),
        )
        .expect("payload");

        assert_eq!(built["operation"], "get_delivery_forecast_by_ids");
        assert_eq!(
            built["payload_xml"],
            "<lineItemIds>12345</lineItemIds>\n<lineItemIds>67890</lineItemIds>\n<forecastOptions />"
        );
    }

    #[test]
    fn soap_payload_builder_renders_yield_group_query() {
        let built = build_soap_payload_template(
            SoapPayloadTemplate::YieldGroupsByStatement,
            &values(json!({
                "query": "LIMIT 25"
            })),
        )
        .expect("payload");

        assert_eq!(built["operation"], "get_yield_groups_by_statement");
        assert_eq!(
            built["payload_xml"],
            "<statement>\n  <query>LIMIT 25</query>\n</statement>"
        );
        assert_eq!(built["mutation_performed"], false);
    }

    #[test]
    fn soap_payload_builder_renders_yield_partners_empty_payload() {
        let built =
            build_soap_payload_template(SoapPayloadTemplate::YieldPartners, &values(json!({})))
                .expect("payload");

        assert_eq!(built["operation"], "get_yield_partners");
        assert_eq!(built["payload_xml"], "");
        assert_eq!(built["required_values"], json!([]));
        assert_eq!(built["mutation_performed"], false);
        assert_eq!(built["upstream_called"], false);
    }

    #[test]
    fn yield_group_diagnostics_are_bounded_redacted_and_explicit() {
        let blocked = blocked_yield_group_response(SoapTraffickingApplyResult {
            upstream_status: 503,
            upstream_response_xml: "<Fault>ServerError.SERVER_ERROR</Fault>".to_string(),
            response_truncated: true,
            request_id: Some(format!(
                "google_access_token=opaque-secret {}",
                "A".repeat(PROBE_DIAGNOSTIC_SAMPLE_BYTES + 100)
            )),
            response_time: Some("1".repeat(799) + "€tail"),
            soap_fault: Some(format!(
                "Authorization: Bearer opaque-secret {}",
                "€".repeat(400)
            )),
        });
        assert_eq!(blocked["proof_state"], "blocked");
        assert_eq!(blocked["request_id_truncated"], true);
        assert_eq!(blocked["response_time_truncated"], true);
        assert_eq!(blocked["soap_fault_truncated"], true);
        assert_eq!(blocked["message_truncated"], true);
        assert!(!blocked.to_string().contains("opaque-secret"));

        let complete = summarize_yield_groups(
            "<rval><totalResultSetSize>0</totalResultSetSize></rval>",
            false,
            Some(format!(
                "oauth_client_secret=opaque-secret {}",
                "A".repeat(PROBE_DIAGNOSTIC_SAMPLE_BYTES + 100)
            )),
            Some("1".repeat(799) + "€tail"),
            &probe_targets(&["200"]),
            false,
        );
        assert_eq!(complete["request_id_truncated"], true);
        assert_eq!(complete["response_time_truncated"], true);
        assert!(!complete.to_string().contains("opaque-secret"));
    }

    #[test]
    fn exchange_probe_parses_yield_group_target_matches() {
        let xml = r#"
        <getYieldGroupsByStatementResponse>
          <rval>
            <totalResultSetSize>1</totalResultSetSize>
            <results>
              <yieldGroupId>10</yieldGroupId>
              <yieldGroupName>Open bidding group</yieldGroupName>
              <exchangeStatus>ACTIVE</exchangeStatus>
              <format>DISPLAY</format>
              <environmentType>WEB</environmentType>
              <targeting>
                <inventoryTargeting>
                  <targetedAdUnits>
                    <adUnitId>123</adUnitId>
                  </targetedAdUnits>
                </inventoryTargeting>
              </targeting>
            </results>
          </rval>
        </getYieldGroupsByStatementResponse>
        "#;

        let summary = summarize_yield_groups(
            xml,
            false,
            Some("req".to_string()),
            None,
            &probe_targets(&["123"]),
            false,
        );

        assert_eq!(summary["decision"], "targeted_exposed");
        assert_eq!(summary["proof_state"], "complete");
        assert_eq!(summary["target_ad_unit_matches"][0]["yield_group_id"], "10");
        assert_eq!(
            summary["targeted_exposed"][0]["classification"],
            "targeted_exposed"
        );
        assert_eq!(
            summary["targeted_exposed"][0]["targeting_match"]["match_type"],
            "exact"
        );
    }

    #[test]
    fn exchange_probe_treats_exact_excluded_yield_group_match_as_protected() {
        let xml = r#"
        <getYieldGroupsByStatementResponse>
          <rval>
            <totalResultSetSize>1</totalResultSetSize>
            <results>
              <yieldGroupId>755382</yieldGroupId>
              <yieldGroupName>Open bidding group</yieldGroupName>
              <exchangeStatus>ACTIVE</exchangeStatus>
              <format>DISPLAY</format>
              <environmentType>WEB</environmentType>
              <targeting>
                <inventoryTargeting>
                  <targetedAdUnits>
                    <adUnitId>303152</adUnitId>
                    <includeDescendants>true</includeDescendants>
                  </targetedAdUnits>
                  <excludedAdUnits>
                    <adUnitId>987654</adUnitId>
                    <includeDescendants>true</includeDescendants>
                  </excludedAdUnits>
                </inventoryTargeting>
              </targeting>
            </results>
          </rval>
        </getYieldGroupsByStatementResponse>
        "#;

        let summary = summarize_yield_groups(
            xml,
            false,
            Some("req".to_string()),
            None,
            &probe_targets(&["987654"]),
            false,
        );

        assert_eq!(summary["decision"], "targeted_and_excluded");
        assert_eq!(summary["proof_state"], "complete");
        assert_eq!(summary["targeted_exposed"], json!([]));
        assert_eq!(
            summary["targeted_and_excluded"][0]["classification"],
            "targeted_and_excluded"
        );
        assert_eq!(
            summary["targeted_and_excluded"][0]["exclusion_match"]["match_type"],
            "exact"
        );
        assert_eq!(
            summary["targeted_and_excluded"][0]["targeting_match"]["match_type"],
            "broad_descendant_target_unresolved_hierarchy"
        );
        assert_eq!(
            summary["target_ad_unit_matches"][0]["targeted_and_excluded_ad_unit_ids"],
            json!(["987654"])
        );
    }

    #[test]
    fn exchange_probe_uses_known_ancestor_targeting_and_exact_exclusion() {
        let xml = r#"
        <getYieldGroupsByStatementResponse>
          <rval>
            <totalResultSetSize>1</totalResultSetSize>
            <results>
              <yieldGroupId>755382</yieldGroupId>
              <yieldGroupName>Open bidding group</yieldGroupName>
              <exchangeStatus>ACTIVE</exchangeStatus>
              <targeting>
                <inventoryTargeting>
                  <targetedAdUnits>
                    <adUnitId>303152</adUnitId>
                    <includeDescendants>true</includeDescendants>
                  </targetedAdUnits>
                  <excludedAdUnits>
                    <adUnitId>987654</adUnitId>
                    <includeDescendants>false</includeDescendants>
                  </excludedAdUnits>
                </inventoryTargeting>
              </targeting>
            </results>
          </rval>
        </getYieldGroupsByStatementResponse>
        "#;

        let summary = summarize_yield_groups(
            xml,
            false,
            None,
            None,
            &[probe_target_with_ancestors("987654", &["303152"])],
            false,
        );

        assert_eq!(summary["decision"], "targeted_and_excluded");
        assert_eq!(
            summary["targeted_and_excluded"][0]["targeting_match"]["match_type"],
            "ancestor_descendant"
        );
        assert_eq!(
            summary["targeted_and_excluded"][0]["exclusion_match"]["include_descendants"],
            false
        );
    }

    #[test]
    fn exchange_probe_marks_known_ancestor_target_without_exclusion_as_exposed() {
        let xml = r#"
        <getYieldGroupsByStatementResponse>
          <rval>
            <totalResultSetSize>1</totalResultSetSize>
            <results>
              <yieldGroupId>755382</yieldGroupId>
              <exchangeStatus>ACTIVE</exchangeStatus>
              <targeting>
                <inventoryTargeting>
                  <targetedAdUnits>
                    <adUnitId>303152</adUnitId>
                    <includeDescendants>true</includeDescendants>
                  </targetedAdUnits>
                </inventoryTargeting>
              </targeting>
            </results>
          </rval>
        </getYieldGroupsByStatementResponse>
        "#;

        let summary = summarize_yield_groups(
            xml,
            false,
            None,
            None,
            &[probe_target_with_ancestors("987654", &["303152"])],
            false,
        );

        assert_eq!(summary["decision"], "targeted_exposed");
        assert_eq!(
            summary["targeted_exposed"][0]["targeting_match"]["match_type"],
            "ancestor_descendant"
        );
    }

    #[test]
    fn yield_group_exclusion_update_adds_descendant_safe_exclusions_and_repairs_self_only_entries()
    {
        let update = build_yield_group_exclusion_update(
            sample_yield_group_xml(),
            "10",
            &["200".to_string(), "201".to_string(), "202".to_string()],
        )
        .expect("update");

        assert!(update.already_excluded_ad_unit_ids.is_empty());
        assert_eq!(update.updated_excluded_ad_unit_ids, vec!["200"]);
        assert_eq!(update.added_excluded_ad_unit_ids, vec!["201", "202"]);
        assert!(update.payload_xml.contains("<yieldGroups>"));
        assert!(
            update
                .payload_xml
                .contains("<yieldGroupId>10</yieldGroupId>")
        );
        assert!(update.payload_xml.contains(
            "<targetedAdUnits><adUnitId>100</adUnitId><includeDescendants>true</includeDescendants></targetedAdUnits>"
        ));
        assert_eq!(
            update
                .payload_xml
                .matches("<adUnitId>200</adUnitId>")
                .count(),
            1
        );
        for id in ["200", "201", "202"] {
            assert!(update.payload_xml.contains(&format!(
                "<adUnitId>{id}</adUnitId>\n      <includeDescendants>true</includeDescendants>"
            )));
        }
        assert_eq!(
            update
                .payload_xml
                .matches("<includeDescendants>false</includeDescendants>")
                .count(),
            0
        );
        assert_eq!(
            update
                .payload_xml
                .matches("<includeDescendants>true</includeDescendants>")
                .count(),
            4
        );
        assert!(
            update
                .payload_xml
                .contains("<targetedPlacementIds>300</targetedPlacementIds>")
        );
    }

    #[test]
    fn yield_group_exclusion_update_is_noop_when_already_excluded() {
        let xml = sample_yield_group_xml().replace(
            "<includeDescendants>false</includeDescendants>",
            "<includeDescendants>true</includeDescendants>",
        );
        let update =
            build_yield_group_exclusion_update(&xml, "10", &["200".to_string()]).expect("update");

        assert_eq!(update.already_excluded_ad_unit_ids, vec!["200"]);
        assert!(update.added_excluded_ad_unit_ids.is_empty());
        assert!(update.updated_excluded_ad_unit_ids.is_empty());
        assert_eq!(
            update
                .payload_xml
                .matches("<adUnitId>200</adUnitId>")
                .count(),
            1
        );
    }

    #[test]
    fn yield_group_exclusion_update_rejects_direct_target_conflict() {
        let err = build_yield_group_exclusion_update(
            sample_yield_group_xml(),
            "10",
            &["100".to_string()],
        )
        .expect_err("target and exclude conflict should fail");

        assert!(matches!(
            err,
            AdManagerError::InvalidInput {
                field: "excluded_ad_unit_ids",
                ..
            }
        ));
    }

    #[test]
    fn yield_group_exclusion_update_blocks_missing_targeting() {
        let err = build_yield_group_exclusion_update(
            "<yieldGroupId>10</yieldGroupId><yieldGroupName>Group</yieldGroupName>",
            "10",
            &["201".to_string()],
        )
        .expect_err("missing targeting should fail closed");

        assert!(matches!(
            err,
            AdManagerError::InvalidInput {
                field: "yield_group_readback",
                ..
            }
        ));
    }

    #[test]
    fn yield_group_readback_requires_exact_matching_result() {
        let xml = r#"
        <getYieldGroupsByStatementResponse>
          <rval>
            <totalResultSetSize>1</totalResultSetSize>
            <results><yieldGroupId>11</yieldGroupId></results>
          </rval>
        </getYieldGroupsByStatementResponse>
        "#;
        let err = exact_yield_group_from_readback(xml, "10")
            .expect_err("missing exact yield group should fail closed");

        assert!(matches!(
            err,
            AdManagerError::UpstreamApi { status: 404, .. }
        ));
    }

    #[test]
    fn yield_group_exclusion_identifiers_bind_current_readback() {
        let request = YieldGroupExclusionRequestArgs {
            network_code: "1234567".to_string(),
            yield_group_id: "10".to_string(),
            excluded_ad_unit_ids: vec!["201".to_string()],
            api_version: None,
            include_payload_xml: false,
            reason: "prevent broad yield eligibility for exact ad unit".to_string(),
            expected_impact: Some("only broad yield group eligibility changes".to_string()),
            rollback_note: Some("remove the added excludedAdUnits entry".to_string()),
            idempotency_key: None,
        };
        let draft_a = sample_yield_group_exclusion_draft("hash-a");
        let draft_b = sample_yield_group_exclusion_draft("hash-b");

        let (_, token_a, _) =
            guarded_yield_group_exclusion_identifiers(&request, &draft_a).expect("token a");
        let (_, token_b, _) =
            guarded_yield_group_exclusion_identifiers(&request, &draft_b).expect("token b");

        assert_ne!(token_a, token_b);
    }

    #[test]
    fn yield_group_exclusion_readback_does_not_prove_self_only_exclusions() {
        let mut draft = sample_yield_group_exclusion_draft("hash-a");

        draft.current_excluded_ad_units = vec![AdUnitTargetingValue {
            ad_unit_id: "201".to_string(),
            include_descendants: Some(false),
        }];
        assert!(!draft.all_requested_ids_currently_excluded());

        draft.current_excluded_ad_units = vec![AdUnitTargetingValue {
            ad_unit_id: "201".to_string(),
            include_descendants: Some(true),
        }];
        assert!(draft.all_requested_ids_currently_excluded());
    }

    #[test]
    fn yield_group_exclusion_apply_context_requires_operator_impact() {
        let request = YieldGroupExclusionRequestArgs {
            network_code: "1234567".to_string(),
            yield_group_id: "10".to_string(),
            excluded_ad_unit_ids: vec!["201".to_string()],
            api_version: None,
            include_payload_xml: false,
            reason: "prevent broad yield eligibility for exact ad unit".to_string(),
            expected_impact: None,
            rollback_note: Some("remove the added excludedAdUnits entry".to_string()),
            idempotency_key: None,
        };

        let err = validate_yield_group_exclusion_apply_context(&request)
            .expect_err("missing expected impact should fail");
        assert!(matches!(
            err,
            AdManagerError::InvalidInput {
                field: "expected_impact",
                ..
            }
        ));
    }

    #[test]
    fn yield_group_exclusion_rejects_duplicate_requested_ids() {
        let err = validated_excluded_ad_unit_ids(&["201".to_string(), "201".to_string()])
            .expect_err("duplicate ids should fail");
        assert!(matches!(
            err,
            AdManagerError::InvalidInput {
                field: "excluded_ad_unit_ids",
                ..
            }
        ));
    }

    #[test]
    fn exchange_probe_marks_truncated_yield_group_response_as_sample_only() {
        let xml = r#"<rval><totalResultSetSize>10</totalResultSetSize><results><id>1</id></results></rval>"#;
        let summary =
            summarize_yield_groups(xml, false, None, None, &probe_targets(&["999"]), false);

        assert_eq!(summary["decision"], "sample_only");
        assert_eq!(summary["proof_state"], "sample_only");
    }

    #[test]
    fn soap_line_item_rows_parse_delivery_fields_for_scratchpad() {
        let xml = r#"
        <getLineItemsByStatementResponse>
          <rval>
            <totalResultSetSize>1</totalResultSetSize>
            <results>
              <orderId>4102710161</orderId>
              <id>7360951637</id>
              <name>Celonis Right Skin</name>
              <orderName>Celonis June/July 2026</orderName>
              <startDateTime>
                <date><year>2026</year><month>7</month><day>3</day></date>
                <hour>20</hour><minute>23</minute><second>0</second>
                <timeZoneId>Australia/Sydney</timeZoneId>
              </startDateTime>
              <endDateTime>
                <date><year>2026</year><month>7</month><day>23</day></date>
                <hour>23</hour><minute>59</minute><second>0</second>
                <timeZoneId>Australia/Sydney</timeZoneId>
              </endDateTime>
              <deliveryRateType>EVENLY</deliveryRateType>
              <roadblockingType>ONE_OR_MORE</roadblockingType>
              <lineItemType>SPONSORSHIP</lineItemType>
              <priority>4</priority>
              <costType>CPD</costType>
              <creativePlaceholders>
                <size><width>160</width><height>600</height><isAspectRatio>false</isAspectRatio></size>
                <expectedCreativeCount>1</expectedCreativeCount>
              </creativePlaceholders>
              <stats>
                <impressionsDelivered>940</impressionsDelivered>
                <clicksDelivered>1</clicksDelivered>
              </stats>
              <deliveryIndicator>
                <expectedDeliveryPercentage>46.835</expectedDeliveryPercentage>
                <actualDeliveryPercentage>6.94</actualDeliveryPercentage>
              </deliveryIndicator>
              <status>DELIVERING</status>
              <reservationStatus>RESERVED</reservationStatus>
              <isArchived>false</isArchived>
              <isMissingCreatives>false</isMissingCreatives>
              <primaryGoal>
                <goalType>DAILY</goalType>
                <unitType>IMPRESSIONS</unitType>
                <units>100</units>
              </primaryGoal>
              <targeting>
                <inventoryTargeting>
                  <targetedAdUnits><adUnitId>182784632</adUnitId><includeDescendants>true</includeDescendants></targetedAdUnits>
                </inventoryTargeting>
                <customTargeting>
                  <children><keyId>20229275</keyId><valueIds>453440212481</valueIds></children>
                </customTargeting>
              </targeting>
            </results>
          </rval>
        </getLineItemsByStatementResponse>
        "#;

        let rows = soap_line_item_rows_for_scratchpad(
            xml,
            "1015422",
            "v202605",
            Some("request-1"),
            Some("591"),
            false,
        );

        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row["network_code"], "1015422");
        assert_eq!(row["api_version"], "v202605");
        assert_eq!(row["request_id"], "request-1");
        assert_eq!(row["order_id"], 4102710161_u64);
        assert_eq!(row["line_item_id"], 7360951637_u64);
        assert_eq!(row["line_item_name"], "Celonis Right Skin");
        assert_eq!(row["status"], "DELIVERING");
        assert_eq!(row["line_item_type"], "SPONSORSHIP");
        assert_eq!(row["priority"], 4_u64);
        assert_eq!(row["creative_sizes"], "160x600");
        assert_eq!(row["impressions_delivered"], 940_u64);
        assert_eq!(row["clicks_delivered"], 1_u64);
        assert_eq!(row["primary_goal_units"], 100_u64);
        assert_eq!(row["is_missing_creatives"], false);
        assert_eq!(row["target_ad_unit_ids_json"], "[\"182784632\"]");
        assert_eq!(row["custom_targeting_key_ids_json"], "[\"20229275\"]");
        assert_eq!(row["custom_targeting_value_ids_json"], "[\"453440212481\"]");
    }

    #[test]
    fn dependency_probe_parses_placement_membership_from_rest_rows() {
        let payload = json!({
            "placements": [
                {
                    "name": "networks/1015422/placements/300",
                    "displayName": "Section side placement",
                    "status": "ACTIVE",
                    "adUnitAssignments": [
                        {"adUnit": "networks/1015422/adUnits/200"},
                        {"adUnitId": "201"}
                    ]
                }
            ]
        });
        let target = dependency_target("200", &[], &["Section_Page_LS"]);
        let summary = summarize_dependency_placements(&payload, "1015422", 100, &[target]);

        assert_eq!(summary["proof_state"], "complete_for_page");
        assert_eq!(
            summary["target_placement_ids_by_ad_unit_id"]["200"][0],
            "300"
        );
        assert_eq!(
            summary["target_placement_matches_sample"][0]["placement_id"],
            "300"
        );
        assert_eq!(summary["target_placement_match_count"], 1);
    }

    #[test]
    fn nested_rest_identities_are_network_bound_and_incomplete_when_malformed() {
        let ad_units = json!({"adUnits":[{
            "name":"networks/1015422/adUnits/200",
            "adUnitCode":"Section_Page_LS",
            "parentPath":[{"name":"networks/7654321/adUnits/100"}],
            "appliedAdsenseEnabled":false,
            "effectiveAdsenseEnabled":false
        }]});
        let exchange = summarize_probe_ad_unit("1015422", "Section_Page_LS", &ad_units);
        assert_eq!(exchange["proof_state"], "resolved_exact");
        assert_eq!(exchange["ancestor_identity_complete"], false);
        assert_eq!(exchange["proof_complete"], false);
        assert_eq!(exchange["decision"], "partial_api_proof");
        assert_eq!(exchange["ancestor_ad_unit_ids"], json!([]));
        for parent in [
            json!({"adUnitId":"999","name":"networks/1015422/adUnits/200"}),
            json!({"adUnitId":"200","adUnit":"networks/7654321/adUnits/999"}),
        ] {
            let compound = json!({"adUnits":[{
                "name":"networks/1015422/adUnits/200",
                "adUnitCode":"Section_Page_LS",
                "parentPath":[parent],
                "appliedAdsenseEnabled":false,
                "effectiveAdsenseEnabled":false
            }]});
            let summary = summarize_probe_ad_unit("1015422", "Section_Page_LS", &compound);
            assert_eq!(summary["ancestor_identity_complete"], false);
            assert_eq!(summary["ancestor_ad_unit_ids"], json!([]));
            assert_eq!(summary["decision"], "partial_api_proof");
        }

        let dependency = summarize_dependency_ad_unit_code("1015422", "Section_Page_LS", &ad_units);
        assert_eq!(dependency["proof_state"], "resolved_exact");
        assert_eq!(dependency["ancestor_identity_complete"], false);
        let target = dependency_target_from_ad_unit_summary(&dependency, "1015422")
            .expect("the exact primary target remains usable for partial proof");
        assert!(target.ancestor_ad_unit_ids.is_empty());
        assert!(!target.proof_notes.is_empty());

        let placements = json!({"placements":[
            {
                "name":"networks/1015422/placements/300",
                "adUnitAssignments":[{"adUnit":"networks/7654321/adUnits/200"}]
            },
            {
                "name":"networks/1015422/placements/301",
                "adUnitAssignments":[{"adUnit":"networks/1015422/adUnits/0200"}]
            },
            {
                "name":"networks/7654321/placements/302",
                "adUnitAssignments":[{"adUnit":"networks/1015422/adUnits/200"}]
            },
            {
                "name":"networks/1015422/placements/303",
                "adUnitAssignments":[
                    {"adUnit":"networks/7654321/adUnits/999"},
                    {"adUnit":"networks/1015422/adUnits/200"}
                ]
            },
            {
                "name":"networks/1015422/placements/304",
                "adUnitAssignments":[
                    {"adUnitId":"999","name":"networks/1015422/adUnits/200"}
                ]
            },
            {
                "name":"networks/1015422/placements/305",
                "adUnitAssignments":[
                    {"adUnitId":"200","adUnit":"networks/7654321/adUnits/999"}
                ]
            }
        ]});
        let summary = summarize_dependency_placements(&placements, "1015422", 100, &[target]);
        assert_eq!(summary["proof_state"], "sample_or_shape_incomplete");
        assert_eq!(summary["membership_shape_unknown_count"], 6);
        assert_eq!(summary["target_placement_match_count"], 1);
        assert_eq!(
            summary["target_placement_ids_by_ad_unit_id"]["200"],
            json!(["303"])
        );

        let issue = dependency_target_resolution_issue("Section_Page_LS", &dependency)
            .expect("invalid ancestor identities must remain a resolution issue");
        let response = finalize_dependency_probe_response(
            dependency_probe_response_json(
                "1015422",
                vec![dependency],
                summary,
                json!({"proof_state":"complete","dependency_match_count":0}),
                vec![issue],
                json!({"target_resolution_incomplete":true}),
                "dependencies_found",
            ),
            "1015422",
            "dependencies_found",
        )
        .expect("partial dependency producer finalization");
        assert_eq!(
            response["proof_flags"]["target_resolution_incomplete"],
            true
        );
        assert_eq!(
            response["evidence_receipt_template"]["state"],
            "not_generated"
        );
        assert!(
            response["evidence_receipt_template"]
                .get("result_hash")
                .is_none()
        );
    }

    #[tokio::test]
    async fn dependency_probe_handler_finalizes_a_provider_blocked_normal_path() {
        let settings = crate::Settings {
            service_account_json_path: Some(
                "/definitely-missing/gam-mcp-test-service-account.json".to_string(),
            ),
            ..crate::Settings::default()
        };
        let server = AdManagerServer::new(settings).expect("server");
        let result = server
            .gam_ad_unit_dependency_probe(Parameters(AdUnitDependencyProbeArgs {
                network_code: "001015422".to_string(),
                ad_unit_codes: Vec::new(),
                ad_unit_ids: vec!["200".to_string()],
                api_version: None,
                line_item_page_size: None,
                max_line_items: None,
                placement_page_size: None,
                include_line_item_xml: false,
            }))
            .await
            .expect("handler result");
        let response = result.structured_content.expect("structured response");
        assert_eq!(response["ok"], true);
        assert_eq!(response["data"]["network_code"], "1015422");
        assert_eq!(response["data"]["dependency_decision"], "blocked");
        assert!(
            response["data"]["result_fingerprint"]
                .as_str()
                .is_some_and(|value| value.len() == 16)
        );
        assert_eq!(
            response["data"]["evidence_receipt_template"]["state"],
            "not_generated"
        );
    }

    #[test]
    fn dependency_probe_classifies_line_item_inventory_matches() {
        let target = dependency_target("200", &["100"], &["Section_Page_LS"]);
        let placement_summary = json!({
            "target_placement_ids_by_ad_unit_id": {
                "200": ["300"]
            }
        });
        let xml = r#"
        <results>
          <orderId>4102710161</orderId>
          <id>7360951637</id>
          <name>NEXTGEN Section Side</name>
          <orderName>NEXTGEN</orderName>
          <lineItemType>SPONSORSHIP</lineItemType>
          <priority>4</priority>
          <status>DELIVERING</status>
          <reservationStatus>RESERVED</reservationStatus>
          <isArchived>false</isArchived>
          <targeting>
            <inventoryTargeting>
              <targetedPlacementIds>300</targetedPlacementIds>
            </inventoryTargeting>
            <customTargeting>
              <children><keyId>20229275</keyId><valueIds>453440212481</valueIds></children>
            </customTargeting>
          </targeting>
        </results>
        "#;

        let entry = line_item_dependency_entry(xml, &[target], &placement_summary, false)
            .expect("dependency entry");
        assert_eq!(entry["activity_state"], "delivering");
        assert_eq!(
            entry["target_matches"][0]["classification"],
            "placement_target"
        );
        assert_eq!(
            entry["target_matches"][0]["matched_placement_ids"][0],
            "300"
        );
        assert_eq!(entry["custom_targeting_key_ids"][0], "20229275");
    }

    #[test]
    fn bounded_probe_diagnostics_report_utf8_safe_truncation() {
        let source = "A".repeat(799) + "€tail";
        let (sample, truncated) = bounded_redacted_probe_text(&source);
        assert!(truncated);
        assert_eq!(sample, "A".repeat(799));
        assert!(sample.is_char_boundary(sample.len()));

        let blocked = blocked_probe_surface(
            "placements",
            AdManagerError::AuthBootstrap(format!(
                "access_token = opaque-secret {}",
                "€".repeat(400)
            )),
        );
        assert_eq!(blocked["proof_state"], "blocked");
        assert_eq!(blocked["error_truncated"], true);
        assert_eq!(blocked["hint_truncated"], false);
        assert!(!blocked["error"].to_string().contains("opaque-secret"));
    }

    #[tokio::test]
    async fn line_item_permission_block_bounds_and_redacts_the_current_scope() {
        let settings = crate::Settings {
            scope: format!(
                "access_token is opaque-secret {}",
                "A".repeat(PROBE_DIAGNOSTIC_SAMPLE_BYTES + 100)
            ),
            ..crate::Settings::default()
        };
        let server = AdManagerServer::new(settings).expect("server");
        let target = dependency_target("200", &[], &[]);
        let response = probe_ad_unit_line_item_dependencies(
            &server,
            LineItemDependencyProbeOptions {
                network_code: "1234567",
                api_version: Some("v202605"),
                line_item_page_size: 100,
                max_line_items: 100,
                include_line_item_xml: false,
            },
            &[target],
            &json!({}),
        )
        .await
        .expect("permission block");

        assert_eq!(response["proof_state"], "blocked");
        assert_eq!(response["block_class"], "permission");
        assert_eq!(response["current_scope_truncated"], true);
        assert!(!response["current_scope"].to_string().contains("opaque-secret"));
    }

    #[test]
    fn repeated_success_metadata_is_bounded_counted_and_explicitly_truncated() {
        let options = LineItemDependencyProbeOptions {
            network_code: "1234567",
            api_version: Some("v202605"),
            line_item_page_size: 1,
            max_line_items: 100,
            include_line_item_xml: false,
        };
        let mut state = LineItemDependencyScanState::default();
        for index in 0..55 {
            state.record_successful_page(
                SuccessfulLineItemPage {
                    upstream_response_xml: "<rval><totalResultSetSize>0</totalResultSetSize></rval>",
                    response_truncated: false,
                    request_id: Some(format!(
                        "request-{index} access_token = opaque-secret {}",
                        "A".repeat(900)
                    )),
                    response_time: Some("1".repeat(799) + "€tail"),
                },
                &[],
                &json!({}),
                false,
            );
        }
        let response = state.into_completed_response(&options);

        assert_eq!(response["request_id_count"], 55);
        assert_eq!(response["response_time_count"], 55);
        assert_eq!(response["request_ids_truncated"], true);
        assert_eq!(response["response_times_truncated"], true);
        assert_eq!(response["transport_metadata_sample_limit"], 50);
        assert_eq!(response["request_ids"].as_array().map(Vec::len), Some(50));
        assert_eq!(response["response_times"].as_array().map(Vec::len), Some(50));
        assert!(response["request_ids"].as_array().is_some_and(|values| {
            values.iter().all(|value| {
                value
                    .as_str()
                    .is_some_and(|value| value.len() <= PROBE_DIAGNOSTIC_SAMPLE_BYTES)
            })
        }));
        assert!(!response.to_string().contains("opaque-secret"));
    }

    #[tokio::test]
    async fn dependency_scan_preserves_positive_evidence_when_a_later_soap_page_blocks() {
        let options = LineItemDependencyProbeOptions {
            network_code: "1015422",
            api_version: Some("v202605"),
            line_item_page_size: 1,
            max_line_items: 100,
            include_line_item_xml: false,
        };
        let target = dependency_target("200", &[], &["Section_Page_LS"]);
        let placement_summary = json!({
            "proof_state":"complete_for_page",
            "target_placement_match_count":0,
            "target_placement_ids_by_ad_unit_id":{"200":[]}
        });
        let first_page = r#"
        <rval>
          <totalResultSetSize>2</totalResultSetSize>
          <results>
            <id>1</id>
            <name>Observed dependency</name>
            <status>DELIVERING</status>
            <isArchived>false</isArchived>
            <targeting><inventoryTargeting>
              <targetedAdUnits><adUnitId>200</adUnitId><includeDescendants>false</includeDescendants></targetedAdUnits>
            </inventoryTargeting></targeting>
          </results>
        </rval>
        "#;
        let mut pages = std::collections::VecDeque::<
            Result<SoapTraffickingApplyResult, AdManagerError>,
        >::from([
            Ok(SoapTraffickingApplyResult {
                upstream_status: 200,
                upstream_response_xml: first_page.to_string(),
                response_truncated: false,
                request_id: Some("request-1".to_string()),
                response_time: Some("42".to_string()),
                soap_fault: None,
            }),
            Ok(SoapTraffickingApplyResult {
                upstream_status: 503,
                upstream_response_xml: "<Fault>ServerError.SERVER_ERROR</Fault>".to_string(),
                response_truncated: true,
                request_id: Some("request-2".to_string()),
                response_time: Some("44".to_string()),
                soap_fault: Some(format!(
                    "ServerError.SERVER_ERROR Authorization: Bearer opaque-secret {}",
                    "€".repeat(400)
                )),
            }),
        ]);
        let mut expected_offset = 0;
        let blocked = scan_ad_unit_line_item_dependencies(
            &options,
            std::slice::from_ref(&target),
            &placement_summary,
            |offset, page_limit| {
                assert_eq!(offset, expected_offset);
                assert_eq!(page_limit, 1);
                expected_offset += 1;
                std::future::ready(pages.pop_front().expect("scripted page"))
            },
        )
        .await;

        assert_eq!(blocked["decision"], "dependencies_found");
        assert_eq!(blocked["proof_state"], "blocked");
        assert_eq!(blocked["block_class"], "upstream");
        assert_eq!(blocked["upstream_status"], 503);
        assert_eq!(blocked["request_id"], "request-2");
        assert_eq!(blocked["request_id_truncated"], false);
        assert_eq!(blocked["response_time"], "44");
        assert_eq!(blocked["response_time_truncated"], false);
        assert!(
            blocked["soap_fault"]
                .as_str()
                .is_some_and(|fault| fault.starts_with("ServerError.SERVER_ERROR"))
        );
        assert!(
            blocked["soap_fault"]
                .as_str()
                .is_some_and(|fault| fault.len() <= PROBE_DIAGNOSTIC_SAMPLE_BYTES)
        );
        assert!(!blocked["soap_fault"].to_string().contains("opaque-secret"));
        assert_eq!(blocked["soap_fault_truncated"], true);
        assert_eq!(blocked["message_truncated"], true);
        assert_eq!(blocked["total_result_set_size"], 2);
        assert_eq!(blocked["inspected_results"], 1);
        assert_eq!(blocked["request_ids"], json!(["request-1"]));
        assert_eq!(blocked["request_id_count"], 1);
        assert_eq!(blocked["request_ids_truncated"], false);
        assert_eq!(blocked["response_times"], json!(["42"]));
        assert_eq!(blocked["response_time_count"], 1);
        assert_eq!(blocked["response_times_truncated"], false);
        assert_eq!(blocked["response_truncated"], true);
        assert_eq!(blocked["status_counts"]["DELIVERING"], 1);
        assert_eq!(blocked["dependency_match_count"], 1);
        assert_eq!(blocked["dependency_matches_sample"][0]["line_item_id"], "1");
        let flags = dependency_proof_flags(
            std::slice::from_ref(&target),
            &placement_summary,
            &blocked,
            false,
        );
        assert_eq!(flags["line_items_blocked"], true);
        assert_eq!(flags["line_items_capped_or_truncated"], true);
        assert_eq!(
            dependency_probe_decision(&[], &placement_summary, &blocked),
            "dependencies_found"
        );
        assert_eq!(
            dependency_evidence_state(
                "dependencies_found",
                &json!({"line_items": blocked.clone()})
            ),
            EvidenceState::CompleteBlocked
        );
    }

    #[test]
    fn dependency_scan_without_prior_matches_stays_blocked_and_normal_completion_is_unchanged() {
        let options = LineItemDependencyProbeOptions {
            network_code: "1015422",
            api_version: Some("v202605"),
            line_item_page_size: 1,
            max_line_items: 100,
            include_line_item_xml: false,
        };
        let target = dependency_target("200", &[], &["Section_Page_LS"]);
        let placement_summary = json!({
            "proof_state":"complete_for_page",
            "target_placement_match_count":0,
            "target_placement_ids_by_ad_unit_id":{"200":[]}
        });
        let unrelated_page = r#"
        <rval>
          <totalResultSetSize>1</totalResultSetSize>
          <results>
            <id>2</id>
            <status>PAUSED</status>
            <isArchived>false</isArchived>
            <targeting><inventoryTargeting>
              <targetedAdUnits><adUnitId>999</adUnitId><includeDescendants>false</includeDescendants></targetedAdUnits>
            </inventoryTargeting></targeting>
          </results>
        </rval>
        "#;
        let record_page = |state: &mut LineItemDependencyScanState| {
            state.record_successful_page(
                SuccessfulLineItemPage {
                    upstream_response_xml: unrelated_page,
                    response_truncated: false,
                    request_id: Some("request-2".to_string()),
                    response_time: Some("43".to_string()),
                },
                std::slice::from_ref(&target),
                &placement_summary,
                false,
            )
        };

        let mut blocked_state = LineItemDependencyScanState::default();
        assert_eq!(record_page(&mut blocked_state), 1);
        let auth_error = AdManagerError::AuthBootstrap(format!(
            "google_access_token=opaque-secret {}",
            "€".repeat(PROBE_DIAGNOSTIC_SAMPLE_BYTES)
        ));
        let blocked = blocked_state.into_error_blocked_response(&options, &auth_error);
        assert_eq!(blocked["decision"], "blocked");
        assert_eq!(blocked["proof_state"], "blocked");
        assert_eq!(blocked["block_class"], "permission");
        assert_eq!(blocked["dependency_match_count"], 0);
        assert_eq!(blocked["inspected_results"], 1);
        assert_eq!(blocked["error_truncated"], true);
        assert_eq!(blocked["hint_truncated"], false);
        assert!(blocked["hint"].as_str().is_some_and(|hint| !hint.is_empty()));
        assert!(!blocked["error"].to_string().contains("opaque-secret"));
        assert!(
            blocked["error"]
                .as_str()
                .is_some_and(|error| error.is_char_boundary(error.len()))
        );
        assert_eq!(
            dependency_evidence_state("blocked", &json!({"line_items": blocked.clone()})),
            EvidenceState::BlockedPermission
        );

        let mut complete_state = LineItemDependencyScanState::default();
        assert_eq!(record_page(&mut complete_state), 1);
        let complete = complete_state.into_completed_response(&options);
        assert_eq!(
            complete,
            json!({
                "surface": "line_items",
                "decision": "no_dependencies_observed",
                "proof_state": "complete",
                "total_result_set_size": 1,
                "inspected_results": 1,
                "max_line_items": 100,
                "line_item_page_size": 1,
                "response_truncated": false,
                "missing_total_result_set_size": false,
                "request_ids": ["request-2"],
                "request_id_count": 1,
                "request_ids_truncated": false,
                "response_times": ["43"],
                "response_time_count": 1,
                "response_times_truncated": false,
                "transport_metadata_sample_limit": PROBE_TRANSPORT_METADATA_SAMPLE_LIMIT,
                "status_counts": {"PAUSED": 1},
                "dependency_match_count": 0,
                "dependency_matches_sample": [],
                "dependency_matches_truncated": false,
                "dependency_match_sample_limit": DEPENDENCY_LINE_ITEM_MATCH_SAMPLE_LIMIT,
                "mutation_performed": false,
            })
        );
    }

    #[tokio::test]
    async fn dependency_scan_preserves_full_count_while_bounding_late_transport_block_samples() {
        let options = LineItemDependencyProbeOptions {
            network_code: "1015422",
            api_version: Some("v202605"),
            line_item_page_size: 100,
            max_line_items: 100,
            include_line_item_xml: false,
        };
        let target = dependency_target("200", &[], &["Section_Page_LS"]);
        let placement_summary = json!({
            "proof_state":"complete_for_page",
            "target_placement_match_count":0,
            "target_placement_ids_by_ad_unit_id":{"200":[]}
        });
        let results = (1..=51)
            .map(|id| {
                format!(
                    "<results><id>{id}</id><status>READY</status><isArchived>false</isArchived><targeting><inventoryTargeting><targetedAdUnits><adUnitId>200</adUnitId><includeDescendants>false</includeDescendants></targetedAdUnits></inventoryTargeting></targeting></results>"
                )
            })
            .collect::<String>();
        let page = format!("<rval><totalResultSetSize>52</totalResultSetSize>{results}</rval>");
        let mut pages = std::collections::VecDeque::<
            Result<SoapTraffickingApplyResult, AdManagerError>,
        >::from([
            Ok(SoapTraffickingApplyResult {
                upstream_status: 200,
                upstream_response_xml: page,
                response_truncated: false,
                request_id: Some("request-1".to_string()),
                response_time: Some("42".to_string()),
                soap_fault: None,
            }),
            Err(AdManagerError::UpstreamApi {
                status: 503,
                message: "later page unavailable".to_string(),
            }),
        ]);
        let blocked = scan_ad_unit_line_item_dependencies(
            &options,
            std::slice::from_ref(&target),
            &placement_summary,
            |_, _| std::future::ready(pages.pop_front().expect("scripted page")),
        )
        .await;

        assert_eq!(blocked["decision"], "dependencies_found");
        assert_eq!(blocked["proof_state"], "blocked");
        assert_eq!(blocked["dependency_match_count"], 51);
        assert_eq!(
            blocked["dependency_matches_sample"]
                .as_array()
                .map(Vec::len),
            Some(DEPENDENCY_LINE_ITEM_MATCH_SAMPLE_LIMIT)
        );
        assert_eq!(blocked["dependency_matches_truncated"], true);
    }

    #[test]
    fn dependency_scan_keeps_sample_only_completion_semantics() {
        let options = LineItemDependencyProbeOptions {
            network_code: "1015422",
            api_version: Some("v202605"),
            line_item_page_size: 1,
            max_line_items: 100,
            include_line_item_xml: false,
        };
        let target = dependency_target("200", &[], &["Section_Page_LS"]);
        let placement_summary = json!({
            "proof_state":"complete_for_page",
            "target_placement_match_count":0,
            "target_placement_ids_by_ad_unit_id":{"200":[]}
        });
        let page = r#"
        <rval>
          <totalResultSetSize>2</totalResultSetSize>
          <results>
            <id>1</id>
            <status>PAUSED</status>
            <isArchived>false</isArchived>
            <targeting><inventoryTargeting>
              <targetedAdUnits><adUnitId>999</adUnitId><includeDescendants>false</includeDescendants></targetedAdUnits>
            </inventoryTargeting></targeting>
          </results>
        </rval>
        "#;
        let mut state = LineItemDependencyScanState::default();
        assert_eq!(
            state.record_successful_page(
                SuccessfulLineItemPage {
                    upstream_response_xml: page,
                    response_truncated: false,
                    request_id: Some("request-1".to_string()),
                    response_time: Some("42".to_string()),
                },
                std::slice::from_ref(&target),
                &placement_summary,
                false,
            ),
            1
        );

        let completed = state.into_completed_response(&options);
        assert_eq!(completed["decision"], "no_dependencies_in_sample");
        assert_eq!(completed["proof_state"], "sample_only");
        assert_eq!(completed["total_result_set_size"], 2);
        assert_eq!(completed["inspected_results"], 1);
        assert_eq!(completed["dependency_match_count"], 0);
        assert_eq!(completed["status_counts"]["PAUSED"], 1);
    }

    #[test]
    fn dependency_probe_marks_root_targeting_and_exclusions() {
        let target = dependency_target("200", &["100"], &["Section_Page_LS"]);
        let placement_summary = json!({"target_placement_ids_by_ad_unit_id": {"200": []}});
        let root_xml = r#"
        <results>
          <id>1</id>
          <name>Broad line item</name>
          <status>READY</status>
          <isArchived>false</isArchived>
          <targeting><inventoryTargeting /></targeting>
        </results>
        "#;
        let excluded_xml = r#"
        <results>
          <id>2</id>
          <name>Ancestor target with exact exclusion</name>
          <status>DELIVERING</status>
          <isArchived>false</isArchived>
          <targeting>
            <inventoryTargeting>
              <targetedAdUnits><adUnitId>100</adUnitId><includeDescendants>true</includeDescendants></targetedAdUnits>
              <excludedAdUnits><adUnitId>200</adUnitId><includeDescendants>false</includeDescendants></excludedAdUnits>
            </inventoryTargeting>
          </targeting>
        </results>
        "#;

        let root = line_item_dependency_entry(
            root_xml,
            std::slice::from_ref(&target),
            &placement_summary,
            false,
        )
        .expect("root entry");
        assert_eq!(root["activity_state"], "future_or_ready");
        assert_eq!(
            root["target_matches"][0]["classification"],
            "root_or_network_target"
        );

        let excluded = line_item_dependency_entry(
            excluded_xml,
            std::slice::from_ref(&target),
            &placement_summary,
            false,
        )
        .expect("excluded entry");
        assert_eq!(
            excluded["target_matches"][0]["classification"],
            "targeted_but_excluded"
        );
        assert_eq!(
            excluded["target_matches"][0]["targeting_match"]["match_type"],
            "ancestor_descendant"
        );
        assert_eq!(excluded["target_matches"][0]["dependency_excluded"], true);
    }

    #[test]
    fn dependency_probe_decision_marks_incomplete_absence() {
        let placement_summary = json!({
            "proof_state": "sample_or_shape_incomplete",
            "target_placement_match_count": 0
        });
        let line_item_summary = json!({
            "proof_state": "complete",
            "dependency_match_count": 0
        });

        assert_eq!(
            dependency_probe_decision(&[], &placement_summary, &line_item_summary),
            "incomplete_no_dependencies_observed"
        );
        let flags = dependency_proof_flags(&[], &placement_summary, &line_item_summary, false);
        assert_eq!(flags["placements_capped_or_shape_unknown"], true);
    }

    #[test]
    fn dependency_probe_decision_marks_blocked_when_placements_are_blocked() {
        let mut placement_summary = json!({
            "proof_state": "blocked",
            "target_placement_match_count": 0
        });
        let line_item_summary = json!({
            "proof_state": "complete",
            "dependency_match_count": 0
        });

        assert_eq!(
            dependency_probe_decision(&[], &placement_summary, &line_item_summary),
            "blocked"
        );
        placement_summary["target_placement_match_count"] = json!(1);
        assert_eq!(
            dependency_probe_decision(&[], &placement_summary, &line_item_summary),
            "dependencies_found"
        );
    }

    #[test]
    fn validate_probe_ad_unit_ids_canonicalizes_numeric_strings() {
        let cleaned = validate_probe_ad_unit_ids(&["00123".to_string(), "456".to_string()])
            .expect("ids should be accepted");
        assert_eq!(cleaned, vec!["123".to_string(), "456".to_string()]);
    }

    #[test]
    fn validate_probe_ad_unit_ids_rejects_duplicates_after_canonicalization() {
        let err = validate_probe_ad_unit_ids(&["00123".to_string(), "123".to_string()])
            .expect_err("canonical duplicate should be rejected");
        assert!(matches!(
            err,
            AdManagerError::InvalidInput {
                field: "ad_unit_ids",
                ..
            }
        ));
    }

    #[test]
    fn probe_target_resolution_is_canonical_and_network_bound() {
        let canonical_network = parse_positive_id_string("network_code", " 001234567 ")
            .expect("network code canonicalizes once");
        assert_eq!(canonical_network, "1234567");
        assert_eq!(
            exact_ad_unit_id_from_resource_name(&canonical_network, "networks/1234567/adUnits/200"),
            Some("200".to_string())
        );
        for invalid in [
            "networks/7654321/adUnits/200",
            "networks/1234567/adUnits/0200",
            "networks/1234567/adUnits/not-numeric",
            "networks/1234567/placements/200",
        ] {
            let payload = json!({"adUnits":[{
                "adUnitCode":"Section_Page_LS",
                "name":invalid,
                "appliedAdsenseEnabled":true,
                "effectiveAdsenseEnabled":true
            }]});
            let dependency =
                summarize_dependency_ad_unit_code("1234567", "Section_Page_LS", &payload);
            assert_eq!(dependency["proof_state"], "invalid_resource_name");
            assert!(dependency_target_from_ad_unit_summary(&dependency, "1234567").is_none());

            let exchange = summarize_probe_ad_unit("1234567", "Section_Page_LS", &payload);
            assert_eq!(exchange["decision"], "partial_api_proof");
            assert!(probe_ad_unit_target_from_summary(&exchange, "1234567").is_none());
        }
    }

    #[test]
    fn dependency_probe_response_contract_is_no_mutation_and_not_cleanup_approval() {
        let response = finalize_dependency_probe_response(
            dependency_probe_response_json(
                "1015422",
                vec![json!({
                    "ad_unit_id": "200",
                    "ad_unit_code": "Section_Page_LS",
                    "resource_name": "networks/1015422/adUnits/200",
                    "proof_state": "resolved_exact"
                })],
                json!({
                    "proof_state": "complete_for_page",
                    "target_placement_match_count": 1,
                    "target_placement_matches_sample": [{"placement_id": "300"}]
                }),
                json!({
                    "proof_state": "complete",
                    "dependency_match_count": 1,
                    "dependency_matches_sample": [{"line_item_id": "7360951637"}]
                }),
                Vec::new(),
                json!({"line_items_capped_or_truncated": false}),
                "dependencies_found",
            ),
            "1015422",
            "dependencies_found",
        )
        .expect("dependency producer finalization");

        assert_eq!(response["network_code"], "1015422");
        assert_eq!(response["dependency_decision"], "dependencies_found");
        assert!(
            response["result_fingerprint"]
                .as_str()
                .is_some_and(|value| value.len() == 16)
        );
        assert_eq!(response["mutation_performed"], false);
        assert_eq!(
            response["evidence_receipt_template"]["source"],
            "dependency_probe"
        );
        assert_eq!(
            response["evidence_receipt_template"]["state"],
            "complete_blocked"
        );
        assert_eq!(
            response["evidence_receipt_template"]["result_hash"],
            response["result_fingerprint"]
        );
        assert_eq!(
            response["cleanup_decision"]["safe_to_archive_or_retire"],
            false
        );
        assert!(response["line_items"].get("dependency_matches").is_none());
        assert!(
            response["placements"]
                .get("target_placement_matches")
                .is_none()
        );
    }

    #[test]
    fn exchange_producer_finalization_binds_exact_target_source_state_and_hash() {
        let response = json!({
            "overall_decision": "partial_api_proof",
            "attention_reasons": ["exposed_flag_requires_attention"],
            "partial_reasons": ["manual_ui_review_required"],
            "unsupported_or_unintegrated_surfaces": ["protections"],
            "certainty": {"can_prove_requested_ad_unit_flags": false},
            "ad_units": [{
                "ad_unit_id": "200",
                "resource_name": "networks/1234567/adUnits/200",
                "proof_state": "resolved_exact",
                "proof_complete": false
            }]
        });
        let response = finalize_exchange_probe_response(response, "1234567")
            .expect("exchange producer finalization");

        let receipt = response["evidence_receipt_template"].clone();
        assert_eq!(receipt["network_code"], "1234567");
        assert_eq!(receipt["source"], "exchange_protection_review");
        assert_eq!(receipt["state"], "partial_capped");
        assert_eq!(receipt["target_ad_unit_ids"], json!(["200"]));
        assert_eq!(receipt["result_hash"], response["result_fingerprint"]);
    }

    #[test]
    fn evidence_receipt_is_suppressed_for_unresolved_or_cross_network_targets() {
        for ad_unit in [
            json!({
                "ad_unit_id": "200",
                "resource_name": "networks/1234567/adUnits/200",
                "proof_state": "ambiguous"
            }),
            json!({
                "ad_unit_id": "200",
                "resource_name": "networks/7654321/adUnits/200",
                "proof_state": "resolved_exact"
            }),
            json!({
                "ad_unit_id": "0200",
                "resource_name": "networks/1234567/adUnits/0200",
                "proof_state": "resolved_exact"
            }),
        ] {
            let mut response = json!({
                "result_fingerprint": "0123456789abcdef",
                "ad_units": [ad_unit]
            });
            attach_evidence_receipt_template(
                &mut response,
                "1234567",
                EvidenceSource::DependencyProbe,
                EvidenceState::PartialCapped,
            )
            .expect("suppression is not an internal error");
            assert_eq!(
                response["evidence_receipt_template"]["state"],
                "not_generated"
            );
        }
    }

    #[test]
    fn evidence_receipt_target_scope_enforces_exact_one_to_ten_boundary() {
        let exact_rows = |count: u64| {
            (1..=count)
                .map(|id| {
                    json!({
                        "ad_unit_id":id.to_string(),
                        "resource_name":format!("networks/1234567/adUnits/{id}"),
                        "proof_state":"resolved_exact"
                    })
                })
                .collect::<Vec<_>>()
        };
        let ten = json!({"ad_units":exact_rows(10),"target_resolution_issues":[]});
        assert_eq!(
            evidence_receipt_target_ids(&ten, "1234567")
                .expect("ten exact targets")
                .len(),
            10
        );
        for response in [
            json!({"ad_units":[],"target_resolution_issues":[]}),
            json!({"ad_units":exact_rows(11),"target_resolution_issues":[]}),
            json!({"ad_units":[exact_rows(1)[0].clone(),exact_rows(1)[0].clone()],"target_resolution_issues":[]}),
            json!({"ad_units":exact_rows(1),"target_resolution_issues":["incomplete"]}),
            json!({"ad_units":[{"ad_unit_id":"1","proof_state":"id_only"}],"target_resolution_issues":[]}),
        ] {
            assert!(evidence_receipt_target_ids(&response, "1234567").is_none());
        }
    }

    #[test]
    fn evidence_state_classification_preserves_blocked_and_manual_ui_states() {
        let complete_exchange = json!({
            "ad_units": [{"decision":"clear_on_exposed_flags"}],
            "private_auctions": {"proof_state":"complete_empty"},
            "private_auction_deals": {"proof_state":"complete_empty"},
            "yield_groups": {"proof_state":"complete","decision":"targeted_clear"},
            "rest_discovery": {"proof_state":"complete"},
            "certainty": {
                "can_prove_requested_ad_unit_flags": true,
                "can_prove_private_auction_absence_or_presence": true,
                "can_prove_private_deal_absence_or_presence": true,
                "can_prove_yield_group_targeting": true
            }
        });
        assert_eq!(
            exchange_evidence_state(&complete_exchange),
            EvidenceState::ManualUiProofRequired
        );

        for yield_groups in [
            json!({
                "proof_state":"complete",
                "decision":"targeted_activity_unknown",
                "targeted_activity_unknown":[{"yield_group_id":"1"}]
            }),
            json!({
                "proof_state":"complete",
                "decision":"targeted_and_excluded",
                "targeted_activity_unknown":[{"yield_group_id":"1"}]
            }),
        ] {
            let mut unknown_activity = complete_exchange.clone();
            unknown_activity["yield_groups"] = yield_groups;
            assert_eq!(
                exchange_evidence_state(&unknown_activity),
                EvidenceState::PartialCapped
            );
        }

        let mut permission_blocked = complete_exchange.clone();
        permission_blocked["yield_groups"] =
            json!({"proof_state":"blocked","block_class":"permission"});
        assert_eq!(
            exchange_evidence_state(&permission_blocked),
            EvidenceState::BlockedPermission
        );

        let read_blocked = json!({
            "line_items": {"proof_state":"blocked","block_class":"upstream"}
        });
        assert_eq!(
            dependency_evidence_state("blocked", &read_blocked),
            EvidenceState::BlockedRead
        );
        assert_eq!(
            dependency_evidence_state(
                "blocked",
                &json!({"line_items":{"proof_state":"blocked","block_class":"permission"}}),
            ),
            EvidenceState::BlockedPermission
        );
        assert_eq!(soap_probe_block_class(401, None, ""), "permission");
        assert_eq!(
            soap_probe_block_class(500, Some("PermissionError.PERMISSION_DENIED"), ""),
            "permission"
        );
        assert_eq!(
            soap_probe_block_class(
                500,
                None,
                "<fault>AuthenticationError.NO_NETWORKS_TO_ACCESS</fault>"
            ),
            "permission"
        );
        assert_eq!(
            soap_probe_block_class(500, Some("AuthenticationError.CONNECTION_ERROR"), ""),
            "upstream"
        );
        assert_eq!(
            soap_probe_block_class(500, Some("StatementError.INVALID"), ""),
            "upstream"
        );
    }

    #[test]
    fn dependency_probe_bounds_raw_line_item_xml_sample() {
        let target = dependency_target("200", &[], &["Section_Page_LS"]);
        let placement_summary = json!({"target_placement_ids_by_ad_unit_id": {"200": []}});
        let filler = "x".repeat(DEPENDENCY_LINE_ITEM_XML_SAMPLE_BYTES + 512);
        let xml = format!(
            r#"
        <results>
          <id>3</id>
          <name>{filler}</name>
          <status>DELIVERING</status>
          <isArchived>false</isArchived>
          <targeting><inventoryTargeting /></targeting>
        </results>
        "#
        );

        let entry = line_item_dependency_entry(&xml, &[target], &placement_summary, true)
            .expect("dependency entry");
        assert_eq!(entry["upstream_xml_truncated"], true);
        assert!(
            entry["upstream_xml_sample"]
                .as_str()
                .expect("xml sample")
                .len()
                <= DEPENDENCY_LINE_ITEM_XML_SAMPLE_BYTES
        );
    }

    #[test]
    fn bounded_line_item_query_caps_missing_limit_and_rejects_large_limit() {
        assert_eq!(
            bounded_line_item_pql_query("WHERE status = 'DELIVERING' ORDER BY id DESC")
                .expect("bounded"),
            "WHERE status = 'DELIVERING' ORDER BY id DESC LIMIT 500"
        );
        assert_eq!(
            bounded_line_item_pql_query("LIMIT 25").expect("explicit"),
            "LIMIT 25"
        );
        let err = bounded_line_item_pql_query("WHERE status = 'DELIVERING' LIMIT 5000")
            .expect_err("large limit should fail");
        assert!(matches!(
            err,
            AdManagerError::InvalidInput { field: "query", .. }
        ));
    }

    #[test]
    fn soap_payload_builder_rejects_unsafe_like_fragment() {
        let err = build_soap_payload_template(
            SoapPayloadTemplate::CreativesByAdvertiserName,
            &values(json!({
                "advertiser_id": 12345,
                "name_contains": "NEXTGEN%' OR id > 0 OR name LIKE '%"
            })),
        )
        .expect_err("unsafe fragment should fail");

        assert!(matches!(
            err,
            AdManagerError::InvalidInput {
                field: "name_contains",
                ..
            }
        ));
    }

    #[test]
    fn soap_payload_builder_rejects_unsafe_yield_group_query() {
        let err = build_soap_payload_template(
            SoapPayloadTemplate::YieldGroupsByStatement,
            &values(json!({
                "query": "WHERE id = 1; UPDATE LineItem"
            })),
        )
        .expect_err("unsafe query should fail");

        assert!(matches!(
            err,
            AdManagerError::InvalidInput { field: "query", .. }
        ));
    }

    #[test]
    fn soap_payload_builder_rejects_duplicate_forecast_ids() {
        let err = build_soap_payload_template(
            SoapPayloadTemplate::DeliveryForecastByLineItemIds,
            &values(json!({
                "line_item_ids": [12345, "12345"]
            })),
        )
        .expect_err("duplicate ids should fail");

        assert!(matches!(
            err,
            AdManagerError::InvalidInput {
                field: "line_item_ids",
                ..
            }
        ));
    }

    fn sample_yield_group_xml() -> &'static str {
        r#"
        <yieldGroupId>10</yieldGroupId>
        <yieldGroupName>Open bidding group</yieldGroupName>
        <exchangeStatus>ACTIVE</exchangeStatus>
        <format>BANNER</format>
        <environmentType>WEB</environmentType>
        <targeting>
          <inventoryTargeting>
            <targetedAdUnits><adUnitId>100</adUnitId><includeDescendants>true</includeDescendants></targetedAdUnits>
            <excludedAdUnits><adUnitId>200</adUnitId><includeDescendants>false</includeDescendants></excludedAdUnits>
            <targetedPlacementIds>300</targetedPlacementIds>
          </inventoryTargeting>
        </targeting>
        <adSources><companyId>400</companyId></adSources>
        "#
    }

    fn probe_targets(ids: &[&str]) -> Vec<ProbeAdUnitTarget> {
        ids.iter()
            .map(|id| ProbeAdUnitTarget::exact((*id).to_string()))
            .collect()
    }

    fn probe_target_with_ancestors(id: &str, ancestor_ids: &[&str]) -> ProbeAdUnitTarget {
        ProbeAdUnitTarget {
            ad_unit_id: id.to_string(),
            ancestor_ad_unit_ids: ancestor_ids
                .iter()
                .map(|ancestor_id| (*ancestor_id).to_string())
                .collect(),
        }
    }

    fn dependency_target(id: &str, ancestor_ids: &[&str], codes: &[&str]) -> DependencyProbeTarget {
        DependencyProbeTarget {
            ad_unit_id: id.to_string(),
            ad_unit_codes: codes.iter().map(|code| (*code).to_string()).collect(),
            resource_name: Some(format!("networks/1015422/adUnits/{id}")),
            display_name: Value::String("Section left skin".to_string()),
            status: Value::String("ACTIVE".to_string()),
            ad_unit_sizes: Value::Null,
            ancestor_ad_unit_ids: ancestor_ids
                .iter()
                .map(|ancestor_id| (*ancestor_id).to_string())
                .collect(),
            proof_state: "resolved_exact",
            proof_notes: Vec::new(),
        }
    }

    fn sample_yield_group_exclusion_draft(current_fingerprint: &str) -> YieldGroupExclusionDraft {
        YieldGroupExclusionDraft {
            network_code: "1234567".to_string(),
            api_version: "v202605".to_string(),
            yield_group_id: "10".to_string(),
            yield_group_name: Some("Open bidding group".to_string()),
            exchange_status: Some("ACTIVE".to_string()),
            format: Some("BANNER".to_string()),
            environment_type: Some("WEB".to_string()),
            total_result_set_size: Some(1),
            read_request_id: Some("req".to_string()),
            read_response_time: Some("12".to_string()),
            read_payload_xml: pql_statement_payload("WHERE id = 10"),
            current_yield_group_xml: sample_yield_group_xml().to_string(),
            update_payload_xml: "<yieldGroups />".to_string(),
            current_yield_group_fingerprint: current_fingerprint.to_string(),
            update_payload_fingerprint: "update-hash".to_string(),
            targeted_ad_units: vec![AdUnitTargetingValue {
                ad_unit_id: "100".to_string(),
                include_descendants: Some(true),
            }],
            current_excluded_ad_units: vec![AdUnitTargetingValue {
                ad_unit_id: "200".to_string(),
                include_descendants: Some(false),
            }],
            requested_excluded_ad_unit_ids: vec!["201".to_string()],
            requested_exclusion_include_descendants: true,
            already_excluded_ad_unit_ids: vec![],
            added_excluded_ad_unit_ids: vec!["201".to_string()],
            updated_excluded_ad_unit_ids: vec![],
            noop: false,
        }
    }
}
