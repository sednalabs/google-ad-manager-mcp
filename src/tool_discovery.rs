//! Provider-specific tool-discovery orchestration.
//!
//! This module keeps semantic ranking in `mcp-toolkit-rs` and owns only the
//! Google Ad Manager workflow relationships that the generic toolkit cannot
//! infer, such as guided builder/plan/apply dependencies and filter recovery.

use std::collections::{BTreeMap, BTreeSet};

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
const MAX_RECOVERY_GROUPS: usize = 16;
const MAX_RECOVERY_EXAMPLE_QUERIES: usize = 8;
const MAX_RECOVERY_LOCAL_STATE_TOOLS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepresentativeDiscoveryCandidate {
    pub query: &'static str,
    pub expected_tool: &'static str,
    pub group: &'static str,
    pub read_only: bool,
    pub no_match_recovery: bool,
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
    rank_only_candidate(
        "start a campaign delivery audit with a saved report",
        "gam_report_run",
        "reports",
        false,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportDiscoveryIntent {
    ExplicitNewRun,
    ExistingOperationContinuation,
    Unspecified,
}

impl ReportDiscoveryIntent {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitNewRun => "explicit_new_run",
            Self::ExistingOperationContinuation => "existing_operation_continuation",
            Self::Unspecified => "unspecified",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ReportDiscoveryResolution {
    pub records: Vec<Value>,
    pub callable_alternatives: Vec<&'static str>,
}

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
        no_match_recovery: true,
    }
}

const fn rank_only_candidate(
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
        no_match_recovery: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkflowDependency {
    tool_name: &'static str,
    before_tool: &'static str,
    callable_as_tool: bool,
    required_for_guided_sequence: bool,
    reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkflowCompanion {
    pub tool_name: &'static str,
    pub before_tool: &'static str,
    pub callable_as_tool: bool,
    pub required_for_guided_sequence: bool,
    pub tool_already_selected: bool,
    pub reason: &'static str,
}

// Declaration order is not trusted; the provider composer validates and sorts the full graph.
const WORKFLOW_DEPENDENCIES: [WorkflowDependency; 8] = [
    WorkflowDependency {
        tool_name: "gam_networks_list",
        before_tool: "gam_network_catalog_list",
        callable_as_tool: true,
        required_for_guided_sequence: true,
        reason: "Cold-start catalog sequence: discover the exact network code before listing a network collection. Discovery does not prove that network discovery ran.",
    },
    WorkflowDependency {
        tool_name: "gam_network_catalog_list",
        before_tool: "gam_report_run",
        callable_as_tool: true,
        required_for_guided_sequence: true,
        reason: "Cold-start report sequence: list collection=reports with the exact network code to obtain a saved report id before starting a run. Discovery does not prove that report catalog lookup ran.",
    },
    WorkflowDependency {
        tool_name: "gam_report_run",
        before_tool: "gam_report_operation_poll",
        callable_as_tool: false,
        required_for_guided_sequence: false,
        reason: "Asynchronous report sequence: when no operation_name exists yet, start one report run with wait_for_completion=false, then pass its returned operation_name to gam_report_operation_poll. Do not call gam_report_run when an existing operation_name is already available; the poll tool never starts another report run.",
    },
    WorkflowDependency {
        tool_name: "gam_report_operation_poll",
        before_tool: "gam_report_result_rows",
        callable_as_tool: true,
        required_for_guided_sequence: false,
        reason: "Asynchronous report sequence: poll the existing operation until it returns report_result before fetching rows. This step is optional when gam_report_run already waited for completion and returned report_result.",
    },
    WorkflowDependency {
        tool_name: "gam_rest_write_plan",
        before_tool: "gam_rest_write_apply",
        callable_as_tool: true,
        required_for_guided_sequence: true,
        reason: "Guided sequence: review the REST plan before apply. Discovery does not prove that plan ran. REST apply independently revalidates its exact request and token and retains runtime, scope, and confirmation gates; configured readback is attempted where available but is not a universal success gate.",
    },
    WorkflowDependency {
        tool_name: "gam_soap_payload_build",
        before_tool: "gam_soap_trafficking_plan",
        callable_as_tool: true,
        required_for_guided_sequence: false,
        reason: "Guided sequence: optionally build a typed SOAP payload before planning. Discovery does not prove the builder ran; independently authored payload_xml remains valid input, and the plan validates and builds its exact no-upstream-call request.",
    },
    WorkflowDependency {
        tool_name: "gam_soap_trafficking_plan",
        before_tool: "gam_soap_trafficking_apply",
        callable_as_tool: true,
        required_for_guided_sequence: true,
        reason: "Guided sequence: review the SOAP trafficking plan before apply. Discovery does not prove that plan ran. Generic SOAP apply independently revalidates its exact request and token and retains runtime, scope, and confirmation gates; follow-up verification is still required.",
    },
    WorkflowDependency {
        tool_name: "gam_yield_group_exclusions_preview",
        before_tool: "gam_yield_group_exclusions_apply",
        callable_as_tool: true,
        required_for_guided_sequence: true,
        reason: "Guided sequence: review descendant-safe yield-group exclusions before apply. Discovery does not prove that preview ran. Typed yield apply independently revalidates its exact request and token, retains runtime, scope, and confirmation gates, and requires descendant-safe post-apply readback.",
    },
];

const AD_UNIT_RETIREMENT_DEPENDENCIES: [WorkflowDependency; 3] = [
    WorkflowDependency {
        tool_name: "gam_network_catalog_list",
        before_tool: "gam_ad_unit_dependency_probe",
        callable_as_tool: true,
        required_for_guided_sequence: true,
        reason: "Ad-unit retirement sequence: resolve the exact ad-unit code and canonical id before checking placement and line-item dependencies.",
    },
    WorkflowDependency {
        tool_name: "gam_ad_unit_dependency_probe",
        before_tool: "gam_ad_unit_retirement_assessment",
        callable_as_tool: true,
        required_for_guided_sequence: true,
        reason: "Ad-unit retirement sequence: inspect current placement and line-item dependencies before producing a freshness-bound retirement assessment.",
    },
    WorkflowDependency {
        tool_name: "gam_ad_unit_retirement_assessment",
        before_tool: "gam_rest_write_plan",
        callable_as_tool: true,
        required_for_guided_sequence: true,
        reason: "Ad-unit archive/deactivate sequence: review the conservative identity, hierarchy, dependency, and freshness assessment before creating any REST write plan.",
    },
];

pub(crate) fn compose_workflow_companions(
    results: &[ToolSearchResult],
    query: Option<&str>,
) -> Result<Vec<WorkflowCompanion>, &'static str> {
    let mut dependencies = WORKFLOW_DEPENDENCIES.to_vec();
    if is_ad_unit_retirement_intent(query) {
        dependencies.extend(AD_UNIT_RETIREMENT_DEPENDENCIES);
    }
    let dependencies = topologically_sorted_dependencies(dependencies)?;
    let selected = results
        .iter()
        .map(|result| result.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut prerequisites = selected.clone();
    if selected.contains("gam_report_run")
        && report_discovery_intent(query) == ReportDiscoveryIntent::ExistingOperationContinuation
    {
        prerequisites.insert("gam_report_operation_poll");
    }
    loop {
        let mut changed = false;
        for dependency in dependencies.iter().copied() {
            if dependency.callable_as_tool && prerequisites.contains(dependency.before_tool) {
                changed |= prerequisites.insert(dependency.tool_name);
            }
        }
        if !changed {
            break;
        }
    }

    let mut emitted_edges = BTreeSet::new();
    Ok(dependencies
        .into_iter()
        .filter(|dependency| prerequisites.contains(dependency.before_tool))
        .filter(|dependency| emitted_edges.insert((dependency.tool_name, dependency.before_tool)))
        .map(|dependency| WorkflowCompanion {
            tool_name: dependency.tool_name,
            before_tool: dependency.before_tool,
            callable_as_tool: dependency.callable_as_tool,
            required_for_guided_sequence: dependency.required_for_guided_sequence,
            tool_already_selected: selected.contains(dependency.tool_name),
            reason: dependency.reason,
        })
        .collect())
}

fn topologically_sorted_dependencies(
    dependencies: Vec<WorkflowDependency>,
) -> Result<Vec<WorkflowDependency>, &'static str> {
    let mut deduplicated = BTreeMap::new();
    for dependency in dependencies {
        deduplicated
            .entry((dependency.tool_name, dependency.before_tool))
            .or_insert(dependency);
    }
    let dependencies = deduplicated.into_values().collect::<Vec<_>>();
    let mut nodes = BTreeSet::new();
    let mut outgoing = BTreeMap::<&str, BTreeSet<&str>>::new();
    let mut indegree = BTreeMap::<&str, usize>::new();
    for dependency in dependencies.iter().copied() {
        nodes.insert(dependency.tool_name);
        nodes.insert(dependency.before_tool);
        outgoing
            .entry(dependency.tool_name)
            .or_default()
            .insert(dependency.before_tool);
        *indegree.entry(dependency.before_tool).or_default() += 1;
        indegree.entry(dependency.tool_name).or_default();
    }

    let mut ready = nodes
        .iter()
        .copied()
        .filter(|node| indegree.get(node).copied().unwrap_or_default() == 0)
        .collect::<BTreeSet<_>>();
    let mut ordered_nodes = Vec::with_capacity(nodes.len());
    while let Some(node) = ready.pop_first() {
        ordered_nodes.push(node);
        if let Some(successors) = outgoing.get(node) {
            for successor in successors {
                let successor_indegree = indegree
                    .get_mut(successor)
                    .expect("workflow successor has an indegree entry");
                *successor_indegree -= 1;
                if *successor_indegree == 0 {
                    ready.insert(successor);
                }
            }
        }
    }
    if ordered_nodes.len() != nodes.len() {
        return Err("provider workflow dependency graph contains a cycle");
    }

    let node_order = ordered_nodes
        .into_iter()
        .enumerate()
        .map(|(index, node)| (node, index))
        .collect::<BTreeMap<_, _>>();
    let mut dependencies = dependencies;
    dependencies.sort_by_key(|dependency| {
        (
            *node_order
                .get(dependency.before_tool)
                .expect("workflow target has a topological position"),
            *node_order
                .get(dependency.tool_name)
                .expect("workflow source has a topological position"),
            dependency.tool_name,
            dependency.before_tool,
        )
    });
    Ok(dependencies)
}

#[cfg(test)]
fn workflow_companions(
    results: &[ToolSearchResult],
    query: Option<&str>,
) -> Vec<WorkflowCompanion> {
    compose_workflow_companions(results, query).expect("static workflow graph must be acyclic")
}

fn is_ad_unit_retirement_intent(query: Option<&str>) -> bool {
    let Some(query) = query else {
        return false;
    };
    let normalized = query.to_ascii_lowercase().replace(['-', '_'], " ");
    normalized.contains("ad unit")
        && normalized.split_whitespace().any(|term| {
            matches!(
                term,
                "archive"
                    | "archiving"
                    | "deactivate"
                    | "deactivating"
                    | "deactivation"
                    | "retire"
                    | "retiring"
                    | "retirement"
            )
        })
}

fn report_discovery_intent(query: Option<&str>) -> ReportDiscoveryIntent {
    let Some(query) = query else {
        return ReportDiscoveryIntent::Unspecified;
    };
    let normalized = query.to_ascii_lowercase().replace(['-', '_'], " ");
    let terms = normalized
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    let term_set = terms.iter().copied().collect::<BTreeSet<_>>();
    let report_context = term_set.contains("report")
        || term_set.contains("reports")
        || normalized.contains("campaign delivery audit");
    let clear_new_run_action = has_clear_report_start_action(&terms, report_context, &normalized);
    let strong_continuation_action = terms.iter().any(|term| is_report_continuation_term(term));
    let wait_action = term_set.contains("wait") || term_set.contains("waiting");
    let explicit_existing_operation_reference =
        has_explicit_existing_report_operation_reference(&terms, report_context, &normalized);
    let report_run_noun_reference =
        has_explicit_existing_report_run_reference(&terms, report_context);
    if explicit_existing_operation_reference || report_run_noun_reference {
        return ReportDiscoveryIntent::ExistingOperationContinuation;
    }

    if clear_new_run_action {
        ReportDiscoveryIntent::ExplicitNewRun
    } else if (normalized.contains("continue waiting") && report_context)
        || (strong_continuation_action && report_context)
        || (wait_action && report_context)
    {
        ReportDiscoveryIntent::ExistingOperationContinuation
    } else {
        ReportDiscoveryIntent::Unspecified
    }
}

fn has_explicit_existing_report_operation_reference(
    terms: &[&str],
    report_context: bool,
    normalized: &str,
) -> bool {
    if normalized.contains("/operations/reports/runs/") {
        return true;
    }
    if !report_context {
        return false;
    }
    terms.iter().enumerate().any(|(index, term)| {
        if !matches!(*term, "operation" | "operations") {
            return false;
        }
        let previous = index.checked_sub(1).map(|previous| terms[previous]);
        let next = terms.get(index + 1).copied();
        matches!(previous, Some("existing" | "current" | "latest" | "active"))
            || matches!(next, Some("name" | "handle" | "id"))
            || next.is_some_and(is_numeric_term)
    })
}

fn has_explicit_existing_report_run_reference(terms: &[&str], report_context: bool) -> bool {
    if !report_context {
        return false;
    }
    terms.iter().enumerate().any(|(index, term)| {
        if !matches!(*term, "run" | "runs") {
            return false;
        }
        let previous = index.checked_sub(1).map(|previous| terms[previous]);
        let next = terms.get(index + 1).copied();
        let explicit_identity = matches!(next, Some("id"))
            || next.is_some_and(is_numeric_term)
            || matches!(
                previous,
                Some("existing" | "current" | "latest" | "recent" | "active")
            );
        if explicit_identity {
            return true;
        }
        let reverse_relation = terms[index + 1..]
            .iter()
            .find(|candidate| !is_report_reference_article(candidate))
            .is_some_and(|candidate| matches!(*candidate, "of" | "for"));
        if reverse_relation {
            return true;
        }
        let Some(report_index) = terms[..index]
            .iter()
            .rposition(|candidate| matches!(*candidate, "report" | "reports"))
        else {
            return false;
        };
        let action_precedes_report = terms[..report_index]
            .iter()
            .any(|candidate| is_report_start_action(candidate));
        if !action_precedes_report {
            return true;
        }
        let action_index = terms[..report_index]
            .iter()
            .rposition(|candidate| is_report_start_action(candidate))
            .expect("action presence checked above");
        let reference_terms = &terms[action_index + 1..index];
        reference_terms
            .iter()
            .rev()
            .find(|candidate| {
                matches!(
                    **candidate,
                    "new" | "existing" | "current" | "latest" | "recent" | "active"
                )
            })
            .is_some_and(|candidate| {
                matches!(
                    *candidate,
                    "existing" | "current" | "latest" | "recent" | "active"
                )
            })
    })
}

fn is_numeric_term(term: &str) -> bool {
    !term.is_empty() && term.chars().all(|character| character.is_ascii_digit())
}

fn is_report_reference_article(term: &str) -> bool {
    matches!(term, "a" | "an" | "the" | "one" | "saved")
}

fn has_clear_report_start_action(terms: &[&str], report_context: bool, normalized: &str) -> bool {
    if !report_context {
        return false;
    }
    terms.iter().enumerate().any(|(index, term)| {
        if !is_report_start_action(term) {
            return false;
        }
        if has_non_execution_report_language(terms, index, normalized) {
            return false;
        }
        let object_terms = &terms[index + 1..];
        if let Some(consumed) = campaign_delivery_audit_report_object_end(object_terms) {
            return report_start_tail_is_safe(&object_terms[consumed..]);
        }
        for (candidate_index, candidate) in object_terms.iter().enumerate() {
            if matches!(*candidate, "report" | "reports") {
                return report_start_tail_is_safe(&object_terms[candidate_index + 1..]);
            }
            if !is_report_object_modifier(candidate) {
                return false;
            }
        }
        false
    })
}

fn is_report_start_action(term: &str) -> bool {
    matches!(term, "start" | "run" | "launch" | "execute")
}

fn has_non_execution_report_language(
    terms: &[&str],
    action_index: usize,
    normalized: &str,
) -> bool {
    normalized.contains("do not")
        || normalized.contains("don't")
        || terms
            .iter()
            .any(|term| matches!(*term, "not" | "never" | "without"))
        || terms.iter().any(|term| {
            matches!(
                *term,
                "plan" | "planning" | "preview" | "previewing" | "simulate" | "simulation"
            )
        })
        || terms[..action_index].iter().any(|term| {
            matches!(
                *term,
                "how"
                    | "show"
                    | "showing"
                    | "explain"
                    | "explaining"
                    | "example"
                    | "examples"
                    | "tutorial"
                    | "guide"
            )
        })
}

fn campaign_delivery_audit_report_object_end(terms: &[&str]) -> Option<usize> {
    let mut index = usize::from(matches!(terms.first(), Some(&"a" | &"an" | &"the")));
    if terms.get(index..index + 3) != Some(&["campaign", "delivery", "audit"]) {
        return None;
    }
    index += 3;
    if terms.get(index) != Some(&"with") {
        return None;
    }
    index += 1;
    if matches!(terms.get(index), Some(&"a" | &"an" | &"the" | &"one")) {
        index += 1;
    }
    if terms.get(index) == Some(&"saved") {
        index += 1;
    }
    matches!(terms.get(index), Some(&"report" | &"reports")).then_some(index + 1)
}

fn report_start_tail_is_safe(terms: &[&str]) -> bool {
    if terms.is_empty() {
        return true;
    }
    if terms.iter().all(|term| matches!(*term, "now" | "please")) {
        return true;
    }
    let starts_as_continuation = matches!(terms.first(), Some(&"and" | &"then" | &"until"));
    let has_continuation_action = terms
        .iter()
        .any(|term| is_report_continuation_term(term) || matches!(*term, "show" | "showing"));
    starts_as_continuation
        && has_continuation_action
        && terms.iter().all(|term| {
            is_report_continuation_term(term)
                || matches!(
                    *term,
                    "and"
                        | "then"
                        | "until"
                        | "show"
                        | "showing"
                        | "it"
                        | "its"
                        | "this"
                        | "the"
                        | "new"
                        | "result"
                        | "results"
                        | "status"
                        | "operation"
                        | "run"
                        | "runs"
                        | "for"
                        | "to"
                        | "completion"
                        | "complete"
                        | "completed"
                        | "done"
                        | "finish"
                        | "finishes"
                        | "finishing"
                        | "now"
                        | "please"
                )
        })
}

fn is_report_object_modifier(term: &str) -> bool {
    matches!(
        term,
        "a" | "an"
            | "the"
            | "one"
            | "new"
            | "saved"
            | "this"
            | "that"
            | "my"
            | "our"
            | "your"
            | "another"
            | "latest"
            | "current"
            | "daily"
            | "weekly"
            | "monthly"
            | "quarter"
            | "quarterly"
            | "annual"
            | "campaign"
            | "delivery"
            | "audit"
            | "performance"
            | "inventory"
            | "revenue"
            | "sales"
            | "advertiser"
            | "google"
            | "ad"
            | "manager"
    )
}

fn is_report_continuation_term(term: &str) -> bool {
    matches!(
        term,
        "continue"
            | "continuation"
            | "resume"
            | "status"
            | "check"
            | "checking"
            | "poll"
            | "polling"
            | "monitor"
            | "monitoring"
            | "inspect"
            | "inspecting"
            | "view"
            | "viewing"
            | "wait"
            | "waiting"
    )
}

pub(crate) fn companion_tool_names(companions: &[WorkflowCompanion]) -> Vec<&'static str> {
    let mut injected_names = BTreeSet::new();
    companions
        .iter()
        .filter(|companion| companion.callable_as_tool)
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
                "callable_as_tool": companion.callable_as_tool,
                "selection_semantics": if companion.callable_as_tool {
                    "callable_companion"
                } else {
                    "condition_only"
                },
                "invocation_condition": if !companion.callable_as_tool
                    && companion.tool_name == "gam_report_run"
                {
                    Value::String("only_when_no_existing_operation_name".to_string())
                } else {
                    Value::Null
                },
                "tool_already_selected": companion.tool_already_selected,
                "required": companion.required_for_guided_sequence,
                "required_for_guided_sequence": companion.required_for_guided_sequence,
                "required_semantics": "guided_sequence_compatibility_alias",
                "server_call_enforced": false,
                "reason": companion.reason,
                "effect": if !companion.callable_as_tool
                    && companion.tool_name == "gam_report_run"
                {
                    Value::String("starts_upstream_job".to_string())
                } else {
                    Value::Null
                },
                "mutation_performed": false,
                "safety": "non_mutating_guidance",
            })
        })
        .collect()
}

