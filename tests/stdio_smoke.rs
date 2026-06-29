use mcp_toolkit_testing::stdio_contract::assert_stdio_tools_list;

#[test]
fn stdio_initializes_and_lists_tools() {
    assert_stdio_tools_list(
        env!("CARGO_BIN_EXE_google_ad_manager_mcp"),
        &[
            "find_tools",
            "gam_get_started",
            "gam_auth_status",
            "gam_auth_login_command",
            "gam_networks_list",
            "gam_network_catalog_list",
            "gam_report_run",
            "gam_report_result_rows",
        ],
    );
}
