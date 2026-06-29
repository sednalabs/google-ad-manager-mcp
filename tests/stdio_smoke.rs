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
        "gam_report_run",
        "gam_report_result_rows",
    ];
    expected.sort();
    let expected = expected.into_iter().map(str::to_string).collect::<Vec<_>>();
    assert_eq!(names, expected);
}
