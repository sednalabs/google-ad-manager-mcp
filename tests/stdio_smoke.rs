use mcp_toolkit_testing::stdio_contract::StdioMcpProcess;
use serde_json::{Value, json};

#[test]
fn stdio_initializes_and_lists_tools() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    let mut names = process.list_tool_names();
    names.sort();
    let mut expected = vec![
        "find_tools",
        "gam_get_started",
        "gam_auth_status",
        "gam_auth_login_command",
        "gam_networks_list",
        "gam_network_catalog_list",
        "gam_exchange_protection_probe",
        "gam_ad_unit_dependency_probe",
        "gam_ad_unit_retirement_assessment",
        "gam_report_run",
        "gam_report_result_rows",
        "gam_trafficking_tool_matrix",
        "gam_rest_write_plan",
        "gam_rest_write_apply",
        "gam_soap_payload_build",
        "gam_soap_trafficking_plan",
        "gam_soap_trafficking_apply",
        "gam_yield_group_exclusions_preview",
        "gam_yield_group_exclusions_apply",
        "gam_scratchpad_open_session",
        "gam_scratchpad_close_session",
        "gam_scratchpad_list_sessions",
        "gam_scratchpad_list_tables",
        "gam_scratchpad_drop_table",
        "gam_scratchpad_query",
        "gam_scratchpad_ingest_network_catalog",
        "gam_scratchpad_ingest_report_result_rows",
        "gam_scratchpad_ingest_soap_line_items",
        "gam_scratchpad_export_evidence_bundle",
    ];
    expected.sort();
    let expected = expected.into_iter().map(str::to_string).collect::<Vec<_>>();
    assert_eq!(names, expected);
}

#[test]
fn find_tools_is_semantic_compact_and_recoverable() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    for (offset, query, expected_tool) in [
        (
            0,
            "set up and authenticate Google Ad Manager",
            "gam_auth_status",
        ),
        (
            1,
            "inspect ad units and placements",
            "gam_network_catalog_list",
        ),
        (
            2,
            "plan a campaign line item with creatives",
            "gam_soap_trafficking_plan",
        ),
        (
            3,
            "start a campaign delivery audit with a saved report",
            "gam_report_run",
        ),
        (
            4,
            "fetch rows from a completed report result",
            "gam_report_result_rows",
        ),
        (
            5,
            "check exchange and yield protection",
            "gam_exchange_protection_probe",
        ),
        (
            6,
            "assess ad units for retirement",
            "gam_ad_unit_retirement_assessment",
        ),
        (
            7,
            "open a scratchpad session for delivery analysis",
            "gam_scratchpad_open_session",
        ),
        (
            8,
            "ingest line item delivery into an existing scratchpad session",
            "gam_scratchpad_ingest_soap_line_items",
        ),
    ] {
        let response = process.call_tool(
            100 + offset,
            "find_tools",
            json!({"query":query,"limit":10}),
        );
        let data = &response["result"]["structuredContent"]["data"];
        assert!(data.get("schemas").is_none(), "query: {query}");
        assert!(
            data.get("openai_deferred_loading").is_none(),
            "query: {query}"
        );
        assert!(
            data["match_summary"]["total_matches"]
                .as_u64()
                .is_some_and(|count| count > 0)
        );
        assert!(
            data["openai_allowed_tools"]
                .as_array()
                .is_some_and(|tools| tools.contains(&json!(expected_tool))),
            "query '{query}' did not return {expected_tool}: {data}"
        );
        assert!(
            serde_json::to_vec(data)
                .expect("compact discovery serializes")
                .len()
                <= 32 * 1024
        );
    }

    let empty = process.call_tool(
        108,
        "find_tools",
        json!({
            "query":"unknown workflow phrase",
            "group":"missing-group",
            "read_only":true
        }),
    );
    let empty_data = &empty["result"]["structuredContent"]["data"];
    assert_eq!(empty_data["match_summary"]["total_matches"], 0);
    let recovery = empty_data["results"]
        .as_array()
        .and_then(|results| {
            results
                .iter()
                .find(|result| result["type"] == "search_recovery")
        })
        .expect("empty search recovery record");
    assert_eq!(recovery["status"], "no_matches");
    assert_eq!(recovery["active_filter"]["group"], "missing-group");
    assert_eq!(recovery["active_filter"]["read_only"], true);
    assert_eq!(
        recovery["retry"]["example_queries_validated_under_active_filter"],
        true
    );
    assert_eq!(recovery["retry"]["example_queries"], json!([]));
    assert_eq!(
        recovery["available_groups"],
        json!([
            "catalog",
            "discovery",
            "networks",
            "reports",
            "setup",
            "trafficking",
        ])
    );
    assert!(
        serde_json::to_vec(empty_data)
            .expect("recovery serializes")
            .len()
            <= 32 * 1024
    );

    let full = process.call_tool(
        109,
        "find_tools",
        json!({"query":"report rows","limit":2,"include_schema":true}),
    );
    let full_data = &full["result"]["structuredContent"]["data"];
    assert!(
        full_data["schemas"]
            .as_object()
            .is_some_and(|schemas| !schemas.is_empty())
    );
    assert!(full_data.get("openai_deferred_loading").is_some());
}

