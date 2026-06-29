//! Configuration and CLI for the stdio server.

use std::time::Duration;

use clap::Parser;

use crate::AdManagerError;

pub const DEFAULT_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/admanager.readonly";
pub const MANAGE_SCOPE: &str = "https://www.googleapis.com/auth/admanager";
const DEFAULT_API_BASE_URL: &str = "https://admanager.googleapis.com/v1";

#[derive(Debug, Clone, Parser)]
#[command(name = "google-ad-manager-mcp")]
#[command(about = "Google Ad Manager read-only MCP server")]
pub struct Cli {
    /// Print the exported tool names as JSON and exit.
    #[arg(long)]
    pub print_tools: bool,

    /// Print the exported tool schema snapshot as JSON and exit.
    #[arg(long)]
    pub print_tool_schema: bool,

    /// OAuth scope requested from Google credentials.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_SCOPE", default_value = DEFAULT_READONLY_SCOPE)]
    pub scope: String,

    /// Optional x-goog-user-project header value for quota/billing.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT")]
    pub quota_project: Option<String>,

    /// Optional server-specific service account JSON file path.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH")]
    pub service_account_json_path: Option<String>,

    /// Optional server-specific raw service account JSON.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON")]
    pub service_account_json: Option<String>,

    /// Upstream Ad Manager HTTP timeout in milliseconds.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_HTTP_TIMEOUT_MS",
        default_value_t = 15000
    )]
    pub http_timeout_ms: u64,

    /// Default upstream API base URL.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_API_BASE_URL", default_value = DEFAULT_API_BASE_URL)]
    pub api_base_url: String,

    /// Default report polling timeout in milliseconds.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_REPORT_POLL_TIMEOUT_MS",
        default_value_t = 300000
    )]
    pub report_poll_timeout_ms: u64,

    /// Initial report polling interval in milliseconds.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_REPORT_POLL_INITIAL_INTERVAL_MS",
        default_value_t = 5000
    )]
    pub report_poll_initial_interval_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub print_tools: bool,
    pub print_tool_schema: bool,
    pub scope: String,
    pub quota_project: Option<String>,
    pub service_account_json_path: Option<String>,
    pub service_account_json: Option<String>,
    pub http_timeout: Duration,
    pub api_base_url: String,
    pub report_poll_timeout: Duration,
    pub report_poll_initial_interval: Duration,
}

impl Settings {
    /// Validates CLI inputs and produces runtime settings.
    ///
    /// # Errors
    /// Returns an error when a required string is blank or an API URL is not
    /// HTTPS.
    ///
    /// # Security
    /// The API base URL is restricted to HTTPS so operator mistakes do not send
    /// bearer tokens to insecure endpoints.
    pub fn from_cli(cli: Cli) -> Result<Self, AdManagerError> {
        let scope = normalize_required("scope", cli.scope)?;
        let api_base_url = normalize_required("api_base_url", cli.api_base_url)?;
        if !api_base_url.starts_with("https://") {
            return Err(AdManagerError::invalid(
                "api_base_url",
                "must start with https://",
            ));
        }
        Ok(Self {
            print_tools: cli.print_tools,
            print_tool_schema: cli.print_tool_schema,
            scope,
            quota_project: normalize_optional(cli.quota_project),
            service_account_json_path: normalize_optional(cli.service_account_json_path),
            service_account_json: normalize_optional(cli.service_account_json),
            http_timeout: Duration::from_millis(cli.http_timeout_ms.max(1)),
            api_base_url,
            report_poll_timeout: Duration::from_millis(cli.report_poll_timeout_ms.max(1)),
            report_poll_initial_interval: Duration::from_millis(
                cli.report_poll_initial_interval_ms.max(250),
            ),
        })
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            print_tools: false,
            print_tool_schema: false,
            scope: DEFAULT_READONLY_SCOPE.to_string(),
            quota_project: None,
            service_account_json_path: None,
            service_account_json: None,
            http_timeout: Duration::from_millis(15_000),
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            report_poll_timeout: Duration::from_millis(300_000),
            report_poll_initial_interval: Duration::from_millis(5_000),
        }
    }
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_required(field: &'static str, value: String) -> Result<String, AdManagerError> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Err(AdManagerError::invalid(field, "must not be empty"));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_READONLY_SCOPE, Settings};

    #[test]
    fn defaults_are_read_only_and_https() {
        let settings = Settings::default();
        assert_eq!(settings.scope, DEFAULT_READONLY_SCOPE);
        assert!(settings.api_base_url.starts_with("https://"));
    }
}
