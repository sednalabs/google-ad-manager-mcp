//! Provider-specific tool-discovery orchestration.
//!
//! This module keeps semantic ranking in `mcp-toolkit-rs` and owns only the
//! Google Ad Manager workflow relationships that the generic toolkit cannot
//! infer, such as guided builder/plan/apply dependencies and empty-result recovery.

use std::collections::BTreeSet;

use mcp_toolkit_core::tool_inventory::{
    ToolInventory, ToolInventoryPolicy, ToolOperation, ToolSearchFilter, ToolSearchMatchSummary,
    ToolSearchResult,
};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepresentativeDiscoveryCandidate {
    pub query: &'static str,
    pub expected_tool: &'static str,
    pub group: &'static str,
    pub read_only: bool,
}

pub(crate) const REPRESENTATIVE_DISCOVERY_CANDIDATES: [RepresentativeDiscoveryCandidate; 19] = [
    candidate(
        "set up and authenticate Google Ad Manager",
        "gam_auth_status",
        "setup",
        true,
    ),
    candidate(
        "inspect ad units and placements",
        "gam_network_catalog_list",
        "catalog",
        true,
    ),
    candidate(
        "plan a campaign line item with creatives",
        "gam_soap_trafficking_plan",
        "trafficking",
        true,
    ),
    candidate(
        "pause a line item",
        "gam_soap_trafficking_plan",
        "trafficking",
        true,
    ),
    candidate(
        "resume a line item",
        "gam_soap_trafficking_plan",
        "trafficking",
        true,
    ),
    candidate(
        "archive a line item",
        "gam_soap_trafficking_plan",
        "trafficking",
        true,
    ),
    candidate(
        "deactivate an ad unit",
        "gam_rest_write_plan",
        "trafficking",
        true,
    ),
    candidate(
        "archive an ad unit",
        "gam_rest_write_plan",
        "trafficking",
        true,
    ),
    candidate(
        "start a campaign delivery audit with a saved report",
        "gam_report_run",
        "reports",
        true,
    ),
    candidate(
        "fetch rows from a completed report result",
        "gam_report_result_rows",
        "reports",
        true,
    ),
    candidate(
        "check exchange and yield protection",
        "gam_exchange_protection_probe",
        "catalog",
        true,
    ),
    candidate(
        "assess ad units for retirement",
        "gam_ad_unit_retirement_assessment",
        "catalog",
        true,
    ),
    candidate(
        "open a scratchpad session for delivery analysis",
        "gam_scratchpad_open_session",
        "scratchpad",
        false,
    ),
    candidate(
        "ingest line item delivery into an existing scratchpad session",
        "gam_scratchpad_ingest_soap_line_items",
        "scratchpad",
        false,
    ),
    candidate(
        "discover Google Ad Manager MCP tools by workflow",
        "find_tools",
        "discovery",
        true,
    ),
    candidate(
        "list Google Ad Manager networks visible to the authenticated principal",
        "gam_networks_list",
        "networks",
        true,
    ),
    candidate(
        "apply an allowlisted REST write",
        "gam_rest_write_apply",
        "trafficking",
        false,
    ),
    candidate(
        "apply a SOAP trafficking creative operation",
        "gam_soap_trafficking_apply",
        "trafficking",
        false,
    ),
    candidate(
        "apply descendant-safe yield group exclusions",
        "gam_yield_group_exclusions_apply",
        "trafficking",
        false,
    ),
];