#[test]
fn find_tools_recovery_examples_execute_under_active_filters() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    let read_response = process.call_tool(
        110,
        "find_tools",
        json!({"query":"quasar zeppelin","read_only":true,"limit":10}),
    );
    let read_data = &read_response["result"]["structuredContent"]["data"];
    let read_recovery = search_recovery(read_data);
    assert_eq!(read_recovery["active_filter"]["group"], Value::Null);
    assert_eq!(read_recovery["active_filter"]["read_only"], true);
    assert_eq!(
        read_recovery["retry"]["example_queries_validated_under_active_filter"],
        true
    );
    let read_queries = recovery_example_queries(read_recovery);
    assert!(!read_queries.is_empty());
    for forbidden in [
        "apply an allowlisted REST write",
        "apply a SOAP trafficking creative operation",
        "apply descendant-safe yield group exclusions",
        "open a scratchpad session for delivery analysis",
        "ingest line item delivery into an existing scratchpad session",
    ] {
        assert!(!read_queries.contains(&forbidden));
    }
    let mut request_id = 200_u64;
    for query in read_queries {
        let response = process.call_tool(
            request_id,
            "find_tools",
            json!({"query":query,"read_only":true,"limit":1}),
        );
        request_id += 1;
        let data = &response["result"]["structuredContent"]["data"];
        let direct = direct_results(data);
        assert_eq!(direct.len(), 1, "query: {query}; data: {data}");
        assert_eq!(direct[0]["read_only"], true);
    }
    assert!(
        serde_json::to_vec(read_data)
            .expect("read-only recovery serializes")
            .len()
            <= 32 * 1024
    );

    let write_response = process.call_tool(
        220,
        "find_tools",
        json!({
            "query":"quasar zeppelin",
            "group":" trafficking ",
            "read_only":false,
            "limit":10
        }),
    );
    let write_data = &write_response["result"]["structuredContent"]["data"];
    let write_recovery = search_recovery(write_data);
    assert_eq!(write_recovery["active_filter"]["group"], "trafficking");
    assert_eq!(write_recovery["active_filter"]["read_only"], false);
    let write_queries = recovery_example_queries(write_recovery);
    assert_eq!(
        write_queries,
        vec![
            "apply an allowlisted REST write",
            "apply a SOAP trafficking creative operation",
            "apply descendant-safe yield group exclusions",
        ]
    );
    for query in write_queries {
        let response = process.call_tool(
            request_id,
            "find_tools",
            json!({
                "query":query,
                "group":"trafficking",
                "read_only":false,
                "limit":1
            }),
        );
        request_id += 1;
        let data = &response["result"]["structuredContent"]["data"];
        let direct = direct_results(data);
        assert_eq!(direct.len(), 1, "query: {query}; data: {data}");
        assert_eq!(direct[0]["group"], "trafficking");
        assert_eq!(direct[0]["read_only"], false);
    }
    assert!(
        serde_json::to_vec(write_data)
            .expect("write recovery serializes")
            .len()
            <= 32 * 1024
    );
}

