//! Thin authenticated adapter for Google Ad Manager REST APIs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gcp_auth::{CustomServiceAccount, TokenProvider};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, Method, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use tokio::time::sleep;

use crate::config::{Settings, adc_quota_project_id};
use crate::error::AdManagerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    GoogleDefaultProviderChain,
    ServiceAccountJsonPath,
    ServiceAccountJsonEnv,
}

impl AuthSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GoogleDefaultProviderChain => "google_default_provider_chain",
            Self::ServiceAccountJsonPath => "service_account_json_path",
            Self::ServiceAccountJsonEnv => "service_account_json_env",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogCollection {
    AdUnits,
    Orders,
    LineItems,
    Reports,
}

impl CatalogCollection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdUnits => "ad_units",
            Self::Orders => "orders",
            Self::LineItems => "line_items",
            Self::Reports => "reports",
        }
    }

    fn resource_segment(self) -> &'static str {
        match self {
            Self::AdUnits => "adUnits",
            Self::Orders => "orders",
            Self::LineItems => "lineItems",
            Self::Reports => "reports",
        }
    }

    pub fn response_field(self) -> &'static str {
        self.resource_segment()
    }
}

#[derive(Debug, Clone)]
enum UpstreamAuthMode {
    Adc,
    ServiceAccountJsonPath(String),
    ServiceAccountJsonEnv(String),
}

#[derive(Debug, Clone)]
pub struct CompletedReportRun {
    pub operation: Value,
    pub report_result: String,
}

#[derive(Clone)]
pub struct AdManagerClient {
    http: Client,
    auth_mode: UpstreamAuthMode,
    token_provider: Arc<OnceCell<Arc<dyn TokenProvider>>>,
    scope: Arc<str>,
    api_base_url: Arc<str>,
    quota_project: Option<Arc<str>>,
}

