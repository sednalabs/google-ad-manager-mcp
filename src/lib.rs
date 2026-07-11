//! # Google Ad Manager MCP
//!
//! Public stdio MCP server for Google Ad Manager workflows.
//!
//! ## Rationale
//! This server gives MCP clients a compact, public-safe way to inspect Google
//! Ad Manager networks, inventory, delivery catalog data, saved reports,
//! guarded REST write previews, and guarded SOAP trafficking calls without
//! exposing a generic API proxy.
//!
//! ## Security Boundaries
//! * Upstream Google credentials are used only for Ad Manager API calls.
//! * Tool responses never return raw access tokens, private keys, or whole
//!   credential files.
//! * Reads are available by default.
//! * Write tools fail closed unless the server runtime mode explicitly enables
//!   apply, and every apply path is bound to a dry-run preview token.
//!
//! ## References
//! * Google Ad Manager API (Beta) Getting Started
//! * Google Ad Manager API (Beta) Authentication
//! * Google Ad Manager API (Beta) Reports

mod ad_unit_retirement;
pub mod auth_ux;
mod client;
mod config;
mod contract;
mod error;
mod evidence;
mod fingerprint;
mod probe_projection;
mod tool_surface;
mod tools;

pub use client::{
    AdManagerClient, AuthSource, CatalogCollection, CompletedReportRun, DEFAULT_SOAP_API_VERSION,
    RestWriteApplyResult, RestWriteOperation, RestWritePlan, RestWriteResource,
    SoapTraffickingApplyResult, SoapTraffickingOperation, SoapTraffickingPlan,
};
pub use config::{Cli, CliCommand, DEFAULT_READONLY_SCOPE, MANAGE_SCOPE, Settings};
pub use error::AdManagerError;

use std::sync::Arc;

use mcp_toolkit::rmcp::{
    self, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{ServerCapabilities, ServerInfo, Tool},
    tool_handler,
};
use mcp_toolkit_core::guarded_action::GuardedActionPosture;
use mcp_toolkit_core::tool_inventory::ToolInventory;
use mcp_toolkit_scratchpad::{
    DuckDbEngine, ScratchpadSessionConfig, ScratchpadSessionManager, SharedScratchpadEngine,
    SharedScratchpadSessionManager,
};
use tool_surface::build_tool_inventory;

pub type McpError = mcp_toolkit::rmcp::ErrorData;

#[derive(Clone)]
pub struct AdManagerServer {
    settings: Settings,
    client: AdManagerClient,
    scratchpad_sessions: SharedScratchpadSessionManager,
    tool_router: ToolRouter<Self>,
    inventory: ToolInventory,
}

impl AdManagerServer {
    /// Builds the stdio server from validated settings.
    ///
    /// # Errors
    /// Returns an error when tool inventory metadata is internally inconsistent
    /// or the scratchpad engine cannot initialize.
    ///
    /// # Security
    /// This constructor does not contact Google or read tokens. Credential
    /// access happens inside tool calls through `AdManagerClient`.
    pub fn new(settings: Settings) -> anyhow::Result<Self> {
        let scratchpad_engine: SharedScratchpadEngine = Arc::new(DuckDbEngine::new()?);
        let scratchpad_config = ScratchpadSessionConfig::new(
            settings.scratchpad_session_ttl,
            settings.scratchpad_max_sessions,
            settings.scratchpad_max_tables_per_session,
            settings.scratchpad_max_rows_per_session,
            settings.scratchpad_max_memory_mb,
        )
        .with_root_dir(settings.scratchpad_root_dir.clone())
        .with_query_timeout(settings.scratchpad_query_timeout)
        .with_max_sql_bytes(settings.scratchpad_max_sql_bytes);
        let scratchpad_sessions = Arc::new(ScratchpadSessionManager::new(
            scratchpad_engine,
            scratchpad_config,
        )?);

        Ok(Self {
            client: AdManagerClient::from_settings(&settings),
            settings,
            scratchpad_sessions,
            tool_router: Self::tool_router_ad_manager(),
            inventory: build_tool_inventory()?,
        })
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    pub fn tool_schema_snapshot(&self) -> Vec<Tool> {
        self.tool_router.list_all()
    }

    pub fn inventory(&self) -> &ToolInventory {
        &self.inventory
    }

    pub fn client(&self) -> &AdManagerClient {
        &self.client
    }

    pub fn scratchpad_sessions(&self) -> &SharedScratchpadSessionManager {
        &self.scratchpad_sessions
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn report_posture() -> GuardedActionPosture {
        GuardedActionPosture::read_only()
    }

    pub fn write_preview_posture() -> GuardedActionPosture {
        GuardedActionPosture::preview()
    }

    pub fn write_apply_posture() -> GuardedActionPosture {
        GuardedActionPosture::guarded_apply()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AdManagerServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Google Ad Manager MCP for networks, inventory, delivery catalog, saved reports, guarded REST write previews, and guarded SOAP trafficking.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{AdManagerServer, Settings};
    use mcp_toolkit_core::tool_inventory::{ToolInventoryPolicy, ToolOperation};

    #[test]
    fn inventory_matches_exported_tool_names() {
        let server = AdManagerServer::new(Settings::default()).expect("server");
        let mut inventory_names = server
            .inventory()
            .capabilities()
            .into_iter()
            .map(|tool| tool.name())
            .map(str::to_string)
            .collect::<Vec<_>>();
        inventory_names.sort();
        let mut exported_names = server.tool_names();
        exported_names.sort();
        assert_eq!(inventory_names, exported_names);

        let policy = ToolInventoryPolicy::strict();
        assert!(
            server
                .inventory()
                .is_allowed("gam_networks_list", ToolOperation::Call, &policy)
        );
        assert!(
            server
                .inventory()
                .is_allowed("gam_report_run", ToolOperation::List, &policy)
        );
    }
}