#[test]
fn find_tools_rejects_zero_and_preserves_toolkit_limit_diagnostics() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    let invalid = process.call_tool(230, "find_tools", json!({"limit":0}));
    let invalid_result = &invalid["result"]["structuredContent"];
    assert_eq!(invalid_result["ok"], false);
    assert_eq!(invalid_result["error"]["code"], "invalid_input");
    assert_eq!(invalid_result["error"]["reason"], "validation_failed");
    assert!(
        invalid_result["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("limit") && message.contains("at least 1"))
    );
    assert!(invalid_result.get("data").is_none());

    let clamped = process.call_tool(
        231,
        "find_tools",
        json!({"group":"trafficking","limit":101}),
    );
    let clamped_data = &clamped["result"]["structuredContent"]["data"];
    assert_eq!(clamped_data["match_summary"]["result_limit"], 100);
    assert!(
        clamped_data["match_summary"]["truncation_reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.contains(&json!("result_limit_clamped")))
    );

    let group_terminal_marker = "oversized-group-terminal-marker";
    let oversized_group = format!("trafficking{}{}", "x".repeat(1024), group_terminal_marker);
    let truncated_group = process.call_tool(
        232,
        "find_tools",
        json!({"query":"quasar zeppelin","group":oversized_group,"read_only":false}),
    );
    let truncated_group_data = &truncated_group["result"]["structuredContent"]["data"];
    let truncated_group_recovery = search_recovery(truncated_group_data);
    assert_eq!(
        truncated_group_recovery["active_filter"]["read_only"],
        false
    );
    assert_discovery_input_fails_closed(
        &truncated_group,
        "group_input",
        group_terminal_marker,
        true,
    );

    let query_terminal_marker = "oversized-query-terminal-marker";
    let oversized_query = format!("quasar {}{}", "z".repeat(64 * 1024), query_terminal_marker);
    let truncated_query = process.call_tool(
        233,
        "find_tools",
        json!({"query":oversized_query,"read_only":true}),
    );
    assert_discovery_input_fails_closed(
        &truncated_query,
        "query_input",
        query_terminal_marker,
        false,
    );
}

#[test]
fn find_tools_pairs_apply_results_with_plan_or_preview() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    for (offset, query, apply_tool, companion_tool) in [
        (
            0,
            "gam rest write apply",
            "gam_rest_write_apply",
            "gam_rest_write_plan",
        ),
        (
            1,
            "gam soap trafficking apply creative",
            "gam_soap_trafficking_apply",
            "gam_soap_trafficking_plan",
        ),
        (
            2,
            "gam yield group exclusions apply",
            "gam_yield_group_exclusions_apply",
            "gam_yield_group_exclusions_preview",
        ),
    ] {
        let response = process.call_tool(
            120 + offset,
            "find_tools",
            json!({"query":query,"group":"trafficking","read_only":false,"limit":10}),
        );
        let data = &response["result"]["structuredContent"]["data"];
        let allowed = data["openai_allowed_tools"]
            .as_array()
            .expect("allowed tools");
        let results = data["results"].as_array().expect("discovery results");
        let direct_results = results
            .iter()
            .filter(|result| result["type"] == "tool")
            .collect::<Vec<_>>();
        assert!(
            allowed.contains(&json!(apply_tool)),
            "query: {query}; data: {data}"
        );
        assert!(
            allowed.contains(&json!(companion_tool)),
            "query: {query}; data: {data}"
        );
        let direct_apply = direct_results
            .iter()
            .find(|result| result["name"] == apply_tool)
            .unwrap_or_else(|| panic!("apply tool was not a direct result: {data}"));
        assert_eq!(direct_apply["read_only"], false);
        assert_eq!(
            direct_apply["risk_posture"]["operation_class"],
            "guarded_apply"
        );
        assert!(results.iter().any(|result| {
            result["type"] == "workflow_companion"
                && result["name"] == companion_tool
                && result["before_tool"] == apply_tool
                && result["required"] == true
                && result["required_for_guided_sequence"] == true
                && result["required_semantics"] == "guided_sequence_compatibility_alias"
                && result["server_call_enforced"] == false
                && result["tool_already_selected"] == false
        }));
        for non_mutating_name in allowed.iter().filter_map(|name| {
            name.as_str()
                .filter(|name| name.ends_with("_plan") || name.ends_with("_preview"))
        }) {
            assert!(
                results.iter().any(|result| {
                    result["type"] == "workflow_companion" && result["name"] == non_mutating_name
                }),
                "non-mutating allowed tool was not a companion: {data}"
            );
            assert!(
                !direct_results
                    .iter()
                    .any(|result| result["name"] == non_mutating_name),
                "non-mutating allowed tool was a direct result: {data}"
            );
        }
    }

    let full = process.call_tool(
        124,
        "find_tools",
        json!({
            "query":"gam rest write apply",
            "group":"trafficking",
            "read_only":false,
            "limit":1,
            "include_schema":true
        }),
    );
    let schemas = full["result"]["structuredContent"]["data"]["schemas"]
        .as_object()
        .expect("full schema map");
    assert!(schemas.contains_key("gam_rest_write_apply"));
    assert!(schemas.contains_key("gam_rest_write_plan"));
}

