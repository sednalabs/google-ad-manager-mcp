//! Provider-specific tool-discovery orchestration.
//!
//! This module keeps semantic ranking in `mcp-toolkit-rs` and owns only the
//! Google Ad Manager workflow relationships that the generic toolkit cannot
//! infer, such as preview-before-apply companions and empty-result recovery.

use std::collections::BTreeSet;

use mcp_toolkit_core::tool_inventory::{
    ToolInventory, ToolInventoryPolicy, ToolOperation, ToolSearchFilter, ToolSearchMatchSummary,
    ToolSearchResult,
};
use serde_json::{Value, json};

pub(crate) const REPRESENTATIVE_DISCOVERY_QUERIES: [(&str, &str); 7] = [
    (
        "set up and authenticate Google Ad Manager",
        "gam_auth_status",
    ),
    (
        "inspect ad units and placements",
        "gam_network_catalog_list",
    ),
    (
        "plan a campaign line item with creatives",
        "gam_soap_trafficking_plan",
    ),
    (
        "audit campaign delivery and report rows",
        "gam_report_result_rows",
    ),
    (
        "check exchange and yield protection",
        "gam_exchange_protection_probe",
    ),
    (
        "assess ad units for retirement",
        "gam_ad_unit_retirement_assessment",
    ),
    (
        "analyze line item delivery in a scratchpad",
        "gam_scratchpad_ingest_soap_line_items",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkflowCompanion {
    pub tool_name: &'static str,
    pub before_tool: &'static str,
    pub required: bool,
    pub reason: &'static str,
}

const WORKFLOW_COMPANIONS: [WorkflowCompanion; 4] = [
    WorkflowCompanion {
        tool_name: "gam_rest_write_plan",
        before_tool: "gam_rest_write_apply",
        required: true,
        reason: "Create and review the bound REST write plan before any apply call.",
    },
    WorkflowCompanion {
        tool_name: "gam_soap_trafficking_plan",
        before_tool: "gam_soap_trafficking_apply",
        required: true,
        reason: "Create and review the bound SOAP trafficking plan before any apply call.",
    },
    WorkflowCompanion {
        tool_name: "gam_soap_payload_build",
        before_tool: "gam_soap_trafficking_apply",
        required: false,
        reason: "Use the payload builder when a typed SOAP operation needs a payload_xml fragment.",
    },
    WorkflowCompanion {
        tool_name: "gam_yield_group_exclusions_preview",
        before_tool: "gam_yield_group_exclusions_apply",
        required: true,
        reason: "Preview descendant-safe yield-group exclusions before applying them.",
    },
];

pub(crate) fn workflow_companions(results: &[ToolSearchResult]) -> Vec<WorkflowCompanion> {
    let selected = results
        .iter()
        .map(|result| result.name.as_str())
        .collect::<BTreeSet<_>>();
    WORKFLOW_COMPANIONS
        .into_iter()
        .filter(|companion| selected.contains(companion.before_tool))
        .collect()
}

pub(crate) fn companion_tool_names(companions: &[WorkflowCompanion]) -> Vec<&'static str> {
    companions
        .iter()
        .map(|companion| companion.tool_name)
        .collect()
}

pub(crate) fn companion_result_records(companions: &[WorkflowCompanion]) -> Vec<Value> {
    companions
        .iter()
        .map(|companion| {
            json!({
                "type": "workflow_companion",
                "name": companion.tool_name,
                "relation": "before",
                "before_tool": companion.before_tool,
                "required": companion.required,
                "reason": companion.reason,
                "mutation_performed": false,
                "safety": "non_mutating_guidance",
            })
        })
        .collect()
}

pub(crate) fn recovery_result_record(
    inventory: &ToolInventory,
    filter: &ToolSearchFilter,
    summary: &ToolSearchMatchSummary,
) -> Option<Value> {
    if summary.total_matches > 0 {
        return None;
    }
    let fail_closed_reasons = summary
        .truncation_reasons
        .iter()
        .filter(|reason| {
            matches!(
                reason.as_str(),
                "query_input"
                    | "group_input"
                    | "excluded_query_terms"
                    | "query_intent_ambiguous"
                    | "result_metadata"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    Some(json!({
        "type": "search_recovery",
        "status": "no_matches",
        "fail_closed": !fail_closed_reasons.is_empty(),
        "reason_codes": fail_closed_reasons,
        "available_groups": available_groups(inventory, filter.read_only),
        "retry": {
            "guidance": [
                "Retry with one outcome phrase and one or two Google Ad Manager nouns.",
                "Remove an incorrect group filter or choose one of the available groups.",
                "Keep read_only=true when you want non-mutating tools; do not relax it merely to force a match.",
                "Request include_schema=true only after discovery has narrowed the tool set."
            ],
            "example_queries": REPRESENTATIVE_DISCOVERY_QUERIES
                .iter()
                .map(|(query, _)| *query)
                .collect::<Vec<_>>(),
        },
    }))
}

fn available_groups(inventory: &ToolInventory, read_only: Option<bool>) -> Vec<String> {
    let mut groups = inventory
        .search(
            &ToolSearchFilter {
                query: None,
                group: None,
                read_only,
                limit: Some(100),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        )
        .into_iter()
        .filter_map(|result| result.group)
        .collect::<Vec<_>>();
    groups.sort();
    groups.dedup();
    groups
}

#[cfg(test)]
mod tests {
    use super::{
        REPRESENTATIVE_DISCOVERY_QUERIES, companion_result_records, companion_tool_names,
        recovery_result_record, workflow_companions,
    };
    use crate::tool_surface::build_tool_inventory;
    use mcp_toolkit_core::tool_inventory::{
        ToolInventoryPolicy, ToolOperation, ToolSearchFilter, ToolSearchMatchSummary,
        ToolSearchResult,
    };

    #[test]
    fn representative_workflows_have_semantic_results() {
        let inventory = build_tool_inventory().expect("inventory");
        for (query, expected_tool) in REPRESENTATIVE_DISCOVERY_QUERIES {
            let ranked = inventory.search_ranked(
                &ToolSearchFilter {
                    query: Some(query.to_string()),
                    limit: Some(10),
                    ..ToolSearchFilter::default()
                },
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            );
            assert!(
                ranked
                    .response
                    .results
                    .iter()
                    .any(|result| result.name == expected_tool),
                "query '{query}' did not return {expected_tool}: {:?}",
                ranked
                    .response
                    .results
                    .iter()
                    .map(|result| result.name.as_str())
                    .collect::<Vec<_>>()
            );
        }

        let trafficking = inventory.search_ranked(
            &ToolSearchFilter {
                query: Some("traffic a campaign line item creative".to_string()),
                group: Some("trafficking".to_string()),
                limit: Some(20),
                ..ToolSearchFilter::default()
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        let names = trafficking
            .response
            .results
            .iter()
            .map(|result| result.name.as_str())
            .collect::<Vec<_>>();
        let plan_position = names
            .iter()
            .position(|name| *name == "gam_soap_trafficking_plan")
            .expect("SOAP plan result");
        let apply_position = names
            .iter()
            .position(|name| *name == "gam_soap_trafficking_apply")
            .expect("SOAP apply result");
        assert!(plan_position < apply_position, "ranked names: {names:?}");
    }

    #[test]
    fn apply_results_add_only_non_mutating_plan_first_companions() {
        let results = [
            result("gam_rest_write_apply"),
            result("gam_soap_trafficking_apply"),
            result("gam_yield_group_exclusions_apply"),
        ];
        let companions = workflow_companions(&results);
        assert_eq!(
            companion_tool_names(&companions),
            vec![
                "gam_rest_write_plan",
                "gam_soap_trafficking_plan",
                "gam_soap_payload_build",
                "gam_yield_group_exclusions_preview",
            ]
        );
        assert!(
            companions
                .iter()
                .all(|companion| !companion.tool_name.ends_with("_apply"))
        );
        assert!(companion_result_records(&companions).iter().all(|record| {
            record["relation"] == "before"
                && record["before_tool"].as_str().is_some()
                && record["mutation_performed"] == false
        }));
    }

    #[test]
    fn empty_results_return_bounded_recovery_without_relaxing_read_only() {
        let inventory = build_tool_inventory().expect("inventory");
        let filter = ToolSearchFilter {
            query: Some("unknown workflow phrase".to_string()),
            group: Some("missing-group".to_string()),
            read_only: Some(true),
            limit: Some(10),
        };
        let recovery = recovery_result_record(
            &inventory,
            &filter,
            &ToolSearchMatchSummary {
                total_matches: 0,
                returned_count: 0,
                result_limit: 10,
                truncated: false,
                truncation_reasons: Vec::new(),
                normalized_query_terms: vec!["unknown".to_string()],
                excluded_query_terms: Vec::new(),
                ignored_query_terms: Vec::new(),
            },
        )
        .expect("recovery");
        assert_eq!(recovery["status"], "no_matches");
        assert_eq!(recovery["fail_closed"], false);
        assert!(
            recovery["available_groups"]
                .as_array()
                .is_some_and(|groups| !groups.is_empty())
        );
        assert!(recovery.to_string().contains("do not relax"));
        assert!(
            serde_json::to_vec(&recovery)
                .expect("recovery serializes")
                .len()
                < 4 * 1024
        );

        let ambiguous_filter = ToolSearchFilter {
            query: Some("campaign without".to_string()),
            limit: Some(10),
            ..ToolSearchFilter::default()
        };
        let ambiguous = inventory.search_ranked(
            &ambiguous_filter,
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        let ambiguous_recovery =
            recovery_result_record(&inventory, &ambiguous_filter, &ambiguous.match_summary)
                .expect("fail-closed recovery");
        assert_eq!(ambiguous_recovery["fail_closed"], true);
        assert!(
            ambiguous_recovery["reason_codes"].as_array().is_some_and(
                |reasons| reasons.contains(&serde_json::json!("query_intent_ambiguous"))
            )
        );
    }

    fn result(name: &str) -> ToolSearchResult {
        ToolSearchResult {
            name: name.to_string(),
            group: Some("trafficking".to_string()),
            read_only: false,
            description: None,
            keywords: Vec::new(),
            risk_posture: None,
        }
    }
}
