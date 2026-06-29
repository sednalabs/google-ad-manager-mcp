use mcp_toolkit_core::guarded_action::GuardedActionPosture;
use mcp_toolkit_core::tool_inventory::{
    ToolCapability, ToolDiscoveryMetadata, ToolInventory, ToolInventoryError,
};

pub(crate) fn build_tool_inventory() -> Result<ToolInventory, ToolInventoryError> {
    ToolInventory::from_capabilities(vec![
        cap(
            "find_tools",
            "discovery",
            "Search Google Ad Manager MCP tools by keyword, group, and read-only status.",
            ["tool_search", "deferred", "discover", "tools", "ad-manager"],
        ),
        cap(
            "gam_get_started",
            "setup",
            "Return the recommended first-run flow, credential options, and starter tools.",
            ["google", "ad-manager", "setup", "first-run", "help"],
        ),
        cap(
            "gam_auth_status",
            "setup",
            "Inspect configured Google Ad Manager auth inputs and optionally verify upstream access.",
            ["google", "ad-manager", "auth", "credentials", "status"],
        ),
        cap(
            "gam_auth_login_command",
            "setup",
            "Return a copyable gcloud ADC login command for Google Ad Manager.",
            ["google", "ad-manager", "gcloud", "adc", "login"],
        ),
        cap(
            "gam_networks_list",
            "networks",
            "List Google Ad Manager networks visible to the authenticated principal.",
            ["google", "ad-manager", "networks", "list", "discovery"],
        ),
        cap(
            "gam_network_catalog_list",
            "catalog",
            "List a curated Google Ad Manager network collection such as ad units, orders, line items, or reports.",
            [
                "google",
                "ad-manager",
                "ad-units",
                "orders",
                "line-items",
                "reports",
            ],
        ),
        cap(
            "gam_report_run",
            "reports",
            "Run a saved Google Ad Manager report and optionally wait for the first result page.",
            ["google", "ad-manager", "reports", "run", "result"],
        ),
        cap(
            "gam_report_result_rows",
            "reports",
            "Fetch rows from a completed Google Ad Manager report result.",
            ["google", "ad-manager", "reports", "rows", "fetch"],
        ),
    ])
}

fn cap<const N: usize>(
    name: &'static str,
    group: &'static str,
    description: &'static str,
    keywords: [&'static str; N],
) -> ToolCapability {
    ToolCapability::new(name)
        .with_group(group)
        .with_read_only(true)
        .with_risk_posture(GuardedActionPosture::read_only())
        .with_discovery(ToolDiscoveryMetadata::new(description, keywords))
}

#[cfg(test)]
mod tests {
    use super::build_tool_inventory;
    use mcp_toolkit_core::tool_inventory::{ToolInventoryPolicy, ToolOperation, ToolSearchFilter};

    #[test]
    fn inventory_search_finds_report_tool() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("run report result".to_string()),
                group: Some("reports".to_string()),
                read_only: Some(true),
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(results.iter().any(|result| result.name == "gam_report_run"));
    }
}
