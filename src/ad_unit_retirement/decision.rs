use serde_json::{Value, json};

use crate::fingerprint::stable_fingerprint;

use super::inventory::proof_state;

pub(super) fn recommendation(identity: &Value, descendants: &Value, evidence: &Value) -> Value {
    let mut states = vec![
        ("identity".to_string(), proof_state(identity).to_string()),
        (
            "descendants".to_string(),
            proof_state(descendants).to_string(),
        ),
    ];
    for surface in [
        "dependency",
        "delivery",
        "exchange_protection",
        "site_contract",
        "telemetry",
    ] {
        states.push((
            surface.to_string(),
            evidence
                .get(surface)
                .and_then(|value| value.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("not_run")
                .to_string(),
        ));
    }
    let blocking = states
        .iter()
        .filter(|(_, state)| state == "complete_blocked")
        .map(|(surface, _)| surface.clone())
        .collect::<Vec<_>>();
    let incomplete = states
        .iter()
        .filter(|(_, state)| state != "complete_clear" && state != "complete_blocked")
        .map(|(surface, state)| json!({"surface": surface, "state": state}))
        .collect::<Vec<_>>();
    let evidence_complete = states.iter().all(|(_, state)| state == "complete_clear");
    let decision = if !blocking.is_empty() {
        "blocked_by_dependencies_or_activity"
    } else if evidence_complete {
        "evidence_complete_operator_review_required"
    } else {
        "not_eligible_incomplete_evidence"
    };
    let mut next_actions = states
        .iter()
        .filter_map(|(surface, state)| next_action(surface, state))
        .collect::<Vec<_>>();
    if evidence_complete {
        next_actions.push(json!({
            "surface": "operator_review",
            "action": "obtain explicit operator approval outside this read-only assessor before any guarded retirement workflow"
        }));
    }
    let assessment_input = json!({
        "identity": identity,
        "descendants": descendants,
        "evidence": evidence,
    });
    json!({
        "decision": decision,
        "evidence_summary_complete": evidence_complete,
        "automated_retirement_eligible": false,
        "operator_review_required": true,
        "blocking_surfaces": blocking,
        "incomplete_surfaces": incomplete,
        "next_actions": next_actions,
        "requires_child_first_sequence": descendants.get("requires_child_first_sequence").and_then(Value::as_bool).unwrap_or(false),
        "required_child_first_target_order": descendants.get("required_child_first_target_order").cloned().unwrap_or_else(|| json!([])),
        "assessment_fingerprint": stable_fingerprint(&assessment_input.to_string()),
        "not_an_archive_authorization": true,
    })
}

fn next_action(surface: &str, state: &str) -> Option<Value> {
    let action = match state {
        "complete_clear" => return None,
        "complete_blocked" => "resolve the observed dependency or activity before reassessment",
        "partial_capped" => "rerun the source proof without a result cap",
        "blocked_permission" => "restore read permission and rerun the source proof",
        "blocked_read" => "resolve the upstream read failure and rerun the source proof",
        "unsupported_surface" => "obtain an authoritative manual or alternate-source proof",
        "invalid_binding" => "bind a current source/version/network/exact-target receipt",
        "stale" => "refresh the evidence within its configured TTL",
        "manual_ui_proof_required" => {
            "review the required GAM UI-only protection surfaces and provide a current receipt recording that review"
        }
        _ => "run the required source proof and attach its exact-target receipt",
    };
    Some(json!({"surface": surface, "action": action}))
}