#[test]
fn find_tools_compact_models_exact_soap_dependencies() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    let plan_response = process.call_tool(
        130,
        "find_tools",
        json!({
            "query":"gam soap trafficking plan forecast line item",
            "group":"trafficking",
            "read_only":true,
            "limit":1
        }),
    );
    let plan_data = &plan_response["result"]["structuredContent"]["data"];
    assert!(plan_data.get("schemas").is_none());
    assert!(plan_data.get("openai_deferred_loading").is_none());
    assert_eq!(
        sorted_direct_names(plan_data),
        vec!["gam_soap_trafficking_plan"]
    );
    assert_eq!(
        sorted_string_values(&plan_data["openai_allowed_tools"]),
        vec!["gam_soap_payload_build", "gam_soap_trafficking_plan"]
    );
    assert_eq!(
        workflow_edges(plan_data),
        vec![json!({
            "relation": "before",
            "name": "gam_soap_payload_build",
            "before_tool": "gam_soap_trafficking_plan",
            "tool_already_selected": false,
            "required": false,
            "required_for_guided_sequence": false,
            "required_semantics": "guided_sequence_compatibility_alias",
            "server_call_enforced": false,
        })]
    );

    let apply_response = process.call_tool(
        131,
        "find_tools",
        json!({
            "query":"gam soap trafficking apply creative",
            "group":"trafficking",
            "read_only":false,
            "limit":1
        }),
    );
    let apply_data = &apply_response["result"]["structuredContent"]["data"];
    assert!(apply_data.get("schemas").is_none());
    assert!(apply_data.get("openai_deferred_loading").is_none());
    assert_eq!(
        sorted_direct_names(apply_data),
        vec!["gam_soap_trafficking_apply"]
    );
    assert_eq!(
        sorted_string_values(&apply_data["openai_allowed_tools"]),
        vec![
            "gam_soap_payload_build",
            "gam_soap_trafficking_apply",
            "gam_soap_trafficking_plan",
        ]
    );
    assert_eq!(
        workflow_edges(apply_data),
        vec![
            json!({
                "relation": "before",
                "name": "gam_soap_payload_build",
                "before_tool": "gam_soap_trafficking_plan",
                "tool_already_selected": false,
                "required": false,
                "required_for_guided_sequence": false,
                "required_semantics": "guided_sequence_compatibility_alias",
                "server_call_enforced": false,
            }),
            json!({
                "relation": "before",
                "name": "gam_soap_trafficking_plan",
                "before_tool": "gam_soap_trafficking_apply",
                "tool_already_selected": false,
                "required": true,
                "required_for_guided_sequence": true,
                "required_semantics": "guided_sequence_compatibility_alias",
                "server_call_enforced": false,
            }),
        ]
    );

    let group_response = process.call_tool(
        132,
        "find_tools",
        json!({"group":"trafficking","limit":100}),
    );
    let group_data = &group_response["result"]["structuredContent"]["data"];
    let group_allowed = group_data["openai_allowed_tools"]
        .as_array()
        .expect("trafficking allowed tools");
    for tool in [
        "gam_soap_payload_build",
        "gam_soap_trafficking_plan",
        "gam_soap_trafficking_apply",
    ] {
        assert!(
            group_allowed.contains(&json!(tool)),
            "full trafficking discovery omitted {tool}: {group_data}"
        );
    }
    let group_edges = workflow_edges(group_data);
    assert!(group_edges.contains(&json!({
        "relation": "before",
        "name": "gam_soap_payload_build",
        "before_tool": "gam_soap_trafficking_plan",
        "tool_already_selected": true,
        "required": false,
        "required_for_guided_sequence": false,
        "required_semantics": "guided_sequence_compatibility_alias",
        "server_call_enforced": false,
    })));
    assert!(group_edges.contains(&json!({
        "relation": "before",
        "name": "gam_soap_trafficking_plan",
        "before_tool": "gam_soap_trafficking_apply",
        "tool_already_selected": true,
        "required": true,
        "required_for_guided_sequence": true,
        "required_semantics": "guided_sequence_compatibility_alias",
        "server_call_enforced": false,
    })));
    assert_eq!(group_data["compact_summary"]["truncated"], false);
    assert!(
        serde_json::to_vec(group_data)
            .expect("full trafficking discovery serializes")
            .len()
            <= 32 * 1024
    );
}

