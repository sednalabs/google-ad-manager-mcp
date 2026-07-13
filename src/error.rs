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
    #[error("upstream transport failed")]
    Transport {
        #[source]
        source: reqwest::Error,
    },
    #[error("failed to parse upstream JSON: {0}")]
    UpstreamJson(#[from] serde_json::Error),
    #[error("upstream returned status {status}: {message}")]
    UpstreamApi { status: u16, message: String },
    #[error("upstream response contract failed for {field}: {message}")]
    UpstreamContract {
        field: &'static str,
        message: String,
    },
    #[error("write action disabled: {message}")]
    WriteActionDisabled { message: String },
    #[error("write scope required: current scope is `{scope}`")]
    WriteScopeRequired { scope: String },
    #[error("unsupported Ad Manager REST write operation: {resource}.{operation}")]
    UnsupportedRestWrite {
        resource: &'static str,
        operation: &'static str,
    },
    #[error(
        "confirmation token mismatch: rerun the plan tool and pass the returned confirmation_token unchanged"
    )]
    ConfirmationTokenMismatch,
    #[error("report run operation `{operation_name}` timed out after {timeout_ms}ms")]
    ReportRunTimeout {
        operation_name: String,
        timeout_ms: u64,
    },
    #[error("report run operation `{operation_name}` completed without a report result name")]
    ReportRunMissingResult { operation_name: String },
    #[error("report run operation `{operation_name}` failed: {message}")]
    ReportRunFailed {
        operation_name: String,
        message: String,
    },
    #[error(
        "report run request may have been dispatched but a safe operation handoff was not confirmed: {message}"
    )]
    ReportRunHandoffUncertain { message: String },
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
            Self::Transport { .. } => "transport_error",
            Self::UpstreamJson(_) => "upstream_json_error",
            Self::UpstreamApi { .. } => "upstream_api_error",
            Self::UpstreamContract { .. } => "upstream_contract_error",
            Self::WriteActionDisabled { .. } => "write_action_disabled",
            Self::WriteScopeRequired { .. } => "write_scope_required",
            Self::UnsupportedRestWrite { .. } => "unsupported_rest_write",
            Self::ConfirmationTokenMismatch => "confirmation_token_mismatch",
            Self::ReportRunTimeout { .. } => "report_run_timeout",
            Self::ReportRunMissingResult { .. } => "report_run_missing_result",
            Self::ReportRunFailed { .. } => "report_run_failed",
            Self::ReportRunHandoffUncertain { .. } => "report_run_handoff_uncertain",
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "validation_failed",
            Self::AuthBootstrap(_) => "auth_not_ready",
            Self::Transport { .. } => "upstream_transport_failed",
            Self::UpstreamJson(_) => "upstream_json_invalid",
            Self::UpstreamApi { .. } => "upstream_request_failed",
            Self::UpstreamContract { .. } => "upstream_contract_failed",
            Self::WriteActionDisabled { .. } => "write_runtime_gate_closed",
            Self::WriteScopeRequired { .. } => "google_scope_not_write_capable",
            Self::UnsupportedRestWrite { .. } => "rest_beta_surface_gap",
            Self::ConfirmationTokenMismatch => "confirmation_required",
            Self::ReportRunTimeout { .. } => "report_poll_timeout",
            Self::ReportRunMissingResult { .. } => "report_result_missing",
            Self::ReportRunFailed { .. } => "report_operation_failed",
            Self::ReportRunHandoffUncertain { .. } => "report_run_handoff_uncertain",
        }
    }

    pub fn category(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "input",
            Self::AuthBootstrap(_) => "auth",
            Self::Transport { .. }
            | Self::UpstreamJson(_)
            | Self::UpstreamApi { .. }
            | Self::UpstreamContract { .. } => "upstream",
            Self::WriteActionDisabled { .. }
            | Self::WriteScopeRequired { .. }
            | Self::UnsupportedRestWrite { .. }
            | Self::ConfirmationTokenMismatch => "safety",
            Self::ReportRunTimeout { .. }
            | Self::ReportRunMissingResult { .. }
            | Self::ReportRunFailed { .. }
            | Self::ReportRunHandoffUncertain { .. } => "reports",
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
            Self::Transport { .. } | Self::UpstreamJson(_) => {
                "Retry the request, then confirm the Google principal can access the target network."
            }
            Self::UpstreamApi {
                status: 401 | 403, ..
            } => {
                "Run gam_auth_status and correct the Google principal credentials or target-network access before retrying."
            }
            Self::UpstreamApi { status, .. }
                if (400..500).contains(status) && !matches!(*status, 408 | 429) =>
            {
                "Correct the request arguments or target resource identity before retrying; the provider rejected the current request."
            }
            Self::UpstreamApi { .. } => {
                "Retry the request with bounded backoff, then confirm the Google principal can access the target network if the failure persists."
            }
            Self::UpstreamContract {
                field: "operation.completed_report_identity" | "operation.completed_state",
                ..
            } => {
                "Inspect the completed operation, terminal state, and saved report identity; do not repeat the unchanged poll until the provider contract issue is understood."
            }
            Self::UpstreamContract { field, .. } if field.starts_with("report_result_rows") => {
                "Preserve the exact result and page handles, then inspect the saved report and provider response; do not automatically repeat the unchanged row fetch."
            }
            Self::UpstreamContract { .. } => {
                "Do not reuse the malformed provider value; retry the read and report an adapter or upstream contract defect if it persists."
            }
            Self::WriteActionDisabled { .. } => {
                "Start the server with GOOGLE_AD_MANAGER_MCP_WRITE_MODE=enabled only in an operator-approved environment."
            }
            Self::WriteScopeRequired { .. } => {
                "Reauthenticate with --scope https://www.googleapis.com/auth/admanager before applying write plans."
            }
            Self::UnsupportedRestWrite { .. } => {
                "The current REST beta API does not expose this trafficking mutation; use the tool matrix to see the SOAP follow-up boundary."
            }
            Self::ConfirmationTokenMismatch => {
                "Rerun the matching plan tool and copy the returned confirmation_token into the matching apply tool."
            }
            Self::ReportRunTimeout { .. } => {
                "Retry the existing operation with gam_report_operation_poll and a longer timeout; do not start another report run."
            }
            Self::ReportRunMissingResult { .. } => {
                "Inspect the completed operation payload and confirm the report was shared with the authenticated principal."
            }
            Self::ReportRunFailed { .. } => {
                "Inspect the terminal operation error and the saved report definition; this completed failed operation cannot be resumed by polling."
            }
            Self::ReportRunHandoffUncertain { .. } => {
                "Do not automatically start another report run. Preserve this receipt and inspect Ad Manager report activity or retry only with explicit operator approval."
            }
        }
    }
}

