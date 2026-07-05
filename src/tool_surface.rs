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
        cap(
            "gam_trafficking_tool_matrix",
            "trafficking",
            "Describe REST-supported Ad Manager write tools and SOAP-only trafficking gaps.",
            [
                "google",
                "ad-manager",
                "trafficking",
                "write",
                "matrix",
                "orders",
                "line-items",
            ],
        )
        .with_risk_posture(GuardedActionPosture::no_mutation_proof()),
        cap(
            "gam_rest_write_plan",
            "trafficking",
            "Create a dry-run plan and confirmation token for an allowlisted Ad Manager REST write.",
            [
                "google",
                "ad-manager",
                "write",
                "dry-run",
                "plan",
                "preview",
                "inventory",
            ],
        )
        .with_risk_posture(GuardedActionPosture::preview()),
        cap(
            "gam_rest_write_apply",
            "trafficking",
            "Apply an allowlisted Ad Manager REST write after runtime, scope, and confirmation gates.",
            [
                "google",
                "ad-manager",
                "write",
                "apply",
                "mutation",
                "operator",
                "confirmation",
            ],
        )
        .with_read_only(false)
        .with_risk_posture(GuardedActionPosture::guarded_apply()),
        cap(
            "gam_scratchpad_open_session",
            "scratchpad",
            "Open or refresh a bounded local DuckDB scratchpad session for Ad Manager evidence work.",
            ["google", "ad-manager", "scratchpad", "duckdb", "session"],
        ),
        cap(
            "gam_scratchpad_close_session",
            "scratchpad",
            "Close an Ad Manager scratchpad session and remove its local database.",
            ["google", "ad-manager", "scratchpad", "close", "cleanup"],
        ),
        cap(
            "gam_scratchpad_list_sessions",
            "scratchpad",
            "List active Ad Manager scratchpad sessions.",
            ["google", "ad-manager", "scratchpad", "sessions", "list"],
        ),
        cap(
            "gam_scratchpad_list_tables",
            "scratchpad",
            "List tables in an Ad Manager scratchpad session.",
            ["google", "ad-manager", "scratchpad", "tables", "schema"],
        ),
        cap(
            "gam_scratchpad_drop_table",
            "scratchpad",
            "Drop one table from an Ad Manager scratchpad session.",
            ["google", "ad-manager", "scratchpad", "drop", "table"],
        ),
        cap(
            "gam_scratchpad_query",
            "scratchpad",
            "Run bounded read-only DuckDB SQL against an Ad Manager scratchpad session.",
            ["google", "ad-manager", "scratchpad", "sql", "query"],
        ),
        cap(
            "gam_scratchpad_ingest_network_catalog",
            "scratchpad",
            "Fetch one Ad Manager network catalog page and ingest it into a scratchpad table.",
            ["google", "ad-manager", "scratchpad", "ingest", "catalog"],
        ),
        cap(
            "gam_scratchpad_ingest_report_result_rows",
            "scratchpad",
            "Fetch one Ad Manager report-result page and ingest it into a scratchpad table.",
            ["google", "ad-manager", "scratchpad", "ingest", "reports"],
        ),
        cap(
            "gam_scratchpad_export_evidence_bundle",
            "scratchpad",
            "Export a bounded markdown evidence bundle from Ad Manager scratchpad tables.",
            ["google", "ad-manager", "scratchpad", "evidence", "markdown"],
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

    #[test]
    fn inventory_search_finds_scratchpad_evidence_tool() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("scratchpad evidence markdown".to_string()),
                group: Some("scratchpad".to_string()),
                read_only: Some(true),
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(
            results
                .iter()
                .any(|result| result.name == "gam_scratchpad_export_evidence_bundle")
        );
    }

    #[test]
    fn inventory_search_finds_write_plan_tool() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("trafficking write dry run".to_string()),
                group: Some("trafficking".to_string()),
                read_only: None,
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(
            results
                .iter()
                .any(|result| result.name == "gam_rest_write_plan")
        );
    }
}
