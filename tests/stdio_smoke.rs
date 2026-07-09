use mcp_toolkit_testing::stdio_contract::StdioMcpProcess;

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