impl AdManagerClient {
    pub fn from_settings(settings: &Settings) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("google-ad-manager-mcp/", env!("CARGO_PKG_VERSION"))),
        );

        let http = Client::builder()
            .timeout(settings.http_timeout)
            .default_headers(headers)
            .build()
            .expect("reqwest client should build");

        let quota_project = settings
            .quota_project
            .clone()
            .or_else(adc_quota_project_id)
            .map(Arc::<str>::from);

        Self {
            http,
            auth_mode: select_auth_mode(settings).expect("auth mode should build"),
            token_provider: Arc::new(OnceCell::new()),
            scope: Arc::from(settings.scope.as_str()),
            api_base_url: Arc::from(settings.api_base_url.as_str()),
            quota_project,
        }
    }

    pub fn auth_source(&self) -> AuthSource {
        match &self.auth_mode {
            UpstreamAuthMode::Adc => AuthSource::GoogleDefaultProviderChain,
            UpstreamAuthMode::ServiceAccountJsonPath(_) => AuthSource::ServiceAccountJsonPath,
            UpstreamAuthMode::ServiceAccountJsonEnv(_) => AuthSource::ServiceAccountJsonEnv,
        }
    }

    pub fn scope(&self) -> &str {
        self.scope.as_ref()
    }

    pub fn quota_project_configured(&self) -> bool {
        self.quota_project.is_some()
    }

    pub async fn verify_token(&self) -> Result<(), AdManagerError> {
        self.access_token().await.map(|_| ())
    }

    pub async fn list_networks(
        &self,
        page_size: Option<u32>,
        page_token: Option<String>,
    ) -> Result<Value, AdManagerError> {
        let mut query = Vec::new();
        if let Some(page_size) = page_size {
            query.push(("pageSize", page_size.to_string()));
        }
        if let Some(page_token) = non_empty(page_token) {
            query.push(("pageToken", page_token));
        }
        self.get_json("networks", &query).await
    }

    pub async fn list_network_catalog(
        &self,
        network_code: &str,
        collection: CatalogCollection,
        page_size: Option<u32>,
        page_token: Option<String>,
        filter: Option<String>,
        order_by: Option<String>,
    ) -> Result<Value, AdManagerError> {
        let network_code = validate_network_code(network_code)?;
        let mut query = Vec::new();
        if let Some(page_size) = page_size {
            query.push(("pageSize", page_size.to_string()));
        }
        if let Some(page_token) = non_empty(page_token) {
            query.push(("pageToken", page_token));
        }
        if let Some(filter) = non_empty(filter) {
            query.push(("filter", filter));
        }
        if let Some(order_by) = non_empty(order_by) {
            query.push(("orderBy", order_by));
        }
        self.get_json(
            &format!("networks/{network_code}/{}", collection.resource_segment()),
            &query,
        )
        .await
    }

    pub async fn run_report(
        &self,
        network_code: &str,
        report_id: &str,
    ) -> Result<Value, AdManagerError> {
        let network_code = validate_network_code(network_code)?;
        let report_id = validate_numeric_identifier("report_id", report_id)?;
        self.post_empty_json(&format!("networks/{network_code}/reports/{report_id}:run"))
            .await
    }

    pub async fn get_report_result_rows(
        &self,
        result_name: &str,
        page_size: Option<u32>,
        page_token: Option<String>,
    ) -> Result<Value, AdManagerError> {
        let result_name = validate_report_result_name(result_name)?;
        let mut query = Vec::new();
        if let Some(page_size) = page_size {
            query.push(("pageSize", page_size.to_string()));
        }
        if let Some(page_token) = non_empty(page_token) {
            query.push(("pageToken", page_token));
        }
        self.get_json(&format!("{result_name}:fetchRows"), &query)
            .await
    }

    pub async fn wait_for_report_result(
        &self,
        operation_name: &str,
        timeout: Duration,
        initial_interval: Duration,
    ) -> Result<CompletedReportRun, AdManagerError> {
        let operation_name = validate_operation_name(operation_name)?;
        let started = Instant::now();
        let mut interval = initial_interval.max(Duration::from_millis(250));

        loop {
            let operation = self.get_json(&operation_name, &[]).await?;
            if operation
                .get("done")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                if let Some(error) = operation.get("error") {
                    return Err(AdManagerError::UpstreamApi {
                        status: 500,
                        message: clip_message(error.to_string()),
                    });
                }
                let report_result = operation
                    .get("response")
                    .and_then(|value| value.get("reportResult"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                if let Some(report_result) = report_result {
                    return Ok(CompletedReportRun {
                        operation,
                        report_result,
                    });
                }
                return Err(AdManagerError::ReportRunMissingResult {
                    operation_name: operation_name.to_string(),
                });
            }

            if started.elapsed() >= timeout {
                return Err(AdManagerError::ReportRunTimeout {
                    operation_name: operation_name.to_string(),
                    timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                });
            }

            sleep(interval).await;
            interval = std::cmp::min(interval.mul_f32(1.5), Duration::from_secs(30));
        }
    }

    async fn get_json(
        &self,
        relative_or_absolute_path: &str,
        query: &[(&str, String)],
    ) -> Result<Value, AdManagerError> {
        let token = self.access_token().await?;
        let mut request = self
            .http
            .request(
                Method::GET,
                absolute_api_url(&self.api_base_url, relative_or_absolute_path)?,
            )
            .bearer_auth(token);
        if let Some(quota_project) = &self.quota_project {
            request = request.header("x-goog-user-project", quota_project.as_ref());
        }
        if !query.is_empty() {
            request = request.query(query);
        }
        self.send_json(request).await
    }

    async fn post_empty_json(
        &self,
        relative_or_absolute_path: &str,
    ) -> Result<Value, AdManagerError> {
        let token = self.access_token().await?;
        let mut request = self
            .http
            .request(
                Method::POST,
                absolute_api_url(&self.api_base_url, relative_or_absolute_path)?,
            )
            .bearer_auth(token)
            .json(&json!({}));
        if let Some(quota_project) = &self.quota_project {
            request = request.header("x-goog-user-project", quota_project.as_ref());
        }
        self.send_json(request).await
    }

    async fn send_json(&self, request: RequestBuilder) -> Result<Value, AdManagerError> {
        let response = request.send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;

        if !status.is_success() {
            let message = String::from_utf8_lossy(&bytes).trim().to_string();
            return Err(AdManagerError::UpstreamApi {
                status: status.as_u16(),
                message: if message.is_empty() {
                    "no upstream response body".to_string()
                } else {
                    clip_message(message)
                },
            });
        }

        if bytes.is_empty() {
            return Ok(Value::Null);
        }

        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn token_provider(&self) -> Result<Arc<dyn TokenProvider>, AdManagerError> {
        let provider = self
            .token_provider
            .get_or_try_init(|| async {
                match &self.auth_mode {
                    UpstreamAuthMode::Adc => gcp_auth::provider()
                        .await
                        .map_err(|err| AdManagerError::AuthBootstrap(err.to_string())),
                    UpstreamAuthMode::ServiceAccountJsonPath(path) => {
                        let provider: Arc<dyn TokenProvider> =
                            Arc::new(CustomServiceAccount::from_file(path).map_err(|err| {
                                AdManagerError::AuthBootstrap(format!(
                                    "failed to load service account JSON at '{path}': {err}"
                                ))
                            })?);
                        Ok(provider)
                    }
                    UpstreamAuthMode::ServiceAccountJsonEnv(raw_json) => {
                        let provider: Arc<dyn TokenProvider> =
                            Arc::new(CustomServiceAccount::from_json(raw_json).map_err(
                                |err| {
                                    AdManagerError::AuthBootstrap(format!(
                                        "invalid service account JSON in GOOGLE_AD_MANAGER_MCP_SERVICE_ACCOUNT_JSON: {err}"
                                    ))
                                },
                            )?);
                        Ok(provider)
                    }
                }
            })
            .await?;
        Ok(provider.clone())
    }

    async fn access_token(&self) -> Result<String, AdManagerError> {
        let provider = self.token_provider().await?;
        let token = provider
            .token(&[self.scope.as_ref()])
            .await
            .map_err(|err| AdManagerError::AuthBootstrap(err.to_string()))?;
        Ok(token.as_str().to_string())
    }
}

fn select_auth_mode(settings: &Settings) -> Result<UpstreamAuthMode, AdManagerError> {
    if let Some(raw_json) = settings
        .service_account_json
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(UpstreamAuthMode::ServiceAccountJsonEnv(
            raw_json.to_string(),
        ));
    }

    if let Some(path) = settings
        .service_account_json_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(UpstreamAuthMode::ServiceAccountJsonPath(path.to_string()));
    }

    Ok(UpstreamAuthMode::Adc)
}

