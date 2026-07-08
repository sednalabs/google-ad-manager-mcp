//! Configuration and CLI for the stdio server.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use crate::AdManagerError;
use clap::{Args, Parser, Subcommand};
use mcp_toolkit_core::guarded_action::GuardedActionRuntimeMode;

pub const DEFAULT_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/admanager.readonly";
pub const MANAGE_SCOPE: &str = "https://www.googleapis.com/auth/admanager";
pub const GCLOUD_ADC_REQUIRED_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const DEFAULT_API_BASE_URL: &str = "https://admanager.googleapis.com/v1";
const DEFAULT_SOAP_BASE_URL: &str = "https://ads.google.com/apis/ads/publisher";
const DEFAULT_WRITE_MODE: &str = "preview_only";
const DEFAULT_SCRATCHPAD_SESSION_TTL_SECS: u64 = 900;
const DEFAULT_SCRATCHPAD_MAX_SESSIONS: usize = 64;
const DEFAULT_SCRATCHPAD_MAX_TABLES_PER_SESSION: usize = 32;
const DEFAULT_SCRATCHPAD_MAX_ROWS_PER_SESSION: usize = 1_000_000;
const DEFAULT_SCRATCHPAD_MAX_MEMORY_MB: usize = 256;
const DEFAULT_SCRATCHPAD_QUERY_TIMEOUT_MS: u64 = 15_000;
const DEFAULT_SCRATCHPAD_MAX_SQL_BYTES: usize = 65_536;

#[derive(Debug, Clone, Parser)]
#[command(name = "google-ad-manager-mcp")]
#[command(about = "Google Ad Manager MCP server")]
#[command(version)]
pub struct Cli {
    /// Print the exported tool names as JSON and exit.
    #[arg(long)]
    pub print_tools: bool,

    /// Print the exported tool schema snapshot as JSON and exit.
    #[arg(long)]
    pub print_tool_schema: bool,

    /// OAuth scope requested from Google credentials.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_SCOPE", global = true, default_value = DEFAULT_READONLY_SCOPE)]
    pub scope: String,

    /// Optional x-goog-user-project header value for quota/billing.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_QUOTA_PROJECT", global = true)]
    pub quota_project: Option<String>,

    /// Use conventional shared gcloud ADC instead of the server-specific ADC file.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_SHARED_ADC", global = true)]
    pub shared_adc: bool,

    /// Optional server-specific service account JSON file path.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON_PATH",
        global = true
    )]
    pub service_account_json_path: Option<String>,

    /// Optional server-specific raw service account JSON.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON",
        global = true
    )]
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

    /// Default upstream Ad Manager SOAP base URL, without the version or service segment.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_SOAP_BASE_URL", default_value = DEFAULT_SOAP_BASE_URL)]
    pub soap_base_url: String,

    /// Runtime mode for Ad Manager write tools: read_only, preview_only, or enabled.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_WRITE_MODE",
        default_value = DEFAULT_WRITE_MODE
    )]
    pub write_mode: String,

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

    /// Scratchpad session TTL in seconds.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_SESSION_TTL_SECS",
        default_value_t = DEFAULT_SCRATCHPAD_SESSION_TTL_SECS
    )]
    pub scratchpad_session_ttl_secs: u64,

    /// Maximum number of active scratchpad sessions.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_SESSIONS",
        default_value_t = DEFAULT_SCRATCHPAD_MAX_SESSIONS
    )]
    pub scratchpad_max_sessions: usize,

    /// Maximum number of tables tracked per scratchpad session.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_TABLES_PER_SESSION",
        default_value_t = DEFAULT_SCRATCHPAD_MAX_TABLES_PER_SESSION
    )]
    pub scratchpad_max_tables_per_session: usize,

    /// Maximum number of rows tracked per scratchpad session.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_ROWS_PER_SESSION",
        default_value_t = DEFAULT_SCRATCHPAD_MAX_ROWS_PER_SESSION
    )]
    pub scratchpad_max_rows_per_session: usize,

    /// Maximum DuckDB memory limit in MB per scratchpad session connection.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_MEMORY_MB",
        default_value_t = DEFAULT_SCRATCHPAD_MAX_MEMORY_MB
    )]
    pub scratchpad_max_memory_mb: usize,

    /// Scratchpad query timeout in milliseconds.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_QUERY_TIMEOUT_MS",
        default_value_t = DEFAULT_SCRATCHPAD_QUERY_TIMEOUT_MS
    )]
    pub scratchpad_query_timeout_ms: u64,

    /// Maximum SQL payload size accepted by scratchpad query guardrails.
    #[arg(
        long,
        env = "GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_MAX_SQL_BYTES",
        default_value_t = DEFAULT_SCRATCHPAD_MAX_SQL_BYTES
    )]
    pub scratchpad_max_sql_bytes: usize,

    /// Optional scratchpad root directory. Defaults to the OS temp directory.
    #[arg(long, env = "GOOGLE_AD_MANAGER_MCP_SCRATCHPAD_ROOT_DIR")]
    pub scratchpad_root_dir: Option<PathBuf>,

    /// Optional command. Omit to run the stdio MCP server.
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CliCommand {
    /// Run the stdio MCP server. This is also the default when no command is supplied.
    Serve,
    /// Login, verify, and diagnose Google Ad Manager credentials.
    Auth(AuthCli),
}

