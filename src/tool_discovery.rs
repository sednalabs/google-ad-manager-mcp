//! Provider-specific tool-discovery orchestration.
//!
//! This module keeps semantic ranking in `mcp-toolkit-rs` and owns only the
//! Google Ad Manager workflow relationships that the generic toolkit cannot
//! infer, such as guided builder/plan/apply dependencies and empty-result recovery.

use std::collections::BTreeSet;

use mcp_toolkit_core::guarded_action::GuardedActionOperationClass;
use mcp_toolkit_core::tool_inventory::{
    ToolInventory, ToolInventoryPolicy, ToolOperation, ToolSearchFilter, ToolSearchMatchSummary,
    ToolSearchResult,
};
use serde_json::{Value, json};

const SCRATCHPAD_REST_READ_TOOLS: [&str; 2] = [
    "gam_scratchpad_ingest_network_catalog",
    "gam_scratchpad_ingest_report_result_rows",
];
const SCRATCHPAD_MANAGE_SCOPE_READ_TOOLS: [&str; 1] = ["gam_scratchpad_ingest_soap_line_items"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepresentativeDiscoveryCandidate {
    pub query: &'static str,
    pub expected_tool: &'static str,
    pub group: &'static str,
    pub read_only: bool,
}

pub(crate) const REPRESENTATIVE_DISCOVERY_CANDIDATES: [RepresentativeDiscoveryCandidate; 20] = [
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
        "continue waiting for an existing report operation",
        "gam_report_operation_poll",
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
const WORKFLOW_DEPENDENCIES: [WorkflowDependency; 8] = [
    WorkflowDependency {
        tool_name: "gam_networks_list",
        before_tool: "gam_network_catalog_list",
        required_for_guided_sequence: true,
        reason: "Cold-start catalog sequence: discover the exact network code before listing a network collection. Discovery does not prove that network discovery ran.",
    },
    WorkflowDependency {
        tool_name: "gam_network_catalog_list",
        before_tool: "gam_report_run",
        required_for_guided_sequence: true,
        reason: "Cold-start report sequence: list collection=reports with the exact network code to obtain a saved report id before starting a run. Discovery does not prove that report catalog lookup ran.",
    },
    WorkflowDependency {
        tool_name: "gam_report_run",
        before_tool: "gam_report_operation_poll",
        required_for_guided_sequence: false,
        reason: "Asynchronous report sequence: when no operation_name exists yet, start one report run with wait_for_completion=false, then pass its returned operation_name to gam_report_operation_poll. Do not call gam_report_run when an existing operation_name is already available; the poll tool never starts another report run.",
    },
    WorkflowDependency {
        tool_name: "gam_report_operation_poll",
        before_tool: "gam_report_result_rows",
        required_for_guided_sequence: false,
        reason: "Asynchronous report sequence: poll the existing operation until it returns report_result before fetching rows. This step is optional when gam_report_run already waited for completion and returned report_result.",
    },
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
    let local_state_alternatives = if fail_closed_reasons.is_empty() {
        local_state_alternatives(inventory, filter)
    } else {
        Vec::new()
    };
    let mut guidance = vec![
        "Retry with one outcome phrase and one or two Google Ad Manager nouns.",
        "Remove an incorrect group filter or choose one of the available groups.",
        "Keep read_only=true when you want non-mutating tools; do not relax it merely to force a match.",
        "Request include_schema=true only after discovery has narrowed the tool set.",
    ];
    if !local_state_alternatives.is_empty() {
        guidance.push(
            "For a deliberate scratchpad workflow, review local_state_alternatives before explicitly opting into bounded MCP-local state changes.",
        );
    }
    let recognized_group = recognized_group_filter(inventory, filter.group.as_deref());
    let group_recognized = filter.group.is_none() || recognized_group.is_some();
    let mut recovery = json!({
        "type": "search_recovery",
        "status": "no_matches",
        "fail_closed": !fail_closed_reasons.is_empty(),
        "reason_codes": fail_closed_reasons,
        "active_filter": {
            "group": recognized_group,
            "group_supplied": filter.group.is_some(),
            "group_recognized": group_recognized,
            "read_only": filter.read_only,
        },
        "available_groups": available_groups(inventory, filter.read_only),
        "retry": {
            "example_queries_validated_under_active_filter": true,
            "guidance": guidance,
            "example_queries": example_queries,
        },
    });
    if !local_state_alternatives.is_empty()
        && let Value::Object(recovery) = &mut recovery
    {
        recovery.insert(
            "local_state_alternatives".to_string(),
            Value::Array(local_state_alternatives),
        );
    }
    Some(recovery)
}

fn local_state_alternatives(inventory: &ToolInventory, filter: &ToolSearchFilter) -> Vec<Value> {
    if filter.group.as_deref() != Some("scratchpad") || filter.read_only != Some(true) {
        return Vec::new();
    }

    let mut eligible_tools = Vec::new();
    let mut destructive_tools = Vec::new();
    let mut local_only_tools = Vec::new();
    let mut rest_read_tools = Vec::new();
    let mut manage_scope_read_tools = Vec::new();
    for capability in inventory.capabilities().into_iter().filter(|capability| {
        capability.group() == Some("scratchpad")
            && !capability.read_only()
            && inventory.is_allowed(
                capability.name(),
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            )
    }) {
        let name = capability.name().to_string();
        eligible_tools.push(name.clone());
        if SCRATCHPAD_MANAGE_SCOPE_READ_TOOLS.contains(&capability.name()) {
            manage_scope_read_tools.push(name.clone());
        } else if SCRATCHPAD_REST_READ_TOOLS.contains(&capability.name()) {
            rest_read_tools.push(name.clone());
        } else {
            local_only_tools.push(name.clone());
        }
        if capability.risk_posture().is_some_and(|posture| {
            posture.operation_class == GuardedActionOperationClass::Destructive
        }) {
            destructive_tools.push(name);
        }
    }
    eligible_tools.sort();
    destructive_tools.sort();
    local_only_tools.sort();
    rest_read_tools.sort();
    manage_scope_read_tools.sort();

    vec![json!({
        "type": "filter_alternative",
        "status": "available_by_explicit_opt_in",
        "reason_code": "active_filter_excludes_local_state_tools",
        "mutation_scope": "local_mcp_scratchpad_state",
        "discovery_retry_calls_upstream": false,
        "upstream_gam_reads_possible": true,
        "upstream_gam_mutation": false,
        "runtime_write_enablement_required": false,
        "local_state_writes_enabled_by_default": true,
        "requires_explicit_operator_intent": true,
        "retry_filter": {
            "group": "scratchpad",
            "read_only": false,
        },
        "eligible_tools": eligible_tools,
        "tool_access_classes": [
            {
                "class": "local_only",
                "tools": local_only_tools,
                "upstream_call": false,
                "scope_required": null,
                "manage_scope_required": false
            },
            {
                "class": "gam_rest_read",
                "tools": rest_read_tools,
                "upstream_call": true,
                "scope_required": "https://www.googleapis.com/auth/admanager.readonly",
                "manage_scope_required": false
            },
            {
                "class": "gam_soap_read",
                "tools": manage_scope_read_tools,
                "upstream_call": true,
                "scope_required": "https://www.googleapis.com/auth/admanager",
                "manage_scope_required": true
            }
        ],
        "destructive_tools": destructive_tools,
        "guidance": [
            "This explicit filter enables bounded local scratchpad session state only; it does not authorize or perform a Google Ad Manager mutation.",
            "Scratchpad open, query, list, ingest, and export calls may create, refresh, or prune local session state; ingest tools can also perform upstream GAM reads under the scope shown in tool_access_classes.",
            "Close-session and drop-table tools remove local state and remain distinctly labelled destructive."
        ]
    })]
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

pub(crate) fn recognized_group_filter(
    inventory: &ToolInventory,
    group: Option<&str>,
) -> Option<String> {
    let group = group?.trim();
    if group.is_empty() {
        return None;
    }
    inventory
        .capabilities()
        .into_iter()
        .filter(|capability| {
            inventory.is_allowed(
                capability.name(),
                ToolOperation::List,
                &ToolInventoryPolicy::strict(),
            )
        })
        .any(|capability| capability.group() == Some(group))
        .then(|| group.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        REPRESENTATIVE_DISCOVERY_CANDIDATES, WorkflowCompanion, available_groups,
        companion_result_records, companion_tool_names, recognized_group_filter,
        recovery_result_record, workflow_companions,
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
                "gam_report_run" => {
                    assert_eq!(
                        prerequisites,
                        vec!["gam_networks_list", "gam_network_catalog_list"]
                    )
                }
                "gam_report_operation_poll" => assert_eq!(
                    prerequisites,
                    vec![
                        "gam_networks_list",
                        "gam_network_catalog_list",
                        "gam_report_run",
                    ]
                ),
                "gam_report_result_rows" => assert_eq!(
                    prerequisites,
                    vec![
                        "gam_networks_list",
                        "gam_network_catalog_list",
                        "gam_report_run",
                        "gam_report_operation_poll",
                    ]
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
    fn report_rows_add_complete_cold_start_and_async_chain() {
        let results = [result("gam_report_result_rows")];
        let companions = workflow_companions(&results);
        assert_eq!(
            companion_tool_names(&companions),
            vec![
                "gam_networks_list",
                "gam_network_catalog_list",
                "gam_report_run",
                "gam_report_operation_poll",
            ]
        );
        assert_eq!(
            companion_edges(&companions),
            vec![
                ("gam_networks_list", "gam_network_catalog_list", true, false,),
                ("gam_network_catalog_list", "gam_report_run", true, false,),
                ("gam_report_run", "gam_report_operation_poll", false, false,),
                (
                    "gam_report_operation_poll",
                    "gam_report_result_rows",
                    false,
                    false,
                ),
            ]
        );
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
        assert!(recovery.get("local_state_alternatives").is_none());
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
    fn scratchpad_read_only_recovery_explains_local_state_opt_in() {
        let inventory = build_tool_inventory().expect("inventory");
        let recovery = recovery_for_filter(
            &inventory,
            &ToolSearchFilter {
                query: Some("scratchpad".to_string()),
                group: Some("scratchpad".to_string()),
                read_only: Some(true),
                limit: Some(100),
            },
        );
        assert_eq!(recovery["status"], "no_matches");
        assert_eq!(recovery["active_filter"]["group"], "scratchpad");
        assert_eq!(recovery["active_filter"]["read_only"], true);
        assert!(recovery_queries(&recovery).is_empty());
        assert!(
            recovery["available_groups"]
                .as_array()
                .is_some_and(|groups| !groups.contains(&serde_json::json!("scratchpad")))
        );

        let alternatives = recovery["local_state_alternatives"]
            .as_array()
            .expect("local-state alternatives");
        assert_eq!(alternatives.len(), 1);
        let alternative = &alternatives[0];
        assert_eq!(
            alternative["reason_code"],
            "active_filter_excludes_local_state_tools"
        );
        assert_eq!(alternative["mutation_scope"], "local_mcp_scratchpad_state");
        assert_eq!(alternative["discovery_retry_calls_upstream"], false);
        assert_eq!(alternative["upstream_gam_reads_possible"], true);
        assert_eq!(alternative["upstream_gam_mutation"], false);
        assert_eq!(alternative["local_state_writes_enabled_by_default"], true);
        assert!(alternative.get("upstream_manage_scope_required").is_none());
        assert_eq!(alternative["retry_filter"]["group"], "scratchpad");
        assert_eq!(alternative["retry_filter"]["read_only"], false);
        assert_eq!(
            alternative["eligible_tools"],
            serde_json::json!([
                "gam_scratchpad_close_session",
                "gam_scratchpad_drop_table",
                "gam_scratchpad_export_evidence_bundle",
                "gam_scratchpad_ingest_network_catalog",
                "gam_scratchpad_ingest_report_result_rows",
                "gam_scratchpad_ingest_soap_line_items",
                "gam_scratchpad_list_sessions",
                "gam_scratchpad_list_tables",
                "gam_scratchpad_open_session",
                "gam_scratchpad_query"
            ])
        );
        assert_eq!(
            alternative["destructive_tools"],
            serde_json::json!(["gam_scratchpad_close_session", "gam_scratchpad_drop_table"])
        );
        assert_eq!(
            alternative["tool_access_classes"],
            serde_json::json!([
                {
                    "class":"local_only",
                    "tools":[
                        "gam_scratchpad_close_session",
                        "gam_scratchpad_drop_table",
                        "gam_scratchpad_export_evidence_bundle",
                        "gam_scratchpad_list_sessions",
                        "gam_scratchpad_list_tables",
                        "gam_scratchpad_open_session",
                        "gam_scratchpad_query"
                    ],
                    "upstream_call":false,
                    "scope_required":null,
                    "manage_scope_required":false
                },
                {
                    "class":"gam_rest_read",
                    "tools":[
                        "gam_scratchpad_ingest_network_catalog",
                        "gam_scratchpad_ingest_report_result_rows"
                    ],
                    "upstream_call":true,
                    "scope_required":"https://www.googleapis.com/auth/admanager.readonly",
                    "manage_scope_required":false
                },
                {
                    "class":"gam_soap_read",
                    "tools":["gam_scratchpad_ingest_soap_line_items"],
                    "upstream_call":true,
                    "scope_required":"https://www.googleapis.com/auth/admanager",
                    "manage_scope_required":true
                }
            ])
        );
        assert!(
            recovery
                .to_string()
                .contains("local scratchpad session state only")
        );
        assert!(
            serde_json::to_vec(&recovery)
                .expect("scratchpad recovery serializes")
                .len()
                < 6 * 1024
        );
    }

    #[test]
    fn fail_closed_scratchpad_recovery_does_not_offer_local_state_opt_in() {
        let inventory = build_tool_inventory().expect("inventory");
        let filter = ToolSearchFilter {
            query: Some(format!("scratchpad {}", "z".repeat(64 * 1024))),
            group: Some("scratchpad".to_string()),
            read_only: Some(true),
            limit: Some(100),
        };
        let ranked =
            inventory.search_ranked(&filter, ToolOperation::List, &ToolInventoryPolicy::strict());
        assert_eq!(ranked.match_summary.total_matches, 0);
        let recovery = recovery_result_record(&inventory, &filter, &ranked.match_summary)
            .expect("fail-closed scratchpad recovery");
        assert_eq!(recovery["fail_closed"], true);
        assert!(recovery_queries(&recovery).is_empty());
        assert!(recovery.get("local_state_alternatives").is_none());
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
                "continue waiting for an existing report operation",
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
        assert_eq!(recovery["active_filter"]["group"], serde_json::Value::Null);
        assert_eq!(recovery["active_filter"]["group_supplied"], true);
        assert_eq!(recovery["active_filter"]["group_recognized"], false);
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
    fn recognized_group_filter_returns_only_strict_list_visible_group_literals() {
        let inventory = build_tool_inventory().expect("inventory");
        assert_eq!(
            recognized_group_filter(&inventory, Some(" trafficking ")),
            Some("trafficking".to_string())
        );
        assert_eq!(
            recognized_group_filter(&inventory, Some("leading-secret-marker")),
            None
        );
        assert_eq!(recognized_group_filter(&inventory, None), None);
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