fn absolute_api_url(
    base_url: &str,
    relative_or_absolute_path: &str,
) -> Result<String, AdManagerError> {
    let trimmed = relative_or_absolute_path.trim();
    if trimmed.starts_with("https://") {
        return Ok(trimmed.to_string());
    }
    Ok(format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        trimmed.trim_start_matches('/')
    ))
}

fn validate_network_code(value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AdManagerError::invalid("network_code", "must not be empty"));
    }
    if trimmed.contains('/') || trimmed.chars().any(char::is_whitespace) {
        return Err(AdManagerError::invalid(
            "network_code",
            "must be the raw Ad Manager network code, for example 1234567",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_numeric_identifier(field: &'static str, value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(AdManagerError::invalid(
            field,
            "must be a numeric identifier",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_operation_name(value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || !trimmed.starts_with("networks/")
        || !trimmed.contains("/operations/reports/runs/")
    {
        return Err(AdManagerError::invalid(
            "operation_name",
            "must look like networks/<networkCode>/operations/reports/runs/<operationId>",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_report_result_name(value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || !trimmed.starts_with("networks/")
        || !trimmed.contains("/reports/")
        || !trimmed.contains("/results/")
    {
        return Err(AdManagerError::invalid(
            "result_name",
            "must look like networks/<networkCode>/reports/<reportId>/results/<resultId>",
        ));
    }
    Ok(trimmed.to_string())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clip_message(message: String) -> String {
    let trimmed = message.trim();
    if trimmed.len() <= 800 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..800])
    }
}

#[cfg(test)]
mod tests {
    use super::{CatalogCollection, validate_operation_name, validate_report_result_name};

    #[test]
    fn collection_names_are_curated() {
        assert_eq!(CatalogCollection::AdUnits.as_str(), "ad_units");
        assert_eq!(CatalogCollection::LineItems.as_str(), "line_items");
    }

    #[test]
    fn validates_operation_name_shape() {
        assert!(validate_operation_name("networks/123/operations/reports/runs/456").is_ok());
        assert!(validate_operation_name("reports/123").is_err());
    }

    #[test]
    fn validates_report_result_shape() {
        assert!(validate_report_result_name("networks/123/reports/456/results/789").is_ok());
        assert!(validate_report_result_name("networks/123/reports/456").is_err());
    }
}