#[test]
fn find_tools_full_schema_includes_exact_soap_dependency_tools() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    let plan_response = process.call_tool(
        132,
        "find_tools",
        json!({
            "query":"gam soap trafficking plan forecast line item",
            "group":"trafficking",
            "read_only":true,
            "limit":1,
            "include_schema":true
        }),
    );
    let plan_data = &plan_response["result"]["structuredContent"]["data"];
    assert_eq!(
        sorted_schema_names(plan_data),
        vec!["gam_soap_payload_build", "gam_soap_trafficking_plan"]
    );

    let apply_response = process.call_tool(
        133,
        "find_tools",
        json!({
            "query":"gam soap trafficking apply creative",
            "group":"trafficking",
            "read_only":false,
            "limit":1,
            "include_schema":true
        }),
    );
    let apply_data = &apply_response["result"]["structuredContent"]["data"];
    assert_eq!(
        sorted_schema_names(apply_data),
        vec![
            "gam_soap_payload_build",
            "gam_soap_trafficking_apply",
            "gam_soap_trafficking_plan",
        ]
    );
}

#[test]
fn find_tools_broad_trafficking_keeps_edges_without_duplicate_injection() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    let response = process.call_tool(
        134,
        "find_tools",
        json!({
            "group":"trafficking",
            "limit":100,
            "include_schema":true
        }),
    );
    let data = &response["result"]["structuredContent"]["data"];
    let expected_names = vec![
        "gam_rest_write_apply",
        "gam_rest_write_plan",
        "gam_soap_payload_build",
        "gam_soap_trafficking_apply",
        "gam_soap_trafficking_plan",
        "gam_trafficking_tool_matrix",
        "gam_yield_group_exclusions_apply",
        "gam_yield_group_exclusions_preview",
    ];
    assert_eq!(sorted_direct_names(data), expected_names);
    assert_eq!(
        sorted_string_values(&data["openai_allowed_tools"]),
        expected_names
    );
    assert_eq!(sorted_schema_names(data), expected_names);
    assert_eq!(
        workflow_edges(data),
        vec![
            json!({
                "relation": "before",
                "name": "gam_rest_write_plan",
                "before_tool": "gam_rest_write_apply",
                "tool_already_selected": true,
                "required": true,
                "required_for_guided_sequence": true,
                "required_semantics": "guided_sequence_compatibility_alias",
                "server_call_enforced": false,
            }),
            json!({
                "relation": "before",
                "name": "gam_soap_payload_build",
                "before_tool": "gam_soap_trafficking_plan",
                "tool_already_selected": true,
                "required": false,
                "required_for_guided_sequence": false,
                "required_semantics": "guided_sequence_compatibility_alias",
                "server_call_enforced": false,
            }),
            json!({
                "relation": "before",
                "name": "gam_soap_trafficking_plan",
                "before_tool": "gam_soap_trafficking_apply",
                "tool_already_selected": true,
                "required": true,
                "required_for_guided_sequence": true,
                "required_semantics": "guided_sequence_compatibility_alias",
                "server_call_enforced": false,
            }),
            json!({
                "relation": "before",
                "name": "gam_yield_group_exclusions_preview",
                "before_tool": "gam_yield_group_exclusions_apply",
                "tool_already_selected": true,
                "required": true,
                "required_for_guided_sequence": true,
                "required_semantics": "guided_sequence_compatibility_alias",
                "server_call_enforced": false,
            }),
        ]
    );
}

