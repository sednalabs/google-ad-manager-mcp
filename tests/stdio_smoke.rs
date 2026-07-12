use mcp_toolkit_testing::stdio_contract::StdioMcpProcess;
use serde_json::json;

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
            "audit campaign delivery and report rows",
            "gam_report_result_rows",
        ),
        (
            4,
            "check exchange and yield protection",
            "gam_exchange_protection_probe",
        ),
        (
            5,
            "assess ad units for retirement",
            "gam_ad_unit_retirement_assessment",
        ),
        (
            6,
            "analyze line item delivery in a scratchpad",
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
    assert!(
        recovery["available_groups"]
            .as_array()
            .is_some_and(|groups| !groups.is_empty())
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
        assert!(
            allowed.contains(&json!(apply_tool)),
            "query: {query}; data: {data}"
        );
        assert!(
            allowed.contains(&json!(companion_tool)),
            "query: {query}; data: {data}"
        );
        assert!(data["results"].as_array().is_some_and(|results| {
            results.iter().any(|result| {
                result["type"] == "workflow_companion"
                    && result["name"] == companion_tool
                    && result["before_tool"] == apply_tool
                    && result["required"] == true
            })
        }));
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