const fn candidate(
    query: &'static str,
    expected_tool: &'static str,
    group: &'static str,
    read_only: bool,
) -> RepresentativeDiscoveryCandidate {
    RepresentativeDiscoveryCandidate {
        query,
        expected_tool,
        group,
        read_only,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkflowDependency {
    tool_name: &'static str,
    before_tool: &'static str,
    required_for_guided_sequence: bool,
    reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkflowCompanion {
    pub tool_name: &'static str,
    pub before_tool: &'static str,
    pub required_for_guided_sequence: bool,
    pub tool_already_selected: bool,
    pub reason: &'static str,
}

// Dependency order is topological; keep the SOAP builder before the SOAP plan.
const WORKFLOW_DEPENDENCIES: [WorkflowDependency; 4] = [
    WorkflowDependency {
        tool_name: "gam_rest_write_plan",
        before_tool: "gam_rest_write_apply",
        required_for_guided_sequence: true,
        reason: "Guided sequence: review the REST plan before apply. Discovery does not prove that plan ran. REST apply independently revalidates its exact request and token and retains runtime, scope, and confirmation gates; configured readback is attempted where available but is not a universal success gate.",
    },
    WorkflowDependency {
        tool_name: "gam_soap_payload_build",
        before_tool: "gam_soap_trafficking_plan",
        required_for_guided_sequence: false,
        reason: "Guided sequence: optionally build a typed SOAP payload before planning. Discovery does not prove the builder ran; independently authored payload_xml remains valid input, and the plan validates and builds its exact no-upstream-call request.",
    },
    WorkflowDependency {
        tool_name: "gam_soap_trafficking_plan",
        before_tool: "gam_soap_trafficking_apply",
        required_for_guided_sequence: true,
        reason: "Guided sequence: review the SOAP trafficking plan before apply. Discovery does not prove that plan ran. Generic SOAP apply independently revalidates its exact request and token and retains runtime, scope, and confirmation gates; follow-up verification is still required.",
    },
    WorkflowDependency {
        tool_name: "gam_yield_group_exclusions_preview",
        before_tool: "gam_yield_group_exclusions_apply",
        required_for_guided_sequence: true,
        reason: "Guided sequence: review descendant-safe yield-group exclusions before apply. Discovery does not prove that preview ran. Typed yield apply independently revalidates its exact request and token, retains runtime, scope, and confirmation gates, and requires descendant-safe post-apply readback.",
    },
];

pub(crate) fn workflow_companions(results: &[ToolSearchResult]) -> Vec<WorkflowCompanion> {
    let selected = results
        .iter()
        .map(|result| result.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut prerequisites = selected.clone();
    loop {
        let mut changed = false;
        for dependency in WORKFLOW_DEPENDENCIES {
            if prerequisites.contains(dependency.before_tool) {
                changed |= prerequisites.insert(dependency.tool_name);
            }
        }
        if !changed {
            break;
        }
    }

    let mut emitted_edges = BTreeSet::new();
    WORKFLOW_DEPENDENCIES
        .into_iter()
        .filter(|dependency| prerequisites.contains(dependency.before_tool))
        .filter(|dependency| emitted_edges.insert((dependency.tool_name, dependency.before_tool)))
        .map(|dependency| WorkflowCompanion {
            tool_name: dependency.tool_name,
            before_tool: dependency.before_tool,
            required_for_guided_sequence: dependency.required_for_guided_sequence,
            tool_already_selected: selected.contains(dependency.tool_name),
            reason: dependency.reason,
        })
        .collect()
}

pub(crate) fn companion_tool_names(companions: &[WorkflowCompanion]) -> Vec<&'static str> {
    let mut injected_names = BTreeSet::new();
    companions
        .iter()
        .filter(|companion| !companion.tool_already_selected)
        .filter(|companion| injected_names.insert(companion.tool_name))
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
                "tool_already_selected": companion.tool_already_selected,
                "required": companion.required_for_guided_sequence,
                "required_for_guided_sequence": companion.required_for_guided_sequence,
                "required_semantics": "guided_sequence_compatibility_alias",
                "server_call_enforced": false,
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
    let example_queries = if fail_closed_reasons.is_empty() {
        validated_recovery_example_queries(inventory, filter)
    } else {
        Vec::new()
    };
    Some(json!({
        "type": "search_recovery",
        "status": "no_matches",
        "fail_closed": !fail_closed_reasons.is_empty(),
        "reason_codes": fail_closed_reasons,
        "active_filter": {
            "group": filter.group.clone(),
            "read_only": filter.read_only,
        },
        "available_groups": available_groups(inventory, filter.read_only),
        "retry": {
            "example_queries_validated_under_active_filter": true,
            "guidance": [
                "Retry with one outcome phrase and one or two Google Ad Manager nouns.",
                "Remove an incorrect group filter or choose one of the available groups.",
                "Keep read_only=true when you want non-mutating tools; do not relax it merely to force a match.",
                "Request include_schema=true only after discovery has narrowed the tool set."
            ],
            "example_queries": example_queries,
        },
    }))
}

fn validated_recovery_example_queries(
    inventory: &ToolInventory,
    filter: &ToolSearchFilter,
) -> Vec<&'static str> {
    REPRESENTATIVE_DISCOVERY_CANDIDATES
        .iter()
        .filter(|candidate| {
            filter
                .group
                .as_deref()
                .is_none_or(|group| candidate.group == group)
        })
        .filter(|candidate| {
            filter
                .read_only
                .is_none_or(|read_only| candidate.read_only == read_only)
        })
        .filter(|candidate| candidate.read_only || filter.read_only == Some(false))
        .filter(|candidate| {
            let ranked = inventory.search_ranked(
                &ToolSearchFilter {
                    query: Some(candidate.query.to_string()),
                    group: filter.group.clone(),
                    read_only: filter.read_only,
                    limit: Some(1),
                },
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            );
            ranked
                .response
                .results
                .first()
                .is_some_and(|result| result.name == candidate.expected_tool)
        })
        .map(|candidate| candidate.query)
        .collect()
}

fn available_groups(inventory: &ToolInventory, read_only: Option<bool>) -> Vec<String> {
    let mut groups = inventory
        .capabilities()
        .into_iter()
        .filter(|capability| {
            inventory.is_allowed(
                capability.name(),
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            )
        })
        .filter(|capability| read_only.is_none_or(|read_only| capability.read_only() == read_only))
        .filter_map(|capability| capability.group().map(str::to_string))
        .collect::<Vec<_>>();
    groups.sort();
    groups.dedup();
    groups
}

#[cfg(test)]
mod tests {
    use super::{
        REPRESENTATIVE_DISCOVERY_CANDIDATES, WorkflowCompanion, available_groups,
        companion_result_records, companion_tool_names, recovery_result_record,
        workflow_companions,
    };
    use crate::tool_surface::build_tool_inventory;
    use mcp_toolkit_core::tool_inventory::{
        ToolCapability, ToolExposure, ToolInventory, ToolInventoryPolicy, ToolOperation,
        ToolSearchFilter, ToolSearchResult,
    };
    use serde_json::Value;
    use std::collections::BTreeSet;

    #[test]
    fn representative_workflows_are_rank_one_without_caller_hints() {
        let inventory = build_tool_inventory().expect("inventory");
        for candidate in REPRESENTATIVE_DISCOVERY_CANDIDATES {
            let ranked = inventory.search_ranked(
                &ToolSearchFilter {
                    query: Some(candidate.query.to_string()),
                    group: None,
                    read_only: None,
                    limit: Some(1),
                },
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            );
            let first =
                ranked.response.results.first().unwrap_or_else(|| {
                    panic!("query '{}' returned no direct tool", candidate.query)
                });
            assert_eq!(
                first.name.as_str(),
                candidate.expected_tool,
                "query '{}' ranked the wrong tool first without caller hints",
                candidate.query
            );
        }
    }

    #[test]
    fn representative_workflows_are_rank_one_under_declared_filters() {
        let inventory = build_tool_inventory().expect("inventory");
        let mut covered_filter_classes = BTreeSet::new();
        for candidate in REPRESENTATIVE_DISCOVERY_CANDIDATES {
            covered_filter_classes.insert((candidate.group, candidate.read_only));
            let ranked = inventory.search_ranked(
                &ToolSearchFilter {
                    query: Some(candidate.query.to_string()),
                    group: Some(candidate.group.to_string()),
                    read_only: Some(candidate.read_only),
                    limit: Some(1),
                },
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            );
            let first =
                ranked.response.results.first().unwrap_or_else(|| {
                    panic!("query '{}' returned no direct tool", candidate.query)
                });
            assert_eq!(
                first.name.as_str(),
                candidate.expected_tool,
                "query '{}' ranked the wrong tool first",
                candidate.query
            );
            assert_eq!(first.group.as_deref(), Some(candidate.group));
            assert_eq!(first.read_only, candidate.read_only);

            let prerequisites =
                companion_tool_names(&workflow_companions(&ranked.response.results));
            match candidate.expected_tool {
                "gam_soap_trafficking_plan" => {
                    assert_eq!(prerequisites, vec!["gam_soap_payload_build"])
                }
                "gam_soap_trafficking_apply" => assert_eq!(
                    prerequisites,
                    vec!["gam_soap_payload_build", "gam_soap_trafficking_plan"]
                ),
                _ => {}
            }
        }
        let current_filter_classes = inventory
            .capabilities()
            .into_iter()
            .filter_map(|capability| {
                capability
                    .group()
                    .map(|group| (group, capability.read_only()))
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(covered_filter_classes, current_filter_classes);
    }

    #[test]
    fn soap_plan_adds_optional_builder_dependency() {
        let results = [result("gam_soap_trafficking_plan")];
        let companions = workflow_companions(&results);
        assert_eq!(
            companion_edges(&companions),
            vec![(
                "gam_soap_payload_build",
                "gam_soap_trafficking_plan",
                false,
                false,
            )]
        );
        let records = companion_result_records(&companions);
        assert_eq!(records[0]["required"], false);
        assert_eq!(records[0]["required_for_guided_sequence"], false);
        assert_eq!(
            records[0]["required_semantics"],
            "guided_sequence_compatibility_alias"
        );
        assert_eq!(records[0]["server_call_enforced"], false);
        assert_eq!(records[0]["tool_already_selected"], false);
    }

    #[test]
    fn soap_apply_adds_builder_then_plan_dependencies() {
        let results = [result("gam_soap_trafficking_apply")];
        let companions = workflow_companions(&results);
        assert_eq!(
            companion_tool_names(&companions),
            vec!["gam_soap_payload_build", "gam_soap_trafficking_plan"]
        );
        assert_eq!(
            companion_edges(&companions),
            vec![
                (
                    "gam_soap_payload_build",
                    "gam_soap_trafficking_plan",
                    false,
                    false,
                ),
                (
                    "gam_soap_trafficking_plan",
                    "gam_soap_trafficking_apply",
                    true,
                    false,
                ),
            ]
        );
        let records = companion_result_records(&companions);
        assert_eq!(records[0]["before_tool"], "gam_soap_trafficking_plan");
        assert_eq!(records[1]["before_tool"], "gam_soap_trafficking_apply");
        assert!(records.iter().all(|record| {
            record["relation"] == "before"
                && record["server_call_enforced"] == false
                && record["mutation_performed"] == false
                && record["required"] == record["required_for_guided_sequence"]
                && record["required_semantics"] == "guided_sequence_compatibility_alias"
        }));
        assert!(
            records[0]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("independently authored payload_xml"))
        );
        assert!(
            records[1]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("follow-up verification"))
        );
    }

    #[test]
    fn rest_and_yield_apply_add_required_guided_predecessors() {
        let results = [
            result("gam_rest_write_apply"),
            result("gam_yield_group_exclusions_apply"),
        ];
        let companions = workflow_companions(&results);
        assert_eq!(
            companion_edges(&companions),
            vec![
                ("gam_rest_write_plan", "gam_rest_write_apply", true, false,),
                (
                    "gam_yield_group_exclusions_preview",
                    "gam_yield_group_exclusions_apply",
                    true,
                    false,
                ),
            ]
        );
        let records = companion_result_records(&companions);
        assert!(records.iter().all(|record| {
            record["required_for_guided_sequence"] == true
                && record["required"] == true
                && record["required_semantics"] == "guided_sequence_compatibility_alias"
                && record["server_call_enforced"] == false
        }));
        assert!(
            records[0]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("not a universal success gate"))
        );
        assert!(
            records[1]["reason"].as_str().is_some_and(
                |reason| reason.contains("requires descendant-safe post-apply readback")
            )
        );
    }

    #[test]
    fn semantic_prerequisites_keep_edges_without_duplicate_injection() {
        let plan_and_apply = [
            result("gam_soap_trafficking_plan"),
            result("gam_soap_trafficking_apply"),
        ];
        let companions = workflow_companions(&plan_and_apply);
        assert_eq!(
            companion_edges(&companions),
            vec![
                (
                    "gam_soap_payload_build",
                    "gam_soap_trafficking_plan",
                    false,
                    false,
                ),
                (
                    "gam_soap_trafficking_plan",
                    "gam_soap_trafficking_apply",
                    true,
                    true,
                ),
            ]
        );
        assert_eq!(
            companion_tool_names(&companions),
            vec!["gam_soap_payload_build"]
        );

        let complete_sequence = [
            result("gam_soap_payload_build"),
            result("gam_soap_trafficking_plan"),
            result("gam_soap_trafficking_apply"),
        ];
        let companions = workflow_companions(&complete_sequence);
        assert_eq!(
            companion_edges(&companions),
            vec![
                (
                    "gam_soap_payload_build",
                    "gam_soap_trafficking_plan",
                    false,
                    true,
                ),
                (
                    "gam_soap_trafficking_plan",
                    "gam_soap_trafficking_apply",
                    true,
                    true,
                ),
            ]
        );
        assert!(companion_tool_names(&companions).is_empty());
    }

    #[test]
    fn read_only_recovery_examples_are_filter_validated_and_non_mutating() {
        let inventory = build_tool_inventory().expect("inventory");
        let filter = ToolSearchFilter {
            query: Some("quasar zeppelin".to_string()),
            group: None,
            read_only: Some(true),
            limit: Some(10),
        };
        let recovery = recovery_for_filter(&inventory, &filter);
        assert_eq!(recovery["status"], "no_matches");
        assert_eq!(recovery["fail_closed"], false);
        assert_eq!(recovery["active_filter"]["group"], serde_json::Value::Null);
        assert_eq!(recovery["active_filter"]["read_only"], true);
        assert_eq!(
            recovery["retry"]["example_queries_validated_under_active_filter"],
            true
        );
        let queries = recovery_queries(&recovery);
        assert!(!queries.is_empty());
        for query in queries {
            let candidate = candidate_for_query(query);
            assert!(candidate.read_only, "mutating recovery query: {query}");
            assert_candidate_ranks_first(&inventory, candidate, None, Some(true));
        }
        assert!(recovery.to_string().contains("do not relax"));
        assert!(
            serde_json::to_vec(&recovery)
                .expect("recovery serializes")
                .len()
                < 4 * 1024
        );
    }

    #[test]
    fn mutating_trafficking_recovery_examples_are_exact_and_executable() {
        let inventory = build_tool_inventory().expect("inventory");
        let filter = ToolSearchFilter {
            query: Some("quasar zeppelin".to_string()),
            group: Some("trafficking".to_string()),
            read_only: Some(false),
            limit: Some(10),
        };
        let recovery = recovery_for_filter(&inventory, &filter);
        assert_eq!(recovery["active_filter"]["group"], "trafficking");
        assert_eq!(recovery["active_filter"]["read_only"], false);
        assert_eq!(
            recovery_queries(&recovery),
            vec![
                "apply an allowlisted REST write",
                "apply a SOAP trafficking creative operation",
                "apply descendant-safe yield group exclusions",
            ]
        );
        for query in recovery_queries(&recovery) {
            let candidate = candidate_for_query(query);
            assert!(!candidate.read_only);
            assert_eq!(candidate.group, "trafficking");
            assert_candidate_ranks_first(&inventory, candidate, Some("trafficking"), Some(false));
        }
        assert!(
            serde_json::to_vec(&recovery)
                .expect("recovery serializes")
                .len()
                < 4 * 1024
        );
    }

    #[test]
    fn stateful_recovery_lists_cold_start_before_continuation() {
        let inventory = build_tool_inventory().expect("inventory");

        let reports = recovery_for_filter(
            &inventory,
            &ToolSearchFilter {
                query: Some("quasar zeppelin".to_string()),
                group: Some("reports".to_string()),
                read_only: Some(true),
                limit: Some(10),
            },
        );
        assert_eq!(
            recovery_queries(&reports),
            vec![
                "start a campaign delivery audit with a saved report",
                "fetch rows from a completed report result",
            ]
        );

        let scratchpad = recovery_for_filter(
            &inventory,
            &ToolSearchFilter {
                query: Some("quasar zeppelin".to_string()),
                group: Some("scratchpad".to_string()),
                read_only: Some(false),
                limit: Some(10),
            },
        );
        assert_eq!(
            recovery_queries(&scratchpad),
            vec![
                "open a scratchpad session for delivery analysis",
                "ingest line item delivery into an existing scratchpad session",
            ]
        );
    }

    #[test]
    fn invalid_group_recovery_has_no_examples_and_valid_alternatives() {
        let inventory = build_tool_inventory().expect("inventory");
        let filter = ToolSearchFilter {
            query: Some("quasar zeppelin".to_string()),
            group: Some("missing-group".to_string()),
            read_only: Some(true),
            limit: Some(10),
        };
        let recovery = recovery_for_filter(&inventory, &filter);
        assert_eq!(recovery["active_filter"]["group"], "missing-group");
        assert_eq!(recovery["active_filter"]["read_only"], true);
        assert!(recovery_queries(&recovery).is_empty());
        assert_eq!(
            recovery["available_groups"],
            serde_json::json!([
                "catalog",
                "discovery",
                "networks",
                "reports",
                "setup",
                "trafficking",
            ])
        );
    }

    #[test]
    fn available_groups_scan_the_complete_list_visible_inventory() {
        let mut capabilities = (0..105)
            .map(|index| {
                ToolCapability::new(format!("visible.{index:03}"))
                    .with_group("catalog")
                    .with_read_only(true)
            })
            .collect::<Vec<_>>();
        capabilities.push(
            ToolCapability::new("zz.late")
                .with_group("late-group")
                .with_read_only(true),
        );
        capabilities.push(
            ToolCapability::new("zz.write")
                .with_group("write-group")
                .with_read_only(false),
        );
        capabilities.push(
            ToolCapability::new("zz.call-only")
                .with_group("call-only-group")
                .with_read_only(true)
                .with_exposure(ToolExposure::CallOnly),
        );
        capabilities.push(
            ToolCapability::new("zz.disabled")
                .with_group("disabled-group")
                .with_read_only(true)
                .with_exposure(ToolExposure::Disabled),
        );
        let inventory = ToolInventory::from_capabilities(capabilities).expect("inventory");

        assert_eq!(
            available_groups(&inventory, Some(true)),
            vec!["catalog", "late-group"]
        );
        assert_eq!(
            available_groups(&inventory, Some(false)),
            vec!["write-group"]
        );
        assert_eq!(
            available_groups(&inventory, None),
            vec!["catalog", "late-group", "write-group"]
        );
    }

    #[test]
    fn ambiguous_recovery_remains_fail_closed() {
        let inventory = build_tool_inventory().expect("inventory");
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
        assert!(recovery_queries(&ambiguous_recovery).is_empty());
    }

    #[test]
    fn recovery_without_explicit_mutation_filter_never_suggests_apply() {
        let inventory = build_tool_inventory().expect("inventory");
        let filter = ToolSearchFilter {
            query: Some("quasar zeppelin".to_string()),
            group: None,
            read_only: None,
            limit: Some(10),
        };
        let recovery = recovery_for_filter(&inventory, &filter);
        for query in recovery_queries(&recovery) {
            assert!(
                candidate_for_query(query).read_only,
                "implicit mutation recovery query: {query}"
            );
        }
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

    fn recovery_for_filter(inventory: &ToolInventory, filter: &ToolSearchFilter) -> Value {
        let ranked =
            inventory.search_ranked(filter, ToolOperation::List, &ToolInventoryPolicy::strict());
        assert_eq!(ranked.match_summary.total_matches, 0);
        recovery_result_record(inventory, filter, &ranked.match_summary).expect("recovery")
    }

    fn recovery_queries(recovery: &Value) -> Vec<&str> {
        recovery["retry"]["example_queries"]
            .as_array()
            .expect("example queries")
            .iter()
            .map(|query| query.as_str().expect("example query"))
            .collect()
    }

    fn candidate_for_query(query: &str) -> super::RepresentativeDiscoveryCandidate {
        REPRESENTATIVE_DISCOVERY_CANDIDATES
            .iter()
            .copied()
            .find(|candidate| candidate.query == query)
            .unwrap_or_else(|| panic!("unknown recovery query: {query}"))
    }

    fn assert_candidate_ranks_first(
        inventory: &ToolInventory,
        candidate: super::RepresentativeDiscoveryCandidate,
        group: Option<&str>,
        read_only: Option<bool>,
    ) {
        let ranked = inventory.search_ranked(
            &ToolSearchFilter {
                query: Some(candidate.query.to_string()),
                group: group.map(str::to_string),
                read_only,
                limit: Some(1),
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert_eq!(
            ranked
                .response
                .results
                .first()
                .map(|result| result.name.as_str()),
            Some(candidate.expected_tool),
            "recovery query did not reproduce rank one: {}",
            candidate.query
        );
    }

    fn companion_edges(
        companions: &[WorkflowCompanion],
    ) -> Vec<(&'static str, &'static str, bool, bool)> {
        companions
            .iter()
            .map(|companion| {
                (
                    companion.tool_name,
                    companion.before_tool,
                    companion.required_for_guided_sequence,
                    companion.tool_already_selected,
                )
            })
            .collect()
    }
}