#[test]
fn find_tools_read_only_filter_partitions_all_scratchpad_tools() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    let read_only_response = process.call_tool(
        125,
        "find_tools",
        json!({
            "query":"scratchpad",
            "group":"scratchpad",
            "read_only":true,
            "limit":100
        }),
    );
    let read_only_data = &read_only_response["result"]["structuredContent"]["data"];
    assert_eq!(read_only_data["openai_allowed_tools"], json!([]));

    let mutating_response = process.call_tool(
        126,
        "find_tools",
        json!({
            "query":"scratchpad",
            "group":"scratchpad",
            "read_only":false,
            "limit":100
        }),
    );
    let mutating_data = &mutating_response["result"]["structuredContent"]["data"];
    let mut mutating_allowed = mutating_data["openai_allowed_tools"]
        .as_array()
        .expect("allowed tools")
        .iter()
        .map(|name| name.as_str().expect("allowed tool name"))
        .collect::<Vec<_>>();
    mutating_allowed.sort_unstable();
    assert_eq!(
        mutating_allowed,
        vec![
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
        ]
    );
}

#[test]
fn find_tools_read_only_filter_includes_plans_and_previews() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    for (offset, query, expected_tool) in [
        (0, "gam rest write plan", "gam_rest_write_plan"),
        (1, "gam soap trafficking plan", "gam_soap_trafficking_plan"),
        (
            2,
            "gam yield group exclusions preview",
            "gam_yield_group_exclusions_preview",
        ),
    ] {
        let response = process.call_tool(
            127 + offset,
            "find_tools",
            json!({
                "query":query,
                "group":"trafficking",
                "read_only":true,
                "limit":10
            }),
        );
        let data = &response["result"]["structuredContent"]["data"];
        assert!(
            data["openai_allowed_tools"]
                .as_array()
                .is_some_and(|allowed| allowed.contains(&json!(expected_tool))),
            "query '{query}' did not return {expected_tool}: {data}"
        );
    }
}

#[test]
fn probe_handlers_reject_invalid_soap_versions_before_provider_access() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    for (id, tool_name, arguments) in [
        (
            3,
            "gam_exchange_protection_probe",
            json!({
                "network_code": "1015422",
                "ad_unit_codes": ["Section_Page_LS"],
                "api_version": "invalid"
            }),
        ),
        (
            4,
            "gam_ad_unit_dependency_probe",
            json!({
                "network_code": "1015422",
                "ad_unit_ids": ["200"],
                "api_version": "invalid"
            }),
        ),
    ] {
        let response = process.call_tool(id, tool_name, arguments);
        assert!(
            response.get("error").is_none() || response["error"].is_null(),
            "JSON-RPC error: {response}"
        );
        let result = &response["result"]["structuredContent"];
        assert!(
            serde_json::to_vec(result)
                .expect("serialize public probe error")
                .len()
                < 20 * 1024
        );
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "invalid_input");
        assert_eq!(result["error"]["reason"], "validation_failed");
        assert!(
            result["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("api_version"))
        );
        assert!(result.get("data").is_none());
    }
}

#[test]
fn probe_handler_validation_errors_do_not_echo_oversized_inputs() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    let oversized_version = "v".repeat(64 * 1024);
    let response = process.call_tool(
        5,
        "gam_ad_unit_dependency_probe",
        json!({
            "network_code": "1234567",
            "ad_unit_ids": ["200"],
            "api_version": oversized_version,
        }),
    );
    let result = &response["result"]["structuredContent"];
    let encoded = serde_json::to_vec(result).expect("serialize bounded public error");
    let encoded_transport =
        serde_json::to_vec(&response["result"]).expect("serialize bounded public transport");

    assert!(encoded.len() < 20 * 1024);
    assert!(encoded_transport.len() < 20 * 1024);
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "invalid_input");
    assert!(
        !String::from_utf8(encoded)
            .expect("UTF-8 response")
            .contains(&"v".repeat(512))
    );
}

