//! # Google Ad Manager MCP
//!
//! Public stdio MCP server for Google Ad Manager read-only workflows.
//!
//! ## Rationale
//! This server gives MCP clients a compact, public-safe way to inspect Google
//! Ad Manager networks, inventory, delivery catalog data, and saved reports
//! without exposing a generic API proxy.
//!
//! ## Security Boundaries
//! * Upstream Google credentials are used only for Ad Manager API calls.
//! * Tool responses never return raw access tokens, private keys, or whole
//!   credential files.
//! * The initial release is read-only and exposes only allowlisted Ad Manager
//!   Beta REST collections plus report execution/readback.
//!
//! ## References
//! * Google Ad Manager API (Beta) Getting Started
//! * Google Ad Manager API (Beta) Authentication
//! * Google Ad Manager API (Beta) Reports

mod client;
mod config;
mod contract;
mod error;
mod tool_surface;
mod tools;

pub use client::{AdManagerClient, AuthSource, CatalogCollection, CompletedReportRun};
pub use config::{Cli, DEFAULT_READONLY_SCOPE, MANAGE_SCOPE, Settings};
pub use error::AdManagerError;

use mcp_toolkit::rmcp::{
    self, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{ServerCapabilities, ServerInfo, Tool},
    tool_handler,
};
use mcp_toolkit_core::guarded_action::GuardedActionPosture;
use mcp_toolkit_core::tool_inventory::{ToolInventory, ToolInventoryError};
use tool_surface::build_tool_inventory;

pub type McpError = mcp_toolkit::rmcp::ErrorData;

#[derive(Clone)]
pub struct AdManagerServer {
    settings: Settings,
    client: AdManagerClient,
    tool_router: ToolRouter<Self>,
    inventory: ToolInventory,
}

impl AdManagerServer {
    /// Builds the stdio server from validated settings.
    ///
    /// # Errors
    /// Returns an error when tool inventory metadata is internally inconsistent.
    ///
    /// # Security
    /// This constructor does not contact Google or read tokens. Credential
    /// access happens inside tool calls through `AdManagerClient`.
    pub fn new(settings: Settings) -> Result<Self, ToolInventoryError> {
        Ok(Self {
            client: AdManagerClient::from_settings(&settings),
            settings,
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

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn report_posture() -> GuardedActionPosture {
        GuardedActionPosture::read_only()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AdManagerServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Google Ad Manager read-only MCP for networks, inventory, delivery catalog, and saved reports.",
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
        let snapshot = server.tool_schema_snapshot();
        let names = snapshot
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "find_tools",
                "gam_get_started",
                "gam_auth_status",
                "gam_auth_login_command",
                "gam_networks_list",
                "gam_network_catalog_list",
                "gam_report_run",
                "gam_report_result_rows",
            ]
        );

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
