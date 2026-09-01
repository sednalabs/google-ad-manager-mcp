const ARCHITECTURE: &str = include_str!("../docs/ARCHITECTURE.md");

#[test]
fn architecture_uses_runtime_tool_inventory_as_authority() {
    for required_contract in [
        "google-ad-manager-mcp --print-tools",
        "`--print-tool-schema`",
        "`inventory_matches_exported_tool_names`",
        "intentionally non-exhaustive",
    ] {
        assert!(
            ARCHITECTURE.contains(required_contract),
            "architecture is missing runtime inventory contract: {required_contract}"
        );
    }

    assert!(
        !ARCHITECTURE.contains("The initial first-class tool set is:"),
        "architecture must not duplicate the evolving runtime tool inventory"
    );
}