#[derive(Debug, Clone, Args)]
pub struct AuthCli {
    #[command(subcommand)]
    pub command: AuthSubcommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum AuthSubcommand {
    /// Run the browser-based gcloud Application Default Credentials login flow.
    Login(AuthLoginArgs),
    /// Print the exact gcloud login command without running it.
    Command(AuthCommandArgs),
    /// Show the configured credential source and optional Ad Manager API verification result.
    Status(AuthStatusCliArgs),
    /// Check the local auth environment and suggest the next action.
    Doctor(AuthDoctorArgs),
}

#[derive(Debug, Clone, Args)]
pub struct AuthLoginArgs {
    /// Print a browser URL instead of launching a browser where supported by gcloud.
    #[arg(long)]
    pub headless: bool,

    /// Optional downloaded Google OAuth client id JSON for gcloud ADC login.
    #[arg(long)]
    pub client_id_file: Option<PathBuf>,

    /// Optional quota project to set after successful login.
    #[arg(long)]
    pub quota_project: Option<String>,

    /// Request the write-capable Ad Manager manage scope instead of the configured scope.
    #[arg(long, alias = "write-scope")]
    pub manage_scope: bool,

    /// Print the command that would run, without invoking gcloud.
    #[arg(long)]
    pub dry_run: bool,

    /// Skip post-login Ad Manager API verification.
    #[arg(long)]
    pub no_verify: bool,

    /// Use the conventional shared gcloud ADC file instead of a server-specific file.
    #[arg(long)]
    pub shared_adc: bool,
}

#[derive(Debug, Clone, Args)]
pub struct AuthCommandArgs {
    /// Include the headless browser flag in the printed gcloud command.
    #[arg(long)]
    pub headless: bool,

    /// Optional downloaded Google OAuth client id JSON for gcloud ADC login.
    #[arg(long)]
    pub client_id_file: Option<PathBuf>,

    /// Request the write-capable Ad Manager manage scope instead of the configured scope.
    #[arg(long, alias = "write-scope")]
    pub manage_scope: bool,

