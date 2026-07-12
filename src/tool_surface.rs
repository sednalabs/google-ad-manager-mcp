use mcp_toolkit_core::guarded_action::{GuardedActionOperationClass, GuardedActionPosture};
use mcp_toolkit_core::tool_inventory::{
    ToolCapability, ToolDiscoveryMetadata, ToolInventory, ToolInventoryError,
};

pub(crate) fn build_tool_inventory() -> Result<ToolInventory, ToolInventoryError> {
    ToolInventory::from_capabilities(vec![
        cap(
            "find_tools",
            "discovery",
            "Semantically search Google Ad Manager MCP tools with ranked matches, guided dependency edges, and filter-validated empty-result recovery.",
            [
                "tool_search",
                "deferred",
                "discover",
                "semantic",
                "workflow",
                "tools",
                "ad-manager",
            ],
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
            [
                "google",
                "ad-manager",
                "auth",
                "authenticate",
                "credentials",
                "setup",
                "status",
            ],
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
            "List a curated Google Ad Manager network collection such as ad units, orders, line items, placements, private auctions, private auction deals, or reports.",
            [
                "google",
                "ad-manager",
                "ad-units",
                "orders",
                "line-items",
                "private-auctions",
                "private-deals",
                "reports",
                "inspect",
                "inventory",
                "placements",
            ],
        ),
        cap(
            "gam_exchange_protection_probe",
            "catalog",
            "Read-only proof for exact ad-unit exchange/yield/protection exposure, including AdSense flags, private auctions, private deals, yield groups, and unsupported surfaces.",
            [
                "google",
                "ad-manager",
                "exchange",
                "yield",
                "protection",
                "private-auctions",
                "adsense",
                "ad-units",
                "check",
                "audit",
            ],
        ),
        cap(
            "gam_ad_unit_dependency_probe",
            "catalog",
            "Read-only dependency proof for exact ad units across placements and SOAP line-item inventory targeting.",
            [
                "google",
                "ad-manager",
                "ad-units",
                "dependencies",
                "placements",
                "line-items",
                "targeting",
                "cleanup",
            ],
        ),
        cap(
            "gam_ad_unit_retirement_assessment",
            "catalog",
            "Read-only exact-identity, hierarchy, and freshness-bound evidence assessment for one to ten canonical ad-unit ids, ending in a conservative non-authorizing operator-review recommendation.",
            [
                "google",
                "ad-manager",
                "ad-units",
                "retirement",
                "identity",
                "hierarchy",
                "descendants",
                "preflight",
                "cleanup",
                "assess",
            ],
        ),
        cap(
            "gam_report_run",
            "reports",
            "Run a saved Google Ad Manager report for delivery analysis and optionally wait for the first result page.",
            [
                "google",
                "ad-manager",
                "reports",
                "run",
                "result",
                "audit",
                "campaign",
                "delivery",
                "start",
            ],
        )
        .with_risk_posture(report_run_posture()),
        cap(
            "gam_report_operation_poll",
            "reports",
            "Wait on an existing Google Ad Manager report operation without starting another run, then optionally fetch the first result page.",
            [
                "google",
                "ad-manager",
                "reports",
                "operation",
                "poll",
                "wait",
                "existing",
                "continue",
                "resume",
                "completion",
            ],
        )
        .with_risk_posture(report_poll_posture()),
        cap(
            "gam_report_result_rows",
            "reports",
            "Fetch rows from a completed Google Ad Manager report result.",
            [
                "google",
                "ad-manager",
                "reports",
                "rows",
                "fetch",
                "audit",
                "campaign",
                "delivery",
            ],
        ),
        cap(
            "gam_trafficking_tool_matrix",
            "trafficking",
            "Describe REST-supported Ad Manager write tools, SOAP trafficking operations, and remaining ergonomics gaps.",
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
            "Create a dry-run plan and confirmation token for allowlisted Ad Manager REST create, patch, activate, deactivate, archive, or unarchive operations.",
            [
                "google",
                "ad-manager",
                "write",
                "dry-run",
                "plan",
                "preview",
                "inventory",
                "create",
                "patch",
                "activate",
                "deactivate",
                "archive",
                "unarchive",
                "ad-unit",
                "placement",
                "report",
                "label",
            ],
        )
        .with_risk_posture(GuardedActionPosture::preview())
        .with_read_only(true),
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
            "gam_soap_payload_build",
            "trafficking",
            "Build a safe inner SOAP payload_xml fragment for common Ad Manager trafficking templates.",
            [
                "google",
                "ad-manager",
                "soap",
                "payload",
                "xml",
                "template",
                "builder",
            ],
        )
        .with_risk_posture(GuardedActionPosture::no_mutation_proof()),
        cap(
            "gam_soap_trafficking_plan",
            "trafficking",
            "Create a dry-run plan and confirmation token for allowlisted Ad Manager SOAP trafficking actions such as pausing, resuming, or archiving line items, or for forecast operations.",
            [
                "google",
                "ad-manager",
                "soap",
                "trafficking",
                "orders",
                "line-items",
                "forecast",
                "plan",
                "campaign",
                "creative",
                "pause",
                "resume",
                "archive",
                "action",
            ],
        )
        .with_risk_posture(GuardedActionPosture::preview())
        .with_read_only(true),
        cap(
            "gam_soap_trafficking_apply",
            "trafficking",
            "Run an allowlisted Ad Manager SOAP trafficking or forecast operation after scope, runtime, and confirmation gates.",
            [
                "google",
                "ad-manager",
                "soap",
                "trafficking",
                "apply",
                "line-items",
                "creative",
            ],
        )
        .with_read_only(false)
        .with_risk_posture(GuardedActionPosture::guarded_apply()),
        cap(
            "gam_yield_group_exclusions_preview",
            "trafficking",
            "Read one YieldGroupService yield group and preview descendant-safe ad-unit exclusions for open-bidding or mediation eligibility.",
            [
                "google",
                "ad-manager",
                "yield-groups",
                "exclusions",
                "open-bidding",
                "preview",
                "ad-units",
            ],
        )
        .with_risk_posture(GuardedActionPosture::preview())
        .with_read_only(true),
        cap(
            "gam_yield_group_exclusions_apply",
            "trafficking",
            "Apply a previewed YieldGroupService descendant-safe ad-unit exclusion update after gates and readback proof.",
            [
                "google",
                "ad-manager",
                "yield-groups",
                "exclusions",
                "open-bidding",
                "apply",
                "confirmation",
            ],
        )
        .with_read_only(false)
        .with_risk_posture(GuardedActionPosture::guarded_apply()),
        local_state_write_cap(
            "gam_scratchpad_open_session",
            "scratchpad",
            "Open or refresh a bounded local DuckDB scratchpad session for Ad Manager evidence work.",
            ["google", "ad-manager", "scratchpad", "duckdb", "session"],
            false,
        ),
        local_state_write_cap(
            "gam_scratchpad_close_session",
            "scratchpad",
            "Close an Ad Manager scratchpad session and remove its local database.",
            ["google", "ad-manager", "scratchpad", "close", "cleanup"],
            true,
        ),
        local_state_write_cap(
            "gam_scratchpad_list_sessions",
            "scratchpad",
            "List active Ad Manager scratchpad sessions.",
            ["google", "ad-manager", "scratchpad", "sessions", "list"],
            false,
        ),
        local_state_write_cap(
            "gam_scratchpad_list_tables",
            "scratchpad",
            "List tables in an Ad Manager scratchpad session.",
            ["google", "ad-manager", "scratchpad", "tables", "schema"],
            false,
        ),
        local_state_write_cap(
            "gam_scratchpad_drop_table",
            "scratchpad",
            "Drop one table from an Ad Manager scratchpad session.",
            ["google", "ad-manager", "scratchpad", "drop", "table"],
            true,
        ),
        local_state_write_cap(
            "gam_scratchpad_query",
            "scratchpad",
            "Run bounded read-only DuckDB SQL against an Ad Manager scratchpad session.",
            ["google", "ad-manager", "scratchpad", "sql", "query"],
            false,
        ),
        local_state_write_cap(
            "gam_scratchpad_ingest_network_catalog",
            "scratchpad",
            "Fetch one Ad Manager network catalog page and ingest it into a scratchpad table.",
            ["google", "ad-manager", "scratchpad", "ingest", "catalog"],
            false,
        ),
        local_state_write_cap(
            "gam_scratchpad_ingest_report_result_rows",
            "scratchpad",
            "Fetch one Ad Manager report-result page and ingest it into a scratchpad table.",
            ["google", "ad-manager", "scratchpad", "ingest", "reports"],
            false,
        ),
        local_state_write_cap(
            "gam_scratchpad_ingest_soap_line_items",
            "scratchpad",
            "Run a bounded LineItemService SOAP query and ingest parsed delivery rows into a scratchpad table.",
            [
                "google",
                "ad-manager",
                "scratchpad",
                "ingest",
                "soap",
                "line-items",
                "delivery",
                "analyze",
                "analysis",
                "audit",
            ],
            false,
        ),
        local_state_write_cap(
            "gam_scratchpad_export_evidence_bundle",
            "scratchpad",
            "Export a bounded markdown evidence bundle from Ad Manager scratchpad tables.",
            ["google", "ad-manager", "scratchpad", "evidence", "markdown"],
            false,
        ),
    ])
}