#[test]
fn retirement_assessment_rejects_noncanonical_targets_at_stdio_boundary() {
    let mut process = StdioMcpProcess::start(env!("CARGO_BIN_EXE_google-ad-manager-mcp"));
    for (id, network_code, ad_unit_ids) in [
        (6, "01234567", json!(["200"])),
        (7, "1234567", json!([])),
        (8, "1234567", json!(["0200"])),
        (9, "1234567", json!(["200", "200"])),
    ] {
        let response = process.call_tool(
            id,
            "gam_ad_unit_retirement_assessment",
            json!({"network_code":network_code,"ad_unit_ids":ad_unit_ids}),
        );
        let result = &response["result"]["structuredContent"];
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "invalid_input");
        assert!(result.get("data").is_none());
        assert!(
            serde_json::to_vec(result)
                .expect("serialize retirement validation error")
                .len()
                < 20 * 1024
        );
    }
}

fn search_recovery(data: &Value) -> &Value {
    data["results"]
        .as_array()
        .expect("discovery results")
        .iter()
        .find(|result| result["type"] == "search_recovery")
        .expect("search recovery")
}

fn assert_discovery_input_fails_closed(
    response: &Value,
    reason_code: &str,
    terminal_marker: &str,
    expect_empty_examples: bool,
) {
    let data = &response["result"]["structuredContent"]["data"];
    assert_eq!(data["match_summary"]["total_matches"], 0);
    assert!(
        data["match_summary"]["truncation_reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.contains(&json!(reason_code)))
    );
    assert_eq!(data["openai_allowed_tools"], json!([]));

    let recovery = search_recovery(data);
    assert_eq!(recovery["fail_closed"], true);
    assert!(
        recovery["reason_codes"]
            .as_array()
            .is_some_and(|reasons| reasons.contains(&json!(reason_code)))
    );
    assert_eq!(
        recovery["retry"]["example_queries_validated_under_active_filter"],
        true
    );
    if expect_empty_examples {
        assert!(recovery_example_queries(recovery).is_empty());
    }

    let compact_data = serde_json::to_string(data).expect("compact discovery data serializes");
    assert!(compact_data.len() <= 32 * 1024);
    let full_result = serde_json::to_string(&response["result"])
        .expect("complete MCP discovery result serializes");
    assert!(!full_result.contains(terminal_marker));
}

fn recovery_example_queries(recovery: &Value) -> Vec<&str> {
    recovery["retry"]["example_queries"]
        .as_array()
        .expect("example queries")
        .iter()
        .map(|query| query.as_str().expect("example query"))
        .collect()
}

fn direct_results(data: &Value) -> Vec<&Value> {
    data["results"]
        .as_array()
        .expect("discovery results")
        .iter()
        .filter(|result| result["type"] == "tool")
        .collect()
}

fn sorted_direct_names(data: &Value) -> Vec<&str> {
    let mut names = direct_results(data)
        .into_iter()
        .map(|result| result["name"].as_str().expect("direct tool name"))
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn sorted_string_values(value: &Value) -> Vec<&str> {
    let mut values = value
        .as_array()
        .expect("string array")
        .iter()
        .map(|value| value.as_str().expect("string value"))
        .collect::<Vec<_>>();
    values.sort_unstable();
    values
}

fn sorted_schema_names(data: &Value) -> Vec<&str> {
    let mut names = data["schemas"]
        .as_object()
        .expect("schema map")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn workflow_edges(data: &Value) -> Vec<Value> {
    data["results"]
        .as_array()
        .expect("discovery results")
        .iter()
        .filter(|result| result["type"] == "workflow_companion")
        .map(|result| {
            json!({
                "relation": result["relation"].clone(),
                "name": result["name"].clone(),
                "before_tool": result["before_tool"].clone(),
                "tool_already_selected": result["tool_already_selected"].clone(),
                "required": result["required"].clone(),
                "required_for_guided_sequence": result["required_for_guided_sequence"].clone(),
                "required_semantics": result["required_semantics"].clone(),
                "server_call_enforced": result["server_call_enforced"].clone(),
            })
        })
        .collect()
}