    /// Print commands for the conventional shared gcloud ADC file.
    #[arg(long)]
    pub shared_adc: bool,
}

#[derive(Debug, Clone, Args)]
pub struct AuthStatusCliArgs {
    /// Acquire a Google access token and call networks.list. The token is never printed.
    #[arg(long = "verify-token", alias = "verify-access")]
    pub verify_token: bool,

    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct AuthDoctorArgs {
    /// Acquire a Google access token and call networks.list. The token is never printed.
    #[arg(long = "verify-token", alias = "verify-access")]
    pub verify_token: bool,

    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub print_tools: bool,
    pub print_tool_schema: bool,
    pub scope: String,
    pub quota_project: Option<String>,
    pub shared_adc: bool,
    pub service_account_json_path: Option<String>,
    pub service_account_json: Option<String>,
    pub http_timeout: Duration,
    pub api_base_url: String,
    pub soap_base_url: String,
    pub write_mode: GuardedActionRuntimeMode,
    pub report_poll_timeout: Duration,
    pub report_poll_initial_interval: Duration,
    pub scratchpad_session_ttl: Duration,
    pub scratchpad_max_sessions: usize,
    pub scratchpad_max_tables_per_session: usize,
    pub scratchpad_max_rows_per_session: usize,
    pub scratchpad_max_memory_mb: usize,
    pub scratchpad_query_timeout: Duration,
    pub scratchpad_max_sql_bytes: usize,
    pub scratchpad_root_dir: PathBuf,
    pub command: Option<CliCommand>,
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
        let soap_base_url = normalize_required("soap_base_url", cli.soap_base_url)?;
        let write_mode = parse_write_mode(&cli.write_mode)?;
        if !api_base_url.starts_with("https://") {
            return Err(AdManagerError::invalid(
                "api_base_url",
                "must start with https://",
            ));
        }
        if !soap_base_url.starts_with("https://") {
            return Err(AdManagerError::invalid(
                "soap_base_url",
                "must start with https://",
            ));
        }
        if cli.scratchpad_session_ttl_secs == 0 {
            return Err(AdManagerError::invalid(
                "scratchpad_session_ttl_secs",
                "must be greater than zero",
            ));
        }
        if cli.scratchpad_max_sessions == 0 {
            return Err(AdManagerError::invalid(
                "scratchpad_max_sessions",
                "must be greater than zero",
            ));
        }
        if cli.scratchpad_max_tables_per_session == 0 {
            return Err(AdManagerError::invalid(
                "scratchpad_max_tables_per_session",
                "must be greater than zero",
            ));
        }
        if cli.scratchpad_max_rows_per_session == 0 {
            return Err(AdManagerError::invalid(
                "scratchpad_max_rows_per_session",
                "must be greater than zero",
            ));
        }
        if cli.scratchpad_max_memory_mb == 0 {
            return Err(AdManagerError::invalid(
                "scratchpad_max_memory_mb",
                "must be greater than zero",
            ));
        }
        if cli.scratchpad_query_timeout_ms == 0 {
            return Err(AdManagerError::invalid(
                "scratchpad_query_timeout_ms",
                "must be greater than zero",
            ));
        }
        if cli.scratchpad_max_sql_bytes == 0 {
            return Err(AdManagerError::invalid(
                "scratchpad_max_sql_bytes",
                "must be greater than zero",
            ));
        }
        Ok(Self {
            print_tools: cli.print_tools,
            print_tool_schema: cli.print_tool_schema,
            scope,
            quota_project: normalize_optional(cli.quota_project),
            shared_adc: cli.shared_adc,
            service_account_json_path: normalize_optional(cli.service_account_json_path),
            service_account_json: normalize_optional(cli.service_account_json),
            http_timeout: Duration::from_millis(cli.http_timeout_ms.max(1)),
            api_base_url,
            soap_base_url,
            write_mode,
            report_poll_timeout: Duration::from_millis(cli.report_poll_timeout_ms.max(1)),
            report_poll_initial_interval: Duration::from_millis(
                cli.report_poll_initial_interval_ms.max(250),
            ),
            scratchpad_session_ttl: Duration::from_secs(cli.scratchpad_session_ttl_secs),
            scratchpad_max_sessions: cli.scratchpad_max_sessions,
            scratchpad_max_tables_per_session: cli.scratchpad_max_tables_per_session,
            scratchpad_max_rows_per_session: cli.scratchpad_max_rows_per_session,
            scratchpad_max_memory_mb: cli.scratchpad_max_memory_mb,
            scratchpad_query_timeout: Duration::from_millis(cli.scratchpad_query_timeout_ms),
            scratchpad_max_sql_bytes: cli.scratchpad_max_sql_bytes,
            scratchpad_root_dir: cli.scratchpad_root_dir.unwrap_or_else(std::env::temp_dir),
            command: cli.command,
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
            shared_adc: false,
            service_account_json_path: None,
            service_account_json: None,
            http_timeout: Duration::from_millis(15_000),
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            soap_base_url: DEFAULT_SOAP_BASE_URL.to_string(),
            write_mode: GuardedActionRuntimeMode::PreviewOnly,
            report_poll_timeout: Duration::from_millis(300_000),
            report_poll_initial_interval: Duration::from_millis(5_000),
            scratchpad_session_ttl: Duration::from_secs(DEFAULT_SCRATCHPAD_SESSION_TTL_SECS),
            scratchpad_max_sessions: DEFAULT_SCRATCHPAD_MAX_SESSIONS,
            scratchpad_max_tables_per_session: DEFAULT_SCRATCHPAD_MAX_TABLES_PER_SESSION,
            scratchpad_max_rows_per_session: DEFAULT_SCRATCHPAD_MAX_ROWS_PER_SESSION,
            scratchpad_max_memory_mb: DEFAULT_SCRATCHPAD_MAX_MEMORY_MB,
            scratchpad_query_timeout: Duration::from_millis(DEFAULT_SCRATCHPAD_QUERY_TIMEOUT_MS),
            scratchpad_max_sql_bytes: DEFAULT_SCRATCHPAD_MAX_SQL_BYTES,
            scratchpad_root_dir: std::env::temp_dir(),
            command: None,
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

fn parse_write_mode(value: &str) -> Result<GuardedActionRuntimeMode, AdManagerError> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "read_only" | "readonly" => Ok(GuardedActionRuntimeMode::ReadOnly),
        "preview_only" | "preview" | "dry_run" | "dryrun" => {
            Ok(GuardedActionRuntimeMode::PreviewOnly)
        }
        "enabled" | "write" | "writes" | "apply" => Ok(GuardedActionRuntimeMode::Enabled),
        _ => Err(AdManagerError::invalid(
            "write_mode",
            "must be one of read_only, preview_only, or enabled",
        )),
    }
}

pub fn server_adc_credentials_path() -> Option<PathBuf> {
    server_cloudsdk_config_dir().map(|path| path.join("application_default_credentials.json"))
}

pub fn server_cloudsdk_config_dir() -> Option<PathBuf> {
    config_root().map(|root| root.join("google-ad-manager-mcp").join("gcloud"))
}

pub fn conventional_adc_credentials_path() -> Option<PathBuf> {
    if let Some(config_dir) = env::var_os("CLOUDSDK_CONFIG").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(config_dir).join("application_default_credentials.json"));
    }
    #[cfg(windows)]
    {
        env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(|appdata| {
                PathBuf::from(appdata)
                    .join("gcloud")
                    .join("application_default_credentials.json")
            })
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(|home| {
                PathBuf::from(home)
                    .join(".config")
                    .join("gcloud")
                    .join("application_default_credentials.json")
            })
    }
}

fn config_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_READONLY_SCOPE, GCLOUD_ADC_REQUIRED_SCOPE, Settings, parse_write_mode};
    use mcp_toolkit_core::guarded_action::GuardedActionRuntimeMode;

    #[test]
    fn defaults_are_read_only_preview_only_and_https() {
        let settings = Settings::default();
        assert_eq!(settings.scope, DEFAULT_READONLY_SCOPE);
        assert_eq!(settings.write_mode, GuardedActionRuntimeMode::PreviewOnly);
        assert!(!settings.shared_adc);
        assert!(settings.api_base_url.starts_with("https://"));
        assert!(settings.soap_base_url.starts_with("https://"));
        assert!(GCLOUD_ADC_REQUIRED_SCOPE.ends_with("/auth/cloud-platform"));
    }

    #[test]
    fn parses_write_mode_aliases() {
        assert_eq!(
            parse_write_mode("preview-only").expect("mode"),
            GuardedActionRuntimeMode::PreviewOnly
        );
        assert_eq!(
            parse_write_mode("enabled").expect("mode"),
            GuardedActionRuntimeMode::Enabled
        );
        assert!(parse_write_mode("surprise").is_err());
    }
}