pub(crate) fn provider_guidance_allowed(summary: &ToolSearchMatchSummary) -> bool {
    search_semantics_fail_closed_reasons(summary).is_empty()
}

pub(crate) fn resolve_report_discovery(
    results: &mut Vec<ToolSearchResult>,
    companions: &[WorkflowCompanion],
    query: Option<&str>,
    active_group: Option<&str>,
    group_filter_supplied: bool,
    read_only: Option<bool>,
) -> ReportDiscoveryResolution {
    let intent = report_discovery_intent(query);
    let contextual_companions = companions
        .iter()
        .filter(|companion| !companion.callable_as_tool && companion.tool_already_selected)
        .map(|companion| (companion.tool_name, *companion))
        .collect::<BTreeMap<_, _>>();
    let mut retained = Vec::with_capacity(results.len());
    let mut condition_only = Vec::new();
    for (index, result) in std::mem::take(results).into_iter().enumerate() {
        let contextual_companion = contextual_companions.get(result.name.as_str());
        let is_report_run = result.name == "gam_report_run";
        let report_run_callable = is_report_run
            && read_only == Some(false)
            && intent == ReportDiscoveryIntent::ExplicitNewRun
            && contextual_companion.is_none();
        if is_report_run && !report_run_callable {
            let companion_before_tool = contextual_companion.map(|companion| companion.before_tool);
            condition_only.push(json!({
                "type": "condition_only_match",
                "name": result.name,
                "rank": index + 1,
                "group": result.group,
                "description": result.description,
                "registered_read_only": result.read_only,
                "registered_risk_posture": result.risk_posture,
                "relation": "condition",
                "selection_semantics": "condition_only",
                "callable_as_tool": false,
                "schema_exposed": false,
                "tool_already_selected": true,
                "required_for_guided_sequence": contextual_companion
                    .is_some_and(|companion| companion.required_for_guided_sequence),
                "server_call_enforced": false,
                "before_tool": if intent == ReportDiscoveryIntent::ExistingOperationContinuation {
                    None
                } else {
                    companion_before_tool
                },
                "invocation_condition": "explicit_new_run_intent_and_read_only_false",
                "effect": "starts_upstream_job",
                "report_intent": intent.as_str(),
                "reason": if intent == ReportDiscoveryIntent::ExistingOperationContinuation {
                    "The bounded discovery query expresses continuation of an existing report operation. Starting a new report run would create duplicate upstream work, so use the GET-only poll alternative."
                } else if read_only != Some(false) {
                    "Starting a report creates an upstream job and requires an explicit read_only=false discovery filter plus explicit new-run intent."
                } else {
                    "Starting a report creates an upstream job. The bounded discovery query did not express explicit new-run intent, so this match is not callable."
                },
            }));
        } else {
            retained.push(result);
        }
    }
    *results = retained;
    let mut resolution = ReportDiscoveryResolution {
        records: condition_only,
        callable_alternatives: Vec::new(),
    };
    let report_group_selected = !group_filter_supplied || active_group == Some("reports");
    if intent == ReportDiscoveryIntent::ExistingOperationContinuation
        && report_group_selected
        && read_only == Some(false)
    {
        resolution
            .callable_alternatives
            .push("gam_report_operation_poll");
        resolution.records.push(json!({
            "type": "intent_safe_alternative",
            "name": "gam_report_operation_poll",
            "relation": "safe_alternative",
            "callable_as_tool": true,
            "schema_exposed": true,
            "selection_semantics": "callable_safe_alternative",
            "registered_read_only": true,
            "active_filter_read_only": false,
            "active_filter_exception": "continuation_safety",
            "report_intent": intent.as_str(),
            "effect": "polls_existing_operation_with_get",
            "starts_upstream_job": false,
            "safe_method": "GET",
            "reason": "The bounded discovery query asks to continue an existing report operation. The active read_only=false filter excludes the registered read-only poll tool, so discovery exposes that safe callable alternative without replaying gam_report_run.",
        }));
    }
    resolution
}