impl From<reqwest::Error> for AdManagerError {
    fn from(source: reqwest::Error) -> Self {
        Self::Transport { source }
    }
}

#[cfg(test)]
mod tests {
    use super::AdManagerError;

    #[test]
    fn upstream_api_hints_distinguish_auth_permanent_and_retryable_statuses() {
        for status in [401, 403] {
            let error = AdManagerError::UpstreamApi {
                status,
                message: "rejected".to_string(),
            };
            assert!(error.hint().contains("gam_auth_status"));
            assert!(!error.hint().starts_with("Retry the request"));
        }
        for status in [400, 404, 422] {
            let error = AdManagerError::UpstreamApi {
                status,
                message: "rejected".to_string(),
            };
            assert!(error.hint().contains("Correct the request"));
            assert!(!error.hint().starts_with("Retry the request"));
        }
        for status in [408, 429, 500, 503] {
            let error = AdManagerError::UpstreamApi {
                status,
                message: "retryable".to_string(),
            };
            assert!(error.hint().starts_with("Retry the request"));
        }
    }

    #[test]
    fn malformed_report_result_contract_hint_requires_inspection() {
        let contract_error = AdManagerError::UpstreamContract {
            field: "report_result_rows.rows",
            message: "must be an array".to_string(),
        };
        assert!(
            contract_error
                .hint()
                .contains("do not automatically repeat")
        );
    }
}