pub(crate) const fn report_run_posture() -> GuardedActionPosture {
    GuardedActionPosture {
        operation_class: GuardedActionOperationClass::Mutating,
        requires_runtime_enablement: false,
        writes_enabled_by_default: true,
        post_apply_readback_required: false,
    }
}

pub(crate) const fn report_poll_posture() -> GuardedActionPosture {
    GuardedActionPosture::read_only()
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

fn local_state_write_cap<const N: usize>(
    name: &'static str,
    group: &'static str,
    description: &'static str,
    keywords: [&'static str; N],
    destructive: bool,
) -> ToolCapability {
    cap(name, group, description, keywords)
        .with_read_only(false)
        .with_risk_posture(GuardedActionPosture {
            operation_class: if destructive {
                GuardedActionOperationClass::Destructive
            } else {
                GuardedActionOperationClass::Mutating
            },
            requires_runtime_enablement: false,
            writes_enabled_by_default: true,
            post_apply_readback_required: false,
        })
}

#[cfg(test)]
mod tests {
    use super::build_tool_inventory;
    use mcp_toolkit_core::guarded_action::GuardedActionOperationClass;
    use mcp_toolkit_core::tool_inventory::{ToolInventoryPolicy, ToolOperation, ToolSearchFilter};

    #[test]
    fn inventory_search_finds_report_tool() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("run report result".to_string()),
                group: Some("reports".to_string()),
                read_only: Some(false),
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        let report_run = results
            .iter()
            .find(|result| result.name == "gam_report_run")
            .expect("report run discovery result");
        assert!(!report_run.read_only);
        let posture = report_run.risk_posture.expect("report run risk posture");
        assert_eq!(
            posture.operation_class,
            GuardedActionOperationClass::Mutating
        );
        assert!(!posture.requires_runtime_enablement);
        assert!(posture.writes_enabled_by_default);
        assert!(!posture.post_apply_readback_required);

        let read_only_results = inventory.search(
            &ToolSearchFilter {
                query: Some("run report result".to_string()),
                group: Some("reports".to_string()),
                read_only: Some(true),
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(
            read_only_results
                .iter()
                .all(|result| result.name != "gam_report_run")
        );
    }

    #[test]
    fn inventory_search_finds_scratchpad_evidence_tool() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("scratchpad evidence markdown".to_string()),
                group: Some("scratchpad".to_string()),
                read_only: Some(false),
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
    fn inventory_search_finds_scratchpad_line_item_delivery_tool() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("scratchpad line item delivery soap".to_string()),
                group: Some("scratchpad".to_string()),
                read_only: Some(false),
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(
            results
                .iter()
                .any(|result| result.name == "gam_scratchpad_ingest_soap_line_items")
        );
    }

    #[test]
    fn scratchpad_discovery_metadata_matches_complete_local_state_matrix() {
        let inventory = build_tool_inventory().expect("inventory");
        let read_only = inventory.search(
            &ToolSearchFilter {
                query: None,
                group: Some("scratchpad".to_string()),
                read_only: Some(true),
                limit: Some(100),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(
            read_only.is_empty(),
            "read-only scratchpad tools: {read_only:?}"
        );

        let local_writes = inventory.search(
            &ToolSearchFilter {
                query: None,
                group: Some("scratchpad".to_string()),
                read_only: Some(false),
                limit: Some(100),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        let mut actual = local_writes
            .iter()
            .map(|result| {
                let posture = result.risk_posture.expect("local-state posture");
                (
                    result.name.as_str(),
                    result.read_only,
                    posture.operation_class,
                    posture.requires_runtime_enablement,
                    posture.writes_enabled_by_default,
                    posture.post_apply_readback_required,
                )
            })
            .collect::<Vec<_>>();
        actual.sort_by(|left, right| left.0.cmp(right.0));
        assert_eq!(
            actual,
            vec![
                (
                    "gam_scratchpad_close_session",
                    false,
                    GuardedActionOperationClass::Destructive,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_drop_table",
                    false,
                    GuardedActionOperationClass::Destructive,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_export_evidence_bundle",
                    false,
                    GuardedActionOperationClass::Mutating,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_ingest_network_catalog",
                    false,
                    GuardedActionOperationClass::Mutating,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_ingest_report_result_rows",
                    false,
                    GuardedActionOperationClass::Mutating,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_ingest_soap_line_items",
                    false,
                    GuardedActionOperationClass::Mutating,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_list_sessions",
                    false,
                    GuardedActionOperationClass::Mutating,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_list_tables",
                    false,
                    GuardedActionOperationClass::Mutating,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_open_session",
                    false,
                    GuardedActionOperationClass::Mutating,
                    false,
                    true,
                    false,
                ),
                (
                    "gam_scratchpad_query",
                    false,
                    GuardedActionOperationClass::Mutating,
                    false,
                    true,
                    false,
                ),
            ]
        );
    }

    #[test]
    fn read_only_discovery_includes_non_mutating_plans_and_previews() {
        let inventory = build_tool_inventory().expect("inventory");
        for (query, expected_name) in [
            ("trafficking write dry run", "gam_rest_write_plan"),
            (
                "soap line item forecast trafficking",
                "gam_soap_trafficking_plan",
            ),
            (
                "yield group exclusions open bidding preview",
                "gam_yield_group_exclusions_preview",
            ),
        ] {
            let results = inventory.search(
                &ToolSearchFilter {
                    query: Some(query.to_string()),
                    group: Some("trafficking".to_string()),
                    read_only: Some(true),
                    limit: Some(10),
                },
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            );
            let result = results
                .iter()
                .find(|result| result.name == expected_name)
                .unwrap_or_else(|| {
                    panic!("query '{query}' did not return {expected_name}: {results:?}")
                });
            assert!(result.read_only);
            assert_eq!(
                result
                    .risk_posture
                    .expect("preview risk posture")
                    .operation_class,
                GuardedActionOperationClass::Preview
            );
        }
    }

    #[test]
    fn write_like_trafficking_discovery_returns_only_apply_tools() {
        let inventory = build_tool_inventory().expect("inventory");
        let mut names = inventory
            .search(
                &ToolSearchFilter {
                    query: None,
                    group: Some("trafficking".to_string()),
                    read_only: Some(false),
                    limit: Some(100),
                },
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            )
            .into_iter()
            .map(|result| result.name)
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                "gam_rest_write_apply",
                "gam_soap_trafficking_apply",
                "gam_yield_group_exclusions_apply",
            ]
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

    #[test]
    fn inventory_search_finds_soap_trafficking_tool() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("soap line item forecast trafficking".to_string()),
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
                .any(|result| result.name == "gam_soap_trafficking_plan")
        );
    }

    #[test]
    fn inventory_search_finds_yield_group_exclusion_tools() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("yield group exclusions open bidding".to_string()),
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
                .any(|result| result.name == "gam_yield_group_exclusions_preview")
        );
        assert!(
            results
                .iter()
                .any(|result| result.name == "gam_yield_group_exclusions_apply")
        );
    }

    #[test]
    fn inventory_search_finds_exchange_protection_probe() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("exchange yield protection ad units".to_string()),
                group: Some("catalog".to_string()),
                read_only: Some(true),
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(
            results
                .iter()
                .any(|result| result.name == "gam_exchange_protection_probe")
        );
    }

    #[test]
    fn inventory_search_finds_ad_unit_dependency_probe() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("ad unit dependencies placement line item targeting".to_string()),
                group: Some("catalog".to_string()),
                read_only: Some(true),
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(
            results
                .iter()
                .any(|result| result.name == "gam_ad_unit_dependency_probe")
        );
    }

    #[test]
    fn inventory_search_finds_ad_unit_retirement_assessment() {
        let inventory = build_tool_inventory().expect("inventory");
        let results = inventory.search(
            &ToolSearchFilter {
                query: Some("ad unit retirement identity hierarchy descendants".to_string()),
                group: Some("catalog".to_string()),
                read_only: Some(true),
                limit: Some(10),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(
            results
                .iter()
                .any(|result| result.name == "gam_ad_unit_retirement_assessment")
        );
    }
}
