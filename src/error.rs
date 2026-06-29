//! Error types for Google Ad Manager tools.

#[derive(Debug, thiserror::Error)]
pub enum AdManagerError {
    #[error("invalid {field}: {message}")]
    InvalidInput {
        field: &'static str,
        message: String,
    },
    #[error("auth bootstrap failed: {0}")]
    AuthBootstrap(String),
    #[error("upstream transport failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("failed to parse upstream JSON: {0}")]
    UpstreamJson(#[from] serde_json::Error),
    #[error("upstream returned status {status}: {message}")]
    UpstreamApi { status: u16, message: String },
    #[error("report run operation `{operation_name}` timed out after {timeout_ms}ms")]
    ReportRunTimeout {
        operation_name: String,
        timeout_ms: u64,
    },
    #[error("report run operation `{operation_name}` completed without a report result name")]
    ReportRunMissingResult { operation_name: String },
}

impl AdManagerError {
    pub fn invalid(field: &'static str, message: impl Into<String>) -> Self {
        Self::InvalidInput {
            field,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "invalid_input",
            Self::AuthBootstrap(_) => "auth_bootstrap",
            Self::Transport(_) => "transport_error",
            Self::UpstreamJson(_) => "upstream_json_error",
            Self::UpstreamApi { .. } => "upstream_api_error",
            Self::ReportRunTimeout { .. } => "report_run_timeout",
            Self::ReportRunMissingResult { .. } => "report_run_missing_result",
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "validation_failed",
            Self::AuthBootstrap(_) => "auth_not_ready",
            Self::Transport(_) => "upstream_transport_failed",
            Self::UpstreamJson(_) => "upstream_json_invalid",
            Self::UpstreamApi { .. } => "upstream_request_failed",
            Self::ReportRunTimeout { .. } => "report_poll_timeout",
            Self::ReportRunMissingResult { .. } => "report_result_missing",
        }
    }

    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "input",
            Self::AuthBootstrap(_) => "auth",
            Self::Transport(_) | Self::UpstreamJson(_) | Self::UpstreamApi { .. } => "upstream",
            Self::ReportRunTimeout { .. } | Self::ReportRunMissingResult { .. } => "reports",
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => {
                "Check the tool arguments and use the documented resource-name format."
            }
            Self::AuthBootstrap(_) => {
                "Run gam_auth_status or gam_auth_login_command to inspect or configure credentials."
            }
            Self::Transport(_) | Self::UpstreamJson(_) | Self::UpstreamApi { .. } => {
                "Retry the request, then confirm the Google principal can access the target network."
            }
            Self::ReportRunTimeout { .. } => {
                "Retry with a longer timeout or poll later with gam_report_result_rows once the run completes."
            }
            Self::ReportRunMissingResult { .. } => {
                "Inspect the completed operation payload and confirm the report was shared with the authenticated principal."
            }
        }
    }
}
