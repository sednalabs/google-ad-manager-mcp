use serde_json::{Value, json};

const SURFACES: [(&str, &str); 7] = [
    ("identity", "proof_state"),
    ("descendants", "proof_state"),
    ("dependency", "state"),
    ("delivery", "state"),
    ("exchange_protection", "state"),
    ("site_contract", "state"),
    ("telemetry", "state"),
];

pub(super) fn recommendation(
    identity: &Value,
    descendants: &Value,
    evidence: &Value,
    assessment_fingerprint: &str,
) -> Value {
    let states = SURFACES
        .iter()
        .map(|(surface, field)| {
            let source = match *surface {
                "identity" => identity,
                "descendants" => descendants,
                _ => evidence.get(*surface).unwrap_or(&Value::Null),
            };
            (
                *surface,
                source
                    .get(*field)
                    .and_then(Value::as_str)
                    .unwrap_or("not_run"),
            )
        })
        .collect::<Vec<_>>();

    let observed_active_descendants = descendants
        .get("blocking_external_descendant_count")
        .and_then(Value::as_u64)
        .is_some_and(|count| count > 0);
    let blocking_surfaces = states
        .iter()
        .filter(|(surface, state)| {
            matches!(*state, "complete_blocked" | "partial_blocked")
                || (*surface == "descendants" && observed_active_descendants)
        })
        .map(|(surface, _)| Value::String((*surface).to_string()))
        .collect::<Vec<_>>();

    let incomplete_surfaces = states
        .iter()
        .filter(|(_, state)| !matches!(*state, "complete_clear" | "complete_blocked"))
        .map(|(surface, state)| json!({"surface": surface, "state": state}))
        .collect::<Vec<_>>();
    let all_required_surfaces_complete =
        blocking_surfaces.is_empty() && states.iter().all(|(_, state)| *state == "complete_clear");
    let decision = if !blocking_surfaces.is_empty() {
        "blocked_by_current_state_or_evidence"
    } else if all_required_surfaces_complete {
        "evidence_complete_operator_review_required"
    } else {
        "not_eligible_incomplete_evidence"
    };
    let reason = match decision {
        "blocked_by_current_state_or_evidence" => {
            "At least one required surface reports a confirmed blocker; resolve it and rerun the assessment."
        }
        "evidence_complete_operator_review_required" => {
            "Every required surface is complete and clear, but explicit operator review remains required before any separate guarded retirement workflow."
        }
        _ => "No confirmed blocker was found, but one or more required surfaces are incomplete.",
    };

    let mut next_actions = states
        .iter()
        .filter_map(|(surface, state)| next_action(surface, state))
        .collect::<Vec<_>>();
    if observed_active_descendants
        && !states.iter().any(|(surface, state)| {
            *surface == "descendants" && matches!(*state, "complete_blocked" | "partial_blocked")
        })
    {
        next_actions.push(json!({
            "surface": "descendants",
            "action": "archive or retarget the observed active descendants before reassessment"
        }));
    }
    if all_required_surfaces_complete {
        next_actions.push(json!({
            "surface": "operator_review",
            "action": "obtain explicit operator approval outside this read-only assessor before any guarded retirement workflow"
        }));
    }

    json!({
        "decision": decision,
        "reason": reason,
        "all_required_surfaces_complete": all_required_surfaces_complete,
        "automated_retirement_eligible": false,
        "safe_to_archive_or_retire": false,
        "operator_review_required": true,
        "blocking_surfaces": blocking_surfaces,
        "incomplete_surfaces": incomplete_surfaces,
        "next_actions": next_actions,
        "requires_child_first_sequence": descendants
            .get("requires_child_first_sequence")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "required_child_first_target_order": descendants
            .get("required_child_first_target_order")
            .cloned()
            .unwrap_or_else(|| json!([])),
        "assessment_fingerprint": assessment_fingerprint,
        "not_an_archive_authorization": true,
    })
}

fn next_action(surface: &str, state: &str) -> Option<Value> {
    let action = match state {
        "complete_clear" => return None,
        "complete_blocked" => "resolve the observed blocker before reassessment",
        "partial_blocked" => "resolve the observed blocker and rerun the incomplete source proof",
        "partial_capped" => "rerun the source proof without a result cap",
        "blocked_auth" => "restore authentication and rerun the source proof",
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
