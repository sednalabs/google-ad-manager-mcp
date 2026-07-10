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
    assert!(!String::from_utf8(encoded).expect("UTF-8 response").contains(&"v".repeat(512)));
}