pub(crate) fn recovery_result_record(
    inventory: &ToolInventory,
    filter: &ToolSearchFilter,
    summary: &ToolSearchMatchSummary,
) -> Option<Value> {
    if summary.total_matches > 0 {
        return None;
    }
    let fail_closed_reasons = search_semantics_fail_closed_reasons(summary);
    let mut example_queries = if fail_closed_reasons.is_empty() {
        validated_recovery_example_queries(inventory, filter)
    } else {
        Vec::new()
    };
    let example_query_total = example_queries.len();
    example_queries.truncate(MAX_RECOVERY_EXAMPLE_QUERIES);
    let local_state_alternatives = if fail_closed_reasons.is_empty() {
        local_state_alternatives(inventory, filter)
    } else {
        Vec::new()
    };
    let mut guidance = vec![
        "Retry with one outcome phrase and one or two Google Ad Manager nouns.",
        "Remove an incorrect group filter or choose one of the available groups.",
        "Keep read_only=true when you want non-mutating tools; do not relax it merely to force a match.",
        "Request include_schema=true only after discovery has narrowed the complete direct-plus-companion selection to no more than five tools.",
    ];
    if !local_state_alternatives.is_empty() {
        guidance.push(
            "For a deliberate scratchpad workflow, review local_state_alternatives before explicitly opting into bounded MCP-local state changes.",
        );
    }
    let group_input_invalid = fail_closed_reasons
        .iter()
        .any(|reason| reason == "group_input");
    let recognized_group = if group_input_invalid {
        None
    } else {
        recognized_group_filter(inventory, filter.group.as_deref())
    };
    let group_supplied = group_input_invalid || filter.group.is_some();
    let group_recognized =
        !group_input_invalid && (filter.group.is_none() || recognized_group.is_some());
    let mut groups = available_groups(inventory, filter.read_only);
    let group_total = groups.len();
    groups.truncate(MAX_RECOVERY_GROUPS);
    let mut recovery = json!({
        "type": "search_recovery",
        "status": "no_matches",
        "fail_closed": !fail_closed_reasons.is_empty(),
        "reason_codes": fail_closed_reasons,
        "active_filter": {
            "group": recognized_group,
            "group_supplied": group_supplied,
            "group_recognized": group_recognized,
            "read_only": filter.read_only,
        },
        "available_groups": groups,
        "available_group_counts": {
            "total": group_total,
            "returned": std::cmp::min(group_total, MAX_RECOVERY_GROUPS),
            "truncated": group_total > MAX_RECOVERY_GROUPS,
        },
        "retry": {
            "example_queries_validated_under_active_filter": true,
            "guidance": guidance,
            "example_queries": example_queries,
            "example_query_counts": {
                "total": example_query_total,
                "returned": std::cmp::min(example_query_total, MAX_RECOVERY_EXAMPLE_QUERIES),
                "truncated": example_query_total > MAX_RECOVERY_EXAMPLE_QUERIES,
            },
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

pub(crate) fn supplemental_local_state_alternative_records(
    inventory: &ToolInventory,
    filter: &ToolSearchFilter,
    summary: &ToolSearchMatchSummary,
) -> Vec<Value> {
    if summary.total_matches == 0 || !search_semantics_fail_closed_reasons(summary).is_empty() {
        return Vec::new();
    }

    local_state_alternatives(inventory, filter)
}

fn search_semantics_fail_closed_reasons(summary: &ToolSearchMatchSummary) -> Vec<String> {
    let mut reasons = summary
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
    if !summary.excluded_query_terms.is_empty()
        && !reasons
            .iter()
            .any(|reason| reason == "excluded_query_terms")
    {
        reasons.push("excluded_query_terms".to_string());
    }
    reasons
}

fn local_state_alternatives(inventory: &ToolInventory, filter: &ToolSearchFilter) -> Vec<Value> {
    let scratchpad_intent = filter.group.as_deref() == Some("scratchpad")
        || filter.query.as_deref().is_some_and(|query| {
            query
                .to_ascii_lowercase()
                .split(|character: char| !character.is_ascii_alphanumeric())
                .any(|term| term == "scratchpad")
        });
    if !scratchpad_intent || filter.read_only != Some(true) {
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

    let eligible_tool_total = eligible_tools.len();
    let destructive_tool_total = destructive_tools.len();
    let local_only_tool_total = local_only_tools.len();
    let rest_read_tool_total = rest_read_tools.len();
    let manage_scope_read_tool_total = manage_scope_read_tools.len();
    eligible_tools.truncate(MAX_RECOVERY_LOCAL_STATE_TOOLS);
    destructive_tools.truncate(MAX_RECOVERY_LOCAL_STATE_TOOLS);
    local_only_tools.truncate(MAX_RECOVERY_LOCAL_STATE_TOOLS);
    rest_read_tools.truncate(MAX_RECOVERY_LOCAL_STATE_TOOLS);
    manage_scope_read_tools.truncate(MAX_RECOVERY_LOCAL_STATE_TOOLS);

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
        "rediscovery_call": {
            "tool": "find_tools",
            "arguments": {
                "query": "scratchpad session analysis",
                "group": "scratchpad",
                "read_only": false,
                "limit": 10,
                "include_schema": false,
            },
            "calls_upstream": false,
        },
        "eligible_tools": eligible_tools,
        "eligible_tool_counts": {
            "total": eligible_tool_total,
            "returned": std::cmp::min(eligible_tool_total, MAX_RECOVERY_LOCAL_STATE_TOOLS),
            "truncated": eligible_tool_total > MAX_RECOVERY_LOCAL_STATE_TOOLS,
        },
        "tool_access_classes": [
            {
                "class": "local_only",
                "tools": local_only_tools,
                "tool_counts": bounded_count(local_only_tool_total, MAX_RECOVERY_LOCAL_STATE_TOOLS),
                "upstream_call": false,
                "scope_required": null,
                "manage_scope_required": false
            },
            {
                "class": "gam_rest_read",
                "tools": rest_read_tools,
                "tool_counts": bounded_count(rest_read_tool_total, MAX_RECOVERY_LOCAL_STATE_TOOLS),
                "upstream_call": true,
                "scope_required": "https://www.googleapis.com/auth/admanager.readonly",
                "manage_scope_required": false
            },
            {
                "class": "gam_soap_read",
                "tools": manage_scope_read_tools,
                "tool_counts": bounded_count(manage_scope_read_tool_total, MAX_RECOVERY_LOCAL_STATE_TOOLS),
                "upstream_call": true,
                "scope_required": "https://www.googleapis.com/auth/admanager",
                "manage_scope_required": true
            }
        ],
        "destructive_tools": destructive_tools,
        "destructive_tool_counts": bounded_count(destructive_tool_total, MAX_RECOVERY_LOCAL_STATE_TOOLS),
        "guidance": [
            "This explicit filter enables bounded local scratchpad session state only; it does not authorize or perform a Google Ad Manager mutation.",
            "Scratchpad open, query, list, ingest, and export calls may create, refresh, or prune local session state; ingest tools can also perform upstream GAM reads under the scope shown in tool_access_classes.",
            "Close-session and drop-table tools remove local state and remain distinctly labelled destructive."
        ]
    })]
}

fn bounded_count(total: usize, limit: usize) -> Value {
    json!({
        "total": total,
        "returned": std::cmp::min(total, limit),
        "truncated": total > limit,
    })
}

fn validated_recovery_example_queries(
    inventory: &ToolInventory,
    filter: &ToolSearchFilter,
) -> Vec<&'static str> {
    REPRESENTATIVE_DISCOVERY_CANDIDATES
        .iter()
        .filter(|candidate| candidate.no_match_recovery)
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
        MAX_RECOVERY_GROUPS, REPRESENTATIVE_DISCOVERY_CANDIDATES, ReportDiscoveryIntent,
        WorkflowCompanion, WorkflowDependency, available_groups, companion_result_records,
        companion_tool_names, provider_guidance_allowed, recognized_group_filter,
        recovery_result_record, report_discovery_intent, resolve_report_discovery,
        supplemental_local_state_alternative_records, topologically_sorted_dependencies,
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
    fn no_match_recovery_catalog_never_contains_report_run() {
        assert!(
            REPRESENTATIVE_DISCOVERY_CANDIDATES
                .iter()
                .filter(|candidate| candidate.no_match_recovery)
                .all(|candidate| candidate.expected_tool != "gam_report_run")
        );
        assert!(
            REPRESENTATIVE_DISCOVERY_CANDIDATES
                .iter()
                .any(|candidate| candidate.expected_tool == "gam_report_run")
        );
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

            let prerequisites = companion_tool_names(&workflow_companions(
                &ranked.response.results,
                Some(candidate.query),
            ));
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
                "gam_report_operation_poll" => assert!(prerequisites.is_empty()),
                "gam_report_result_rows" => {
                    assert_eq!(prerequisites, vec!["gam_report_operation_poll"])
                }
                "gam_rest_write_plan"
                    if candidate.query.contains("archive an ad unit")
                        || candidate.query.contains("deactivate an ad unit") =>
                {
                    assert_eq!(
                        prerequisites,
                        vec![
                            "gam_networks_list",
                            "gam_network_catalog_list",
                            "gam_ad_unit_dependency_probe",
                            "gam_ad_unit_retirement_assessment",
                        ]
                    )
                }
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
    fn report_rows_keep_existing_operation_calls_separate_from_cold_start_guidance() {
        let results = [result("gam_report_result_rows")];
        let companions = workflow_companions(&results, None);
        assert_eq!(
            companion_tool_names(&companions),
            vec!["gam_report_operation_poll"]
        );
        assert_eq!(
            companion_edges(&companions),
            vec![
                ("gam_report_run", "gam_report_operation_poll", false, false,),
                (
                    "gam_report_operation_poll",
                    "gam_report_result_rows",
                    false,
                    false,
                ),
            ]
        );
        let records = companion_result_records(&companions);
        assert_eq!(records[0]["name"], "gam_report_run");
        assert_eq!(records[0]["callable_as_tool"], false);
        assert_eq!(records[0]["selection_semantics"], "condition_only");
        assert_eq!(
            records[0]["invocation_condition"],
            "only_when_no_existing_operation_name"
        );
        assert!(
            records[0]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("Do not call gam_report_run"))
        );
        assert_eq!(records[1]["callable_as_tool"], true);
    }

    #[test]
    fn destructive_ad_unit_intent_adds_evidence_first_retirement_chain() {
        let results = [result("gam_rest_write_plan")];
        let companions = workflow_companions(&results, Some("archive an ad unit"));
        assert_eq!(
            companion_tool_names(&companions),
            vec![
                "gam_networks_list",
                "gam_network_catalog_list",
                "gam_ad_unit_dependency_probe",
                "gam_ad_unit_retirement_assessment",
            ]
        );
        assert_eq!(
            companion_edges(&companions),
            vec![
                ("gam_networks_list", "gam_network_catalog_list", true, false),
                (
                    "gam_network_catalog_list",
                    "gam_ad_unit_dependency_probe",
                    true,
                    false,
                ),
                (
                    "gam_ad_unit_dependency_probe",
                    "gam_ad_unit_retirement_assessment",
                    true,
                    false,
                ),
                (
                    "gam_ad_unit_retirement_assessment",
                    "gam_rest_write_plan",
                    true,
                    false,
                ),
            ]
        );

        assert!(workflow_companions(&results, Some("create an ad unit")).is_empty());
    }

    #[test]
    fn ad_unit_apply_composes_then_topologically_orders_the_complete_chain() {
        let results = [result("gam_rest_write_apply")];
        let companions = workflow_companions(&results, Some("archive an ad unit"));
        assert_eq!(
            companion_edges(&companions),
            vec![
                ("gam_networks_list", "gam_network_catalog_list", true, false),
                (
                    "gam_network_catalog_list",
                    "gam_ad_unit_dependency_probe",
                    true,
                    false,
                ),
                (
                    "gam_ad_unit_dependency_probe",
                    "gam_ad_unit_retirement_assessment",
                    true,
                    false,
                ),
                (
                    "gam_ad_unit_retirement_assessment",
                    "gam_rest_write_plan",
                    true,
                    false,
                ),
                ("gam_rest_write_plan", "gam_rest_write_apply", true, false),
            ]
        );
    }

    #[test]
    fn workflow_dependency_cycles_fail_closed() {
        let dependencies = vec![
            WorkflowDependency {
                tool_name: "tool_a",
                before_tool: "tool_b",
                callable_as_tool: true,
                required_for_guided_sequence: true,
                reason: "test edge",
            },
            WorkflowDependency {
                tool_name: "tool_b",
                before_tool: "tool_a",
                callable_as_tool: true,
                required_for_guided_sequence: true,
                reason: "test edge",
            },
        ];
        assert_eq!(
            topologically_sorted_dependencies(dependencies),
            Err("provider workflow dependency graph contains a cycle")
        );
    }

    #[test]
    fn report_run_is_condition_only_when_a_continuation_is_selected() {
        let mut results = vec![result("gam_report_run"), result("gam_report_result_rows")];
        let companions = workflow_companions(&results, None);
        let resolution = resolve_report_discovery(
            &mut results,
            &companions,
            Some("fetch rows from a completed report result"),
            None,
            false,
            Some(false),
        );
        assert_eq!(
            results
                .iter()
                .map(|result| result.name.as_str())
                .collect::<Vec<_>>(),
            vec!["gam_report_result_rows"]
        );
        assert_eq!(resolution.records[0]["name"], "gam_report_run");
        assert_eq!(resolution.records[0]["callable_as_tool"], false);
        assert_eq!(resolution.records[0]["schema_exposed"], false);
        assert_eq!(resolution.records[0]["effect"], "starts_upstream_job");
        assert_eq!(
            resolution.records[0]["before_tool"],
            "gam_report_operation_poll"
        );
        assert!(
            resolution.records[0]
                .get("explicit_executable_recovery")
                .is_none()
        );
    }

    #[test]
    fn direct_report_run_requires_explicit_new_run_intent_and_write_like_filter() {
        let mut results = vec![result("gam_report_run")];
        let query = "start a campaign delivery audit with a saved report";
        let companions = workflow_companions(&results, Some(query));
        let resolution = resolve_report_discovery(
            &mut results,
            &companions,
            Some(query),
            None,
            false,
            Some(false),
        );
        assert!(resolution.records.is_empty());
        assert_eq!(results[0].name, "gam_report_run");

        let mut results = vec![result("gam_report_run")];
        let companions = workflow_companions(&results, Some("saved report delivery audit"));
        let resolution = resolve_report_discovery(
            &mut results,
            &companions,
            Some("saved report delivery audit"),
            None,
            false,
            Some(false),
        );
        assert!(results.is_empty());
        assert_eq!(resolution.records[0]["report_intent"], "unspecified");
    }

    #[test]
    fn existing_operation_intent_replaces_write_filtered_report_run_with_safe_poll() {
        let mut results = vec![result("gam_report_run")];
        let query = "continue waiting for an existing report operation";
        let companions = workflow_companions(&results, Some(query));
        let resolution = resolve_report_discovery(
            &mut results,
            &companions,
            Some(query),
            Some("reports"),
            true,
            Some(false),
        );
        assert!(results.is_empty());
        assert_eq!(resolution.records[0]["name"], "gam_report_run");
        assert!(resolution.records[0]["before_tool"].is_null());
        assert_eq!(resolution.records[0]["relation"], "condition");
        assert_eq!(
            resolution.callable_alternatives,
            vec!["gam_report_operation_poll"]
        );
        assert_eq!(resolution.records[1]["relation"], "safe_alternative");
        assert_eq!(resolution.records[1]["safe_method"], "GET");
        assert!(
            !resolution.records[1]
                .to_string()
                .contains("automatic_replay")
        );
    }

    #[test]
    fn existing_operation_intent_adds_safe_poll_without_a_ranked_report_start() {
        let mut results = vec![result("gam_soap_trafficking_apply")];
        let query = "check the status of an existing report operation";
        let companions = workflow_companions(&results, Some(query));
        let resolution = resolve_report_discovery(
            &mut results,
            &companions,
            Some(query),
            Some("reports"),
            true,
            Some(false),
        );
        assert_eq!(
            resolution.callable_alternatives,
            vec!["gam_report_operation_poll"]
        );
        assert_eq!(resolution.records.len(), 1);
        assert_eq!(resolution.records[0]["type"], "intent_safe_alternative");
        assert_eq!(resolution.records[0]["name"], "gam_report_operation_poll");
        assert_eq!(results[0].name, "gam_soap_trafficking_apply");
    }

    #[test]
    fn existing_operation_intent_does_not_cross_an_unrelated_explicit_group() {
        let mut results = vec![result("gam_soap_trafficking_apply")];
        let query = "check the status of an existing report operation";
        let companions = workflow_companions(&results, Some(query));
        let resolution = resolve_report_discovery(
            &mut results,
            &companions,
            Some(query),
            Some("trafficking"),
            true,
            Some(false),
        );
        assert!(resolution.callable_alternatives.is_empty());
        assert!(resolution.records.is_empty());
        assert_eq!(results[0].name, "gam_soap_trafficking_apply");
    }

    #[test]
    fn report_run_noun_phrases_are_poll_only_under_write_like_filter() {
        for query in [
            "report run",
            "latest report run",
            "current report run",
            "the latest report run",
            "the report's most recent run",
            "show the current report run",
            "report runs",
            "start the latest report run",
            "latest run of the report",
            "current run for the saved report",
            "show the current run of the report",
            "run of the report",
            "start a report, then poll the current run",
            "start a report and check operation 123",
            "start a report and use run id 123",
            "start a new report then poll the current run",
        ] {
            let mut results = vec![result("gam_report_run")];
            let companions = workflow_companions(&results, Some(query));
            let resolution = resolve_report_discovery(
                &mut results,
                &companions,
                Some(query),
                Some("reports"),
                true,
                Some(false),
            );
            assert!(
                results.is_empty(),
                "report start remained callable: {query}"
            );
            assert_eq!(
                resolution.records[0]["report_intent"], "existing_operation_continuation",
                "noun phrase did not fail closed: {query}"
            );
            assert_eq!(resolution.records[0]["callable_as_tool"], false);
            assert_eq!(
                resolution.callable_alternatives,
                vec!["gam_report_operation_poll"],
                "GET-only continuation missing: {query}"
            );
        }
    }

    #[test]
    fn broad_report_continuation_language_never_classifies_as_a_new_run() {
        for query in [
            "continue the saved report operation",
            "show report run status",
            "resume a report operation",
            "check the campaign delivery report",
            "poll the report operation handle",
            "inspect the report operation handle",
            "monitor operation_name for the report",
            "wait for the existing report operation",
            "networks/123/operations/reports/runs/789",
            "start monitoring the report operation",
            "start checking the report status",
            "start polling the report operation",
            "check run status for the report",
            "monitor the report's latest run",
            "inspect the latest report run",
            "report run",
            "latest report run",
            "current report run",
            "the latest report run",
            "the report's most recent run",
            "show the current report run",
            "report runs",
            "start the latest report run",
            "latest run of the report",
            "current run for the saved report",
            "show the current run of the report",
            "run of the report",
            "start a report, then poll the current run",
            "start a report and check operation 123",
            "start a report and use run id 123",
            "start a new report then poll the current run",
        ] {
            assert_eq!(
                report_discovery_intent(Some(query)),
                ReportDiscoveryIntent::ExistingOperationContinuation,
                "query classified unsafely: {query}"
            );
        }
        assert_eq!(
            report_discovery_intent(Some("start a campaign delivery audit with a saved report")),
            ReportDiscoveryIntent::ExplicitNewRun
        );
        assert_eq!(
            report_discovery_intent(Some("run a saved report and wait for completion")),
            ReportDiscoveryIntent::ExplicitNewRun
        );
        for query in [
            "start a report",
            "run the report",
            "run the current-quarter delivery report",
            "run the latest report",
            "run this report",
            "start my saved report",
            "launch another report",
            "for the current network, run the saved report",
            "launch a saved report",
            "execute the report",
            "start a saved report then monitor its status",
            "run the campaign delivery report and check its status",
            "launch a report and inspect it until completion",
            "execute the report then monitor the result",
            "start a report then poll it until completion",
            "start a report then poll the new run",
            "run the report then show its result",
            "run the report now",
            "launch the saved report and check its status",
        ] {
            assert_eq!(
                report_discovery_intent(Some(query)),
                ReportDiscoveryIntent::ExplicitNewRun,
                "compound new-run query classified unsafely: {query}"
            );
        }
        for query in [
            "start a report then monitor the existing operation",
            "run the report and check operation handle networks/123/operations/reports/runs/789",
        ] {
            assert_eq!(
                report_discovery_intent(Some(query)),
                ReportDiscoveryIntent::ExistingOperationContinuation,
                "explicit existing-operation reference was ignored: {query}"
            );
        }
        for query in [
            "start planning the saved report",
            "execute a query against the report",
            "start server for the report",
            "show me how to run a report",
            "start a report planning session",
            "start a campaign with a report",
            "start a report server",
            "start a report campaign",
            "start a report and email the results",
            "execute line-item operation 123",
            "check line-item operation 123",
            "continue waiting",
        ] {
            assert_eq!(
                report_discovery_intent(Some(query)),
                ReportDiscoveryIntent::Unspecified,
                "unrelated action was treated as a report start: {query}"
            );
        }
    }

    #[test]
    fn excluded_intent_metadata_suppresses_provider_guidance() {
        let inventory = build_tool_inventory().expect("inventory");
        let mut ranked = inventory.search_ranked(
            &ToolSearchFilter {
                query: Some("archive an ad unit".to_string()),
                limit: Some(10),
                ..ToolSearchFilter::default()
            },
            ToolOperation::List,
            &ToolInventoryPolicy::strict(),
        );
        assert!(provider_guidance_allowed(&ranked.match_summary));
        ranked
            .match_summary
            .truncation_reasons
            .push("excluded_query_terms".to_string());
        assert!(!provider_guidance_allowed(&ranked.match_summary));
    }

    #[test]
    fn soap_plan_adds_optional_builder_dependency() {
        let results = [result("gam_soap_trafficking_plan")];
        let companions = workflow_companions(&results, None);
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
        assert_eq!(records[0]["callable_as_tool"], true);
        assert_eq!(records[0]["selection_semantics"], "callable_companion");
    }

    #[test]
    fn soap_apply_adds_builder_then_plan_dependencies() {
        let results = [result("gam_soap_trafficking_apply")];
        let companions = workflow_companions(&results, None);
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
        let companions = workflow_companions(&results, None);
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
        let companions = workflow_companions(&plan_and_apply, None);
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
        let companions = workflow_companions(&complete_sequence, None);
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
            alternative["eligible_tool_counts"],
            serde_json::json!({"total":10,"returned":10,"truncated":false})
        );
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
            alternative["destructive_tool_counts"],
            serde_json::json!({"total":2,"returned":2,"truncated":false})
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
                    "tool_counts":{"total":7,"returned":7,"truncated":false},
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
                    "tool_counts":{"total":2,"returned":2,"truncated":false},
                    "upstream_call":true,
                    "scope_required":"https://www.googleapis.com/auth/admanager.readonly",
                    "manage_scope_required":false
                },
                {
                    "class":"gam_soap_read",
                    "tools":["gam_scratchpad_ingest_soap_line_items"],
                    "tool_counts":{"total":1,"returned":1,"truncated":false},
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
    fn scratchpad_query_intent_recovers_without_internal_group_knowledge() {
        let inventory = build_tool_inventory().expect("inventory");
        let filter = ToolSearchFilter {
            query: Some("open a scratchpad for delivery evidence".to_string()),
            group: None,
            read_only: Some(true),
            limit: Some(20),
        };
        let ranked =
            inventory.search_ranked(&filter, ToolOperation::List, &ToolInventoryPolicy::strict());
        assert!(ranked.match_summary.total_matches > 0);
        let alternatives = supplemental_local_state_alternative_records(
            &inventory,
            &filter,
            &ranked.match_summary,
        );
        assert_eq!(
            alternatives[0]["retry_filter"],
            serde_json::json!({"group":"scratchpad","read_only":false})
        );
        assert_eq!(alternatives[0]["requires_explicit_operator_intent"], true);
        assert_eq!(alternatives[0]["rediscovery_call"]["tool"], "find_tools");
        assert_eq!(
            alternatives[0]["rediscovery_call"]["arguments"],
            serde_json::json!({
                "query":"scratchpad session analysis",
                "group":"scratchpad",
                "read_only":false,
                "limit":10,
                "include_schema":false
            })
        );

        let mut fail_closed_summary = ranked.match_summary;
        fail_closed_summary
            .truncation_reasons
            .push("query_input".to_string());
        assert!(
            supplemental_local_state_alternative_records(
                &inventory,
                &filter,
                &fail_closed_summary,
            )
            .is_empty()
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
    fn stateful_recovery_excludes_report_starts_and_keeps_safe_continuations() {
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
                "continue waiting for an existing report operation",
                "fetch rows from a completed report result",
            ]
        );

        let report_starts = recovery_for_filter(
            &inventory,
            &ToolSearchFilter {
                query: Some("quasar zeppelin".to_string()),
                group: Some("reports".to_string()),
                read_only: Some(false),
                limit: Some(10),
            },
        );
        assert_eq!(recovery_queries(&report_starts), Vec::<&str>::new());
        assert!(!report_starts.to_string().contains("gam_report_run"));
        assert!(
            !report_starts
                .to_string()
                .contains("start a campaign delivery audit")
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
        assert!(
            recovery["retry"]["guidance"]
                .as_array()
                .is_some_and(|guidance| guidance.iter().any(|line| {
                    line.as_str()
                        .is_some_and(|line| line.contains("no more than five tools"))
                }))
        );
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
    fn recovery_caps_dynamic_group_lists_and_reports_truncation() {
        let capabilities = (0..20)
            .map(|index| {
                ToolCapability::new(format!("visible.{index:03}"))
                    .with_group(format!("group-{index:03}"))
                    .with_read_only(true)
            })
            .collect::<Vec<_>>();
        let inventory = ToolInventory::from_capabilities(capabilities).expect("inventory");
        let recovery = recovery_for_filter(
            &inventory,
            &ToolSearchFilter {
                query: Some("quasar zeppelin".to_string()),
                group: None,
                read_only: Some(true),
                limit: Some(20),
            },
        );
        assert_eq!(
            recovery["available_groups"]
                .as_array()
                .expect("bounded groups")
                .len(),
            MAX_RECOVERY_GROUPS
        );
        assert_eq!(
            recovery["available_group_counts"],
            serde_json::json!({"total":20,"returned":16,"truncated":true})
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
