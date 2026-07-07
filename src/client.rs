//! Thin authenticated adapter for Google Ad Manager REST APIs.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gcp_auth::{CustomServiceAccount, TokenProvider};
use mcp_toolkit_auth::upstream_oauth::{
    RefreshTokenProvider, UpstreamOAuthError, google_authorized_user_adc_from_file,
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, Method, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use tokio::time::sleep;

use crate::config::Settings;
use crate::error::AdManagerError;

pub const DEFAULT_SOAP_API_VERSION: &str = "v202605";
const MAX_SOAP_PAYLOAD_XML_BYTES: usize = 256 * 1024;
const MAX_SOAP_RESPONSE_XML_BYTES: usize = 200 * 1024;
const SOAP_ENVELOPE_NAMESPACE: &str = concat!("http", "://schemas.xmlsoap.org/soap/envelope/");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    GoogleDefaultProviderChain,
    GoogleAuthorizedUserAdcFileOrDefaultProviderChain,
    ServiceAccountJsonPath,
    ServiceAccountJsonEnv,
}

impl AuthSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GoogleDefaultProviderChain => "google_default_provider_chain",
            Self::GoogleAuthorizedUserAdcFileOrDefaultProviderChain => {
                "google_authorized_user_adc_file_or_default_provider_chain"
            }
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
    PrivateAuctionDeals,
    PrivateAuctions,
    Reports,
}

impl CatalogCollection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdUnits => "ad_units",
            Self::Orders => "orders",
            Self::LineItems => "line_items",
            Self::PrivateAuctionDeals => "private_auction_deals",
            Self::PrivateAuctions => "private_auctions",
            Self::Reports => "reports",
        }
    }

    fn resource_segment(self) -> &'static str {
        match self {
            Self::AdUnits => "adUnits",
            Self::Orders => "orders",
            Self::LineItems => "lineItems",
            Self::PrivateAuctionDeals => "privateAuctionDeals",
            Self::PrivateAuctions => "privateAuctions",
            Self::Reports => "reports",
        }
    }

    pub fn response_field(self) -> &'static str {
        self.resource_segment()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestWriteResource {
    AdSpots,
    AdUnits,
    Applications,
    CmsMetadataKeys,
    CmsMetadataValues,
    Contacts,
    CustomFields,
    CustomTargetingKeys,
    EntitySignalsMappings,
    Labels,
    Placements,
    PrivateAuctionDeals,
    PrivateAuctions,
    Reports,
    Sites,
    SuggestedAdUnit,
    Teams,
}

impl RestWriteResource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdSpots => "ad_spots",
            Self::AdUnits => "ad_units",
            Self::Applications => "applications",
            Self::CmsMetadataKeys => "cms_metadata_keys",
            Self::CmsMetadataValues => "cms_metadata_values",
            Self::Contacts => "contacts",
            Self::CustomFields => "custom_fields",
            Self::CustomTargetingKeys => "custom_targeting_keys",
            Self::EntitySignalsMappings => "entity_signals_mappings",
            Self::Labels => "labels",
            Self::Placements => "placements",
            Self::PrivateAuctionDeals => "private_auction_deals",
            Self::PrivateAuctions => "private_auctions",
            Self::Reports => "reports",
            Self::Sites => "sites",
            Self::SuggestedAdUnit => "suggested_ad_unit",
            Self::Teams => "teams",
        }
    }

    fn resource_segment(self) -> &'static str {
        match self {
            Self::AdSpots => "adSpots",
            Self::AdUnits => "adUnits",
            Self::Applications => "applications",
            Self::CmsMetadataKeys => "cmsMetadataKeys",
            Self::CmsMetadataValues => "cmsMetadataValues",
            Self::Contacts => "contacts",
            Self::CustomFields => "customFields",
            Self::CustomTargetingKeys => "customTargetingKeys",
            Self::EntitySignalsMappings => "entitySignalsMappings",
            Self::Labels => "labels",
            Self::Placements => "placements",
            Self::PrivateAuctionDeals => "privateAuctionDeals",
            Self::PrivateAuctions => "privateAuctions",
            Self::Reports => "reports",
            Self::Sites => "sites",
            Self::SuggestedAdUnit => "suggestedAdUnit",
            Self::Teams => "teams",
        }
    }

    pub fn supports(self, operation: RestWriteOperation) -> bool {
        use RestWriteOperation as Op;
        match self {
            Self::AdSpots => matches!(
                operation,
                Op::Create | Op::Patch | Op::BatchCreate | Op::BatchUpdate
            ),
            Self::AdUnits => matches!(
                operation,
                Op::Create
                    | Op::Patch
                    | Op::BatchCreate
                    | Op::BatchUpdate
                    | Op::BatchActivate
                    | Op::BatchDeactivate
                    | Op::BatchArchive
            ),
            Self::Applications => matches!(
                operation,
                Op::Create
                    | Op::Patch
                    | Op::BatchCreate
                    | Op::BatchUpdate
                    | Op::BatchArchive
                    | Op::BatchUnarchive
            ),
            Self::CmsMetadataKeys | Self::CmsMetadataValues => {
                matches!(operation, Op::BatchActivate | Op::BatchDeactivate)
            }
            Self::Contacts => {
                matches!(
                    operation,
                    Op::Create | Op::Patch | Op::BatchCreate | Op::BatchUpdate
                )
            }
            Self::CustomFields | Self::CustomTargetingKeys | Self::Labels | Self::Teams => {
                matches!(
                    operation,
                    Op::Create
                        | Op::Patch
                        | Op::BatchCreate
                        | Op::BatchUpdate
                        | Op::BatchActivate
                        | Op::BatchDeactivate
                )
            }
            Self::EntitySignalsMappings => {
                matches!(
                    operation,
                    Op::Create | Op::Patch | Op::BatchCreate | Op::BatchUpdate
                )
            }
            Self::Placements => matches!(
                operation,
                Op::Create
                    | Op::Patch
                    | Op::BatchCreate
                    | Op::BatchUpdate
                    | Op::BatchActivate
                    | Op::BatchDeactivate
                    | Op::BatchArchive
            ),
            Self::PrivateAuctionDeals | Self::PrivateAuctions | Self::Reports => {
                matches!(operation, Op::Create | Op::Patch)
            }
            Self::Sites => matches!(
                operation,
                Op::Create
                    | Op::Patch
                    | Op::BatchCreate
                    | Op::BatchUpdate
                    | Op::BatchDeactivate
                    | Op::BatchSubmitForApproval
            ),
            Self::SuggestedAdUnit => matches!(operation, Op::BatchApprove),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestWriteOperation {
    Create,
    Patch,
    BatchCreate,
    BatchUpdate,
    BatchActivate,
    BatchDeactivate,
    BatchArchive,
    BatchUnarchive,
    BatchSubmitForApproval,
    BatchApprove,
}

impl RestWriteOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Patch => "patch",
            Self::BatchCreate => "batch_create",
            Self::BatchUpdate => "batch_update",
            Self::BatchActivate => "batch_activate",
            Self::BatchDeactivate => "batch_deactivate",
            Self::BatchArchive => "batch_archive",
            Self::BatchUnarchive => "batch_unarchive",
            Self::BatchSubmitForApproval => "batch_submit_for_approval",
            Self::BatchApprove => "batch_approve",
        }
    }

    fn batch_suffix(self) -> Option<&'static str> {
        match self {
            Self::BatchCreate => Some("batchCreate"),
            Self::BatchUpdate => Some("batchUpdate"),
            Self::BatchActivate => Some("batchActivate"),
            Self::BatchDeactivate => Some("batchDeactivate"),
            Self::BatchArchive => Some("batchArchive"),
            Self::BatchUnarchive => Some("batchUnarchive"),
            Self::BatchSubmitForApproval => Some("batchSubmitForApproval"),
            Self::BatchApprove => Some("batchApprove"),
            Self::Create | Self::Patch => None,
        }
    }

    pub fn request_hint(self) -> &'static str {
        match self {
            Self::Create | Self::Patch => "body must be the resource JSON object",
            Self::BatchCreate => "body must contain a requests array with create requests",
            Self::BatchUpdate => "body must contain a requests array with update requests",
            Self::BatchActivate
            | Self::BatchDeactivate
            | Self::BatchArchive
            | Self::BatchUnarchive
            | Self::BatchSubmitForApproval
            | Self::BatchApprove => "body must contain a names array of resource names",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoapTraffickingOperation {
    CreateOrders,
    GetOrdersByStatement,
    PerformOrderAction,
    UpdateOrders,
    CreateLineItems,
    GetLineItemsByStatement,
    PerformLineItemAction,
    UpdateLineItems,
    CreateCreatives,
    GetCreativesByStatement,
    PerformCreativeAction,
    UpdateCreatives,
    CreateLineItemCreativeAssociations,
    GetLineItemCreativeAssociationsByStatement,
    GetLineItemCreativeAssociationPreviewUrl,
    GetLineItemCreativeAssociationNativeStylePreviewUrls,
    PerformLineItemCreativeAssociationAction,
    UpdateLineItemCreativeAssociations,
    GetAvailabilityForecast,
    GetAvailabilityForecastById,
    GetDeliveryForecast,
    GetDeliveryForecastByIds,
    GetTrafficData,
    GetYieldGroupsByStatement,
    GetYieldPartners,
}

impl SoapTraffickingOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CreateOrders => "create_orders",
            Self::GetOrdersByStatement => "get_orders_by_statement",
            Self::PerformOrderAction => "perform_order_action",
            Self::UpdateOrders => "update_orders",
            Self::CreateLineItems => "create_line_items",
            Self::GetLineItemsByStatement => "get_line_items_by_statement",
            Self::PerformLineItemAction => "perform_line_item_action",
            Self::UpdateLineItems => "update_line_items",
            Self::CreateCreatives => "create_creatives",
            Self::GetCreativesByStatement => "get_creatives_by_statement",
            Self::PerformCreativeAction => "perform_creative_action",
            Self::UpdateCreatives => "update_creatives",
            Self::CreateLineItemCreativeAssociations => "create_line_item_creative_associations",
            Self::GetLineItemCreativeAssociationsByStatement => {
                "get_line_item_creative_associations_by_statement"
            }
            Self::GetLineItemCreativeAssociationPreviewUrl => {
                "get_line_item_creative_association_preview_url"
            }
            Self::GetLineItemCreativeAssociationNativeStylePreviewUrls => {
                "get_line_item_creative_association_native_style_preview_urls"
            }
            Self::PerformLineItemCreativeAssociationAction => {
                "perform_line_item_creative_association_action"
            }
            Self::UpdateLineItemCreativeAssociations => "update_line_item_creative_associations",
            Self::GetAvailabilityForecast => "get_availability_forecast",
            Self::GetAvailabilityForecastById => "get_availability_forecast_by_id",
            Self::GetDeliveryForecast => "get_delivery_forecast",
            Self::GetDeliveryForecastByIds => "get_delivery_forecast_by_ids",
            Self::GetTrafficData => "get_traffic_data",
            Self::GetYieldGroupsByStatement => "get_yield_groups_by_statement",
            Self::GetYieldPartners => "get_yield_partners",
        }
    }

    pub fn service_name(self) -> &'static str {
        match self {
            Self::CreateOrders
            | Self::GetOrdersByStatement
            | Self::PerformOrderAction
            | Self::UpdateOrders => "OrderService",
            Self::CreateLineItems
            | Self::GetLineItemsByStatement
            | Self::PerformLineItemAction
            | Self::UpdateLineItems => "LineItemService",
            Self::CreateCreatives
            | Self::GetCreativesByStatement
            | Self::PerformCreativeAction
            | Self::UpdateCreatives => "CreativeService",
            Self::CreateLineItemCreativeAssociations
            | Self::GetLineItemCreativeAssociationsByStatement
            | Self::GetLineItemCreativeAssociationPreviewUrl
            | Self::GetLineItemCreativeAssociationNativeStylePreviewUrls
            | Self::PerformLineItemCreativeAssociationAction
            | Self::UpdateLineItemCreativeAssociations => "LineItemCreativeAssociationService",
            Self::GetAvailabilityForecast
            | Self::GetAvailabilityForecastById
            | Self::GetDeliveryForecast
            | Self::GetDeliveryForecastByIds
            | Self::GetTrafficData => "ForecastService",
            Self::GetYieldGroupsByStatement | Self::GetYieldPartners => "YieldGroupService",
        }
    }

    pub fn soap_method(self) -> &'static str {
        match self {
            Self::CreateOrders => "createOrders",
            Self::GetOrdersByStatement => "getOrdersByStatement",
            Self::PerformOrderAction => "performOrderAction",
            Self::UpdateOrders => "updateOrders",
            Self::CreateLineItems => "createLineItems",
            Self::GetLineItemsByStatement => "getLineItemsByStatement",
            Self::PerformLineItemAction => "performLineItemAction",
            Self::UpdateLineItems => "updateLineItems",
            Self::CreateCreatives => "createCreatives",
            Self::GetCreativesByStatement => "getCreativesByStatement",
            Self::PerformCreativeAction => "performCreativeAction",
            Self::UpdateCreatives => "updateCreatives",
            Self::CreateLineItemCreativeAssociations => "createLineItemCreativeAssociations",
            Self::GetLineItemCreativeAssociationsByStatement => {
                "getLineItemCreativeAssociationsByStatement"
            }
            Self::GetLineItemCreativeAssociationPreviewUrl => "getPreviewUrl",
            Self::GetLineItemCreativeAssociationNativeStylePreviewUrls => {
                "getPreviewUrlsForNativeStyles"
            }
            Self::PerformLineItemCreativeAssociationAction => {
                "performLineItemCreativeAssociationAction"
            }
            Self::UpdateLineItemCreativeAssociations => "updateLineItemCreativeAssociations",
            Self::GetAvailabilityForecast => "getAvailabilityForecast",
            Self::GetAvailabilityForecastById => "getAvailabilityForecastById",
            Self::GetDeliveryForecast => "getDeliveryForecast",
            Self::GetDeliveryForecastByIds => "getDeliveryForecastByIds",
            Self::GetTrafficData => "getTrafficData",
            Self::GetYieldGroupsByStatement => "getYieldGroupsByStatement",
            Self::GetYieldPartners => "getYieldPartners",
        }
    }

    pub fn request_hint(self) -> &'static str {
        match self {
            Self::CreateOrders | Self::UpdateOrders => {
                "payload_xml must contain one or more <orders> elements"
            }
            Self::GetOrdersByStatement => {
                "payload_xml must contain <filterStatement> with a PQL query"
            }
            Self::PerformOrderAction => {
                "payload_xml must contain <orderAction> and <filterStatement>"
            }
            Self::CreateLineItems | Self::UpdateLineItems => {
                "payload_xml must contain one or more <lineItems> elements"
            }
            Self::GetLineItemsByStatement => {
                "payload_xml must contain <filterStatement> with a PQL query"
            }
            Self::PerformLineItemAction => {
                "payload_xml must contain <lineItemAction> and <filterStatement>"
            }
            Self::CreateCreatives | Self::UpdateCreatives => {
                "payload_xml must contain one or more <creatives> elements"
            }
            Self::GetCreativesByStatement => {
                "payload_xml must contain <filterStatement> with a PQL query"
            }
            Self::PerformCreativeAction => {
                "payload_xml must contain <creativeAction> and <filterStatement>"
            }
            Self::CreateLineItemCreativeAssociations | Self::UpdateLineItemCreativeAssociations => {
                "payload_xml must contain one or more <lineItemCreativeAssociations> elements"
            }
            Self::GetLineItemCreativeAssociationsByStatement => {
                "payload_xml must contain <filterStatement> with a PQL query"
            }
            Self::GetLineItemCreativeAssociationPreviewUrl => {
                "payload_xml must contain <lineItemCreativeAssociation> or the documented preview-url request fields"
            }
            Self::GetLineItemCreativeAssociationNativeStylePreviewUrls => {
                "payload_xml must contain the documented native-style preview request fields"
            }
            Self::PerformLineItemCreativeAssociationAction => {
                "payload_xml must contain <lineItemCreativeAssociationAction> and <filterStatement>"
            }
            Self::GetAvailabilityForecast => {
                "payload_xml must contain <lineItem> and optional <forecastOptions>"
            }
            Self::GetAvailabilityForecastById => {
                "payload_xml must contain <lineItemId> and optional <forecastOptions>"
            }
            Self::GetDeliveryForecast => {
                "payload_xml must contain one or more <lineItems> and optional <forecastOptions>"
            }
            Self::GetDeliveryForecastByIds => {
                "payload_xml must contain one or more <lineItemIds> and optional <forecastOptions>"
            }
            Self::GetTrafficData => {
                "payload_xml must contain <lineItem> and optional <forecastOptions>"
            }
            Self::GetYieldGroupsByStatement => {
                "payload_xml must contain <filterStatement> with a PQL query"
            }
            Self::GetYieldPartners => {
                "payload_xml should be empty; the method returns yield partners available to the network"
            }
        }
    }

    fn allows_empty_payload(self) -> bool {
        matches!(self, Self::GetYieldPartners)
    }

    pub fn is_mutating(self) -> bool {
        !matches!(
            self,
            Self::GetOrdersByStatement
                | Self::GetLineItemsByStatement
                | Self::GetCreativesByStatement
                | Self::GetLineItemCreativeAssociationsByStatement
                | Self::GetLineItemCreativeAssociationPreviewUrl
                | Self::GetLineItemCreativeAssociationNativeStylePreviewUrls
                | Self::GetAvailabilityForecast
                | Self::GetAvailabilityForecastById
                | Self::GetDeliveryForecast
                | Self::GetDeliveryForecastByIds
                | Self::GetTrafficData
                | Self::GetYieldGroupsByStatement
                | Self::GetYieldPartners
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RestWritePlan {
    pub resource: RestWriteResource,
    pub operation: RestWriteOperation,
    pub network_code: String,
    pub method: &'static str,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Value,
    pub target: String,
    pub readback_path: Option<String>,
    pub destructive: bool,
    pub send_adjacent: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestWriteApplyResult {
    pub upstream_response: Value,
    pub readback: Option<Value>,
    pub readback_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SoapTraffickingPlan {
    pub operation: SoapTraffickingOperation,
    pub network_code: String,
    pub api_version: String,
    pub service: &'static str,
    pub method: &'static str,
    pub endpoint: String,
    pub namespace: String,
    pub payload_xml: String,
    pub envelope_xml: String,
    pub target: String,
    pub mutating: bool,
    pub destructive: bool,
    pub send_adjacent: bool,
    pub request_hint: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct SoapTraffickingApplyResult {
    pub upstream_status: u16,
    pub upstream_response_xml: String,
    pub response_truncated: bool,
    pub request_id: Option<String>,
    pub response_time: Option<String>,
    pub soap_fault: Option<String>,
}

#[derive(Debug, Clone)]
enum UpstreamAuthMode {
    Adc,
    AuthorizedUserAdcFile(PathBuf),
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
    oauth_token_provider: Arc<OnceCell<Arc<RefreshTokenProvider>>>,
    scope: Arc<str>,
    api_base_url: Arc<str>,
    soap_base_url: Arc<str>,
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

        let quota_project = settings.quota_project.clone().map(Arc::<str>::from);

        Self {
            http,
            auth_mode: select_auth_mode(settings).expect("auth mode should build"),
            token_provider: Arc::new(OnceCell::new()),
            oauth_token_provider: Arc::new(OnceCell::new()),
            scope: Arc::from(settings.scope.as_str()),
            api_base_url: Arc::from(settings.api_base_url.as_str()),
            soap_base_url: Arc::from(settings.soap_base_url.as_str()),
            quota_project,
        }
    }

    pub fn auth_source(&self) -> AuthSource {
        match &self.auth_mode {
            UpstreamAuthMode::Adc => AuthSource::GoogleDefaultProviderChain,
            UpstreamAuthMode::AuthorizedUserAdcFile(_) => {
                AuthSource::GoogleAuthorizedUserAdcFileOrDefaultProviderChain
            }
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

    pub async fn get_rest_discovery_document(&self) -> Result<Value, AdManagerError> {
        let request = self.http.request(
            Method::GET,
            "https://admanager.googleapis.com/$discovery/rest?version=v1",
        );
        self.send_json(request).await
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

    pub fn build_rest_write_plan(
        &self,
        network_code: &str,
        resource: RestWriteResource,
        operation: RestWriteOperation,
        resource_name: Option<&str>,
        update_mask: Option<&str>,
        body: Value,
    ) -> Result<RestWritePlan, AdManagerError> {
        let network_code = validate_network_code(network_code)?;
        if !resource.supports(operation) {
            return Err(AdManagerError::UnsupportedRestWrite {
                resource: resource.as_str(),
                operation: operation.as_str(),
            });
        }
        validate_rest_write_body(operation, &body)?;

        let parent = format!("networks/{network_code}");
        let segment = resource.resource_segment();
        let mut query = Vec::new();
        let (method, path, target, readback_path) = match operation {
            RestWriteOperation::Create => (
                "POST",
                format!("{parent}/{segment}"),
                format!("{parent}/{segment}"),
                None,
            ),
            RestWriteOperation::Patch => {
                let name =
                    validate_resource_name("resource_name", resource_name, &network_code, segment)?;
                if let Some(update_mask) = non_empty(update_mask.map(ToOwned::to_owned)) {
                    query.push(("updateMask".to_string(), update_mask));
                }
                ("PATCH", name.clone(), name.clone(), Some(name))
            }
            RestWriteOperation::BatchCreate
            | RestWriteOperation::BatchUpdate
            | RestWriteOperation::BatchActivate
            | RestWriteOperation::BatchDeactivate
            | RestWriteOperation::BatchArchive
            | RestWriteOperation::BatchUnarchive
            | RestWriteOperation::BatchSubmitForApproval
            | RestWriteOperation::BatchApprove => {
                let suffix = operation
                    .batch_suffix()
                    .expect("batch operations have suffixes");
                (
                    "POST",
                    format!("{parent}/{segment}:{suffix}"),
                    format!("{parent}/{segment}:{suffix}"),
                    None,
                )
            }
        };

        Ok(RestWritePlan {
            resource,
            operation,
            network_code,
            method,
            path,
            query,
            body,
            target,
            readback_path,
            destructive: matches!(
                operation,
                RestWriteOperation::BatchArchive | RestWriteOperation::BatchDeactivate
            ),
            send_adjacent: matches!(operation, RestWriteOperation::BatchSubmitForApproval),
        })
    }

    pub async fn execute_rest_write_plan(
        &self,
        plan: &RestWritePlan,
    ) -> Result<RestWriteApplyResult, AdManagerError> {
        let token = self.access_token().await?;
        let method = match plan.method {
            "POST" => Method::POST,
            "PATCH" => Method::PATCH,
            _ => {
                return Err(AdManagerError::invalid(
                    "method",
                    "unsupported REST write method",
                ));
            }
        };
        let mut request = self
            .http
            .request(method, absolute_api_url(&self.api_base_url, &plan.path)?)
            .bearer_auth(token)
            .json(&plan.body);
        if let Some(quota_project) = &self.quota_project {
            request = request.header("x-goog-user-project", quota_project.as_ref());
        }
        if !plan.query.is_empty() {
            request = request.query(&plan.query);
        }

        let upstream_response = self.send_json(request).await?;
        let readback_path = upstream_response
            .get("name")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| plan.readback_path.clone());

        let mut readback = None;
        let mut readback_error = None;
        if let Some(path) = readback_path {
            match self.get_json(&path, &[]).await {
                Ok(value) => readback = Some(value),
                Err(err) => readback_error = Some(clip_message(err.to_string())),
            }
        }

        Ok(RestWriteApplyResult {
            upstream_response,
            readback,
            readback_error,
        })
    }

    pub fn build_soap_trafficking_plan(
        &self,
        network_code: &str,
        api_version: Option<&str>,
        operation: SoapTraffickingOperation,
        payload_xml: &str,
    ) -> Result<SoapTraffickingPlan, AdManagerError> {
        let network_code = validate_network_code(network_code)?;
        let api_version = validate_soap_api_version(api_version)?;
        let payload_xml = validate_soap_payload_xml_for_operation(operation, payload_xml)?;
        let service = operation.service_name();
        let method = operation.soap_method();
        let namespace = soap_namespace(&api_version);
        let endpoint = soap_service_url(&self.soap_base_url, &api_version, service)?;
        let envelope_xml = build_soap_envelope(&namespace, &network_code, method, &payload_xml);
        let impact = classify_soap_impact(operation, &payload_xml);
        let target = format!("{service}.{method}");

        Ok(SoapTraffickingPlan {
            operation,
            network_code,
            api_version,
            service,
            method,
            endpoint,
            namespace,
            payload_xml,
            envelope_xml,
            target,
            mutating: impact.mutating,
            destructive: impact.destructive,
            send_adjacent: impact.send_adjacent,
            request_hint: operation.request_hint(),
        })
    }

    pub async fn execute_soap_trafficking_plan(
        &self,
        plan: &SoapTraffickingPlan,
    ) -> Result<SoapTraffickingApplyResult, AdManagerError> {
        let token = self.access_token().await?;
        let mut request = self
            .http
            .request(Method::POST, plan.endpoint.as_str())
            .bearer_auth(token)
            .header(CONTENT_TYPE, "text/xml; charset=utf-8")
            .header("SOAPAction", "")
            .body(plan.envelope_xml.clone());
        if let Some(quota_project) = &self.quota_project {
            request = request.header("x-goog-user-project", quota_project.as_ref());
        }

        let (upstream_status, response_xml) = self.send_xml(request).await?;
        let (upstream_response_xml, response_truncated) = clip_xml_response(response_xml);
        let request_id = extract_xml_tag(&upstream_response_xml, "requestId");
        let response_time = extract_xml_tag(&upstream_response_xml, "responseTime");
        let soap_fault = extract_xml_tag(&upstream_response_xml, "faultstring")
            .or_else(|| extract_xml_tag(&upstream_response_xml, "Fault"));

        Ok(SoapTraffickingApplyResult {
            upstream_status,
            upstream_response_xml,
            response_truncated,
            request_id,
            response_time,
            soap_fault,
        })
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

    async fn send_xml(&self, request: RequestBuilder) -> Result<(u16, String), AdManagerError> {
        let response = request.send().await?;
        let status = response.status();
        let text = response.text().await?;
        Ok((status.as_u16(), text))
    }

    async fn token_provider(&self) -> Result<Arc<dyn TokenProvider>, AdManagerError> {
        let provider = self
            .token_provider
            .get_or_try_init(|| async {
                match &self.auth_mode {
                    UpstreamAuthMode::Adc => gcp_auth::provider()
                        .await
                        .map_err(|err| AdManagerError::AuthBootstrap(err.to_string())),
                    UpstreamAuthMode::AuthorizedUserAdcFile(_) => Err(AdManagerError::AuthBootstrap(
                        "authorized-user ADC files are handled by the refresh-token provider"
                            .to_string(),
                    )),
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

    async fn oauth_token_provider(
        &self,
    ) -> Result<Option<Arc<RefreshTokenProvider>>, AdManagerError> {
        if let Some(provider) = self.oauth_token_provider.get() {
            return Ok(Some(provider.clone()));
        }

        let UpstreamAuthMode::AuthorizedUserAdcFile(path) = &self.auth_mode else {
            return Err(AdManagerError::AuthBootstrap(
                "authorized-user ADC provider requested for a non-ADC auth mode".to_string(),
            ));
        };

        let scopes = vec![self.scope.as_ref().to_string()];
        let adc = match google_authorized_user_adc_from_file(path, scopes) {
            Ok(adc) => adc,
            Err(err) if google_adc_file_missing(&err) => return Ok(None),
            Err(err) => {
                return Err(AdManagerError::AuthBootstrap(format!(
                    "failed to load authorized-user ADC at '{}': {err}",
                    path.display()
                )));
            }
        };
        let provider = RefreshTokenProvider::new(adc.into_refresh_config())
            .map(Arc::new)
            .map_err(|err| AdManagerError::AuthBootstrap(err.to_string()))?;
        let _ = self.oauth_token_provider.set(provider.clone());
        Ok(Some(provider))
    }

    async fn access_token(&self) -> Result<String, AdManagerError> {
        let preferred_oauth_provider =
            if matches!(&self.auth_mode, UpstreamAuthMode::AuthorizedUserAdcFile(_)) {
                self.oauth_token_provider().await?
            } else {
                None
            };
        if let Some(provider) = preferred_oauth_provider {
            let token = provider
                .access_token()
                .await
                .map_err(|err| AdManagerError::AuthBootstrap(err.to_string()))?;
            return Ok(token.expose_secret().to_string());
        }

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

    if std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").is_some() {
        return Ok(UpstreamAuthMode::Adc);
    }

    if let Some(path) = crate::config::server_adc_credentials_path() {
        return Ok(UpstreamAuthMode::AuthorizedUserAdcFile(path));
    }

    Ok(UpstreamAuthMode::Adc)
}

fn google_adc_file_missing(err: &UpstreamOAuthError) -> bool {
    matches!(
        err,
        UpstreamOAuthError::Io { source, .. } if source.kind() == ErrorKind::NotFound
    )
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

fn validate_resource_name(
    field: &'static str,
    value: Option<&str>,
    network_code: &str,
    segment: &str,
) -> Result<String, AdManagerError> {
    let Some(value) = value else {
        return Err(AdManagerError::invalid(
            field,
            "is required for patch operations",
        ));
    };
    let trimmed = value.trim();
    let expected_prefix = format!("networks/{network_code}/{segment}/");
    if trimmed.is_empty() || !trimmed.starts_with(&expected_prefix) {
        return Err(AdManagerError::invalid(
            field,
            format!("must start with {expected_prefix}"),
        ));
    }
    let id_segment = &trimmed[expected_prefix.len()..];
    if id_segment.is_empty() || id_segment.contains('/') {
        return Err(AdManagerError::invalid(
            field,
            "must contain exactly one resource ID segment after the prefix",
        ));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(AdManagerError::invalid(
            field,
            "must not contain whitespace",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_rest_write_body(
    operation: RestWriteOperation,
    body: &Value,
) -> Result<(), AdManagerError> {
    if !body.is_object() {
        return Err(AdManagerError::invalid(
            "body",
            "must be a JSON object matching the official Ad Manager REST request shape",
        ));
    }

    match operation {
        RestWriteOperation::BatchCreate | RestWriteOperation::BatchUpdate => {
            if !body.get("requests").is_some_and(Value::is_array) {
                return Err(AdManagerError::invalid(
                    "body.requests",
                    "must be an array for batch create/update operations",
                ));
            }
        }
        RestWriteOperation::BatchActivate
        | RestWriteOperation::BatchDeactivate
        | RestWriteOperation::BatchArchive
        | RestWriteOperation::BatchUnarchive
        | RestWriteOperation::BatchSubmitForApproval
        | RestWriteOperation::BatchApprove => {
            if !body.get("names").is_some_and(Value::is_array) {
                return Err(AdManagerError::invalid(
                    "body.names",
                    "must be an array for batch state/action operations",
                ));
            }
        }
        RestWriteOperation::Create | RestWriteOperation::Patch => {}
    }

    Ok(())
}

fn validate_soap_api_version(value: Option<&str>) -> Result<String, AdManagerError> {
    let version = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_SOAP_API_VERSION);
    if !version.starts_with('v')
        || version.len() != 7
        || !version[1..].chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(AdManagerError::invalid(
            "api_version",
            "must look like v202605",
        ));
    }
    Ok(version.to_string())
}

fn validate_soap_payload_xml(value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AdManagerError::invalid(
            "payload_xml",
            "must contain the inner XML for the selected SOAP operation",
        ));
    }
    if trimmed.len() > MAX_SOAP_PAYLOAD_XML_BYTES {
        return Err(AdManagerError::invalid(
            "payload_xml",
            format!("must be at most {MAX_SOAP_PAYLOAD_XML_BYTES} bytes"),
        ));
    }

    let lower = trimmed.to_ascii_lowercase();
    let compact: String = lower
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-' | ':' | ' ' | '\t' | '\n' | '\r'))
        .collect();
    for disallowed in [
        "<?xml",
        "<!doctype",
        "<!entity",
        "<soap",
        "</soap",
        ":envelope",
        "requestheader",
        "authorization:",
        "bearer ",
        "access_token",
        "refresh_token",
        "client_secret",
        "private_key",
    ] {
        if lower.contains(disallowed) {
            return Err(AdManagerError::invalid(
                "payload_xml",
                format!(
                    "must be an operation payload fragment only and must not contain `{disallowed}`"
                ),
            ));
        }
    }
    for (marker, label) in [
        ("authorization", "authorization"),
        ("bearer", "bearer token"),
        ("accesstoken", "access token"),
        ("refreshtoken", "refresh token"),
        ("clientsecret", "client secret"),
        ("privatekey", "private key"),
    ] {
        if compact.contains(marker) {
            return Err(AdManagerError::invalid(
                "payload_xml",
                format!(
                    "must be an operation payload fragment only and must not contain `{label}`"
                ),
            ));
        }
    }

    Ok(trimmed.to_string())
}

fn validate_soap_payload_xml_for_operation(
    operation: SoapTraffickingOperation,
    value: &str,
) -> Result<String, AdManagerError> {
    if value.trim().is_empty() && operation.allows_empty_payload() {
        return Ok(String::new());
    }
    validate_soap_payload_xml(value)
}

fn soap_namespace(api_version: &str) -> String {
    format!("https://www.google.com/apis/ads/publisher/{api_version}")
}

fn soap_service_url(
    base_url: &str,
    api_version: &str,
    service: &str,
) -> Result<String, AdManagerError> {
    let base = base_url.trim();
    if !base.starts_with("https://") {
        return Err(AdManagerError::invalid(
            "soap_base_url",
            "must start with https://",
        ));
    }
    if service.is_empty()
        || !service.ends_with("Service")
        || !service.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return Err(AdManagerError::invalid(
            "service",
            "must be an allowlisted Ad Manager SOAP service name",
        ));
    }
    Ok(format!(
        "{}/{}/{}",
        base.trim_end_matches('/'),
        api_version,
        service
    ))
}

fn build_soap_envelope(
    namespace: &str,
    network_code: &str,
    method: &str,
    payload_xml: &str,
) -> String {
    let application_name = format!("google-ad-manager-mcp/{}", env!("CARGO_PKG_VERSION"));
    format!(
        r#"<soapenv:Envelope xmlns:soapenv="{soap_envelope_namespace}" xmlns:gam="{namespace}">
  <soapenv:Header>
    <gam:RequestHeader>
      <gam:networkCode>{network_code}</gam:networkCode>
      <gam:applicationName>{application_name}</gam:applicationName>
    </gam:RequestHeader>
  </soapenv:Header>
  <soapenv:Body>
    <{method} xmlns="{namespace}">
{payload_xml}
    </{method}>
  </soapenv:Body>
</soapenv:Envelope>"#,
        soap_envelope_namespace = SOAP_ENVELOPE_NAMESPACE,
        namespace = escape_xml_text(namespace),
        network_code = escape_xml_text(network_code),
        application_name = escape_xml_text(&application_name),
        method = method,
        payload_xml = indent_xml_fragment(payload_xml, 6),
    )
}

#[derive(Debug, Clone, Copy)]
struct SoapImpact {
    mutating: bool,
    destructive: bool,
    send_adjacent: bool,
}

fn classify_soap_impact(operation: SoapTraffickingOperation, payload_xml: &str) -> SoapImpact {
    let mutating = operation.is_mutating();
    if !mutating {
        return SoapImpact {
            mutating: false,
            destructive: false,
            send_adjacent: false,
        };
    }

    let lower = payload_xml.to_ascii_lowercase();
    let destructive = matches!(
        operation,
        SoapTraffickingOperation::PerformOrderAction
            | SoapTraffickingOperation::PerformLineItemAction
            | SoapTraffickingOperation::PerformCreativeAction
            | SoapTraffickingOperation::PerformLineItemCreativeAssociationAction
    ) && [
        "delete",
        "archive",
        "unarchive",
        "pause",
        "disapprove",
        "retract",
        "release",
        "deactivate",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    let send_adjacent = !destructive
        && matches!(
            operation,
            SoapTraffickingOperation::CreateOrders
                | SoapTraffickingOperation::UpdateOrders
                | SoapTraffickingOperation::PerformOrderAction
                | SoapTraffickingOperation::CreateLineItems
                | SoapTraffickingOperation::UpdateLineItems
                | SoapTraffickingOperation::PerformLineItemAction
                | SoapTraffickingOperation::CreateLineItemCreativeAssociations
                | SoapTraffickingOperation::UpdateLineItemCreativeAssociations
                | SoapTraffickingOperation::PerformLineItemCreativeAssociationAction
        );

    SoapImpact {
        mutating,
        destructive,
        send_adjacent,
    }
}

fn indent_xml_fragment(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{prefix}{}", line.trim_end())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_xml_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn clip_xml_response(value: String) -> (String, bool) {
    if value.len() <= MAX_SOAP_RESPONSE_XML_BYTES {
        return (value, false);
    }
    let mut limit = MAX_SOAP_RESPONSE_XML_BYTES;
    while limit > 0 && !value.is_char_boundary(limit) {
        limit -= 1;
    }
    let mut clipped = value;
    clipped.truncate(limit);
    clipped.push_str("...");
    (clipped, true)
}

pub(crate) fn soap_error_message(result: &SoapTraffickingApplyResult) -> String {
    if let Some(fault) = result
        .soap_fault
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return clip_message(fault.to_string());
    }
    let trimmed = result.upstream_response_xml.trim();
    if trimmed.is_empty() {
        "no upstream SOAP response body".to_string()
    } else {
        clip_message(trimmed.to_string())
    }
}

fn extract_xml_tag(value: &str, tag: &str) -> Option<String> {
    for prefix in ["", "gam:", "soapenv:", "soap:"] {
        let full_tag = format!("{prefix}{tag}");
        let open = format!("<{full_tag}");
        let close = format!("</{prefix}{tag}>");
        for (start, _) in value.match_indices(&open) {
            let after_tag = &value[start + open.len()..];
            let starts_with_tag_close = after_tag.starts_with('>');
            let starts_with_space = after_tag.chars().next().is_some_and(char::is_whitespace);
            if !(starts_with_tag_close || starts_with_space) {
                continue;
            }
            if let Some(open_end) = after_tag.find('>') {
                let content_start = start + open.len() + open_end + 1;
                if let Some(end) = value[content_start..].find(&close) {
                    return Some(value[content_start..content_start + end].trim().to_string());
                }
            }
        }
    }
    None
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
    use super::{
        AdManagerClient, CatalogCollection, MAX_SOAP_RESPONSE_XML_BYTES, RestWriteOperation,
        RestWriteResource, SOAP_ENVELOPE_NAMESPACE, SoapTraffickingOperation, classify_soap_impact,
        clip_xml_response, extract_xml_tag, validate_operation_name, validate_report_result_name,
        validate_rest_write_body, validate_soap_payload_xml,
    };
    use crate::Settings;
    use serde_json::json;

    #[test]
    fn collection_names_are_curated() {
        assert_eq!(CatalogCollection::AdUnits.as_str(), "ad_units");
        assert_eq!(CatalogCollection::LineItems.as_str(), "line_items");
        assert_eq!(
            CatalogCollection::PrivateAuctions.as_str(),
            "private_auctions"
        );
        assert_eq!(
            CatalogCollection::PrivateAuctionDeals.response_field(),
            "privateAuctionDeals"
        );
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

    #[test]
    fn rest_write_surface_matches_supported_beta_methods() {
        assert!(RestWriteResource::AdUnits.supports(RestWriteOperation::Create));
        assert!(RestWriteResource::AdUnits.supports(RestWriteOperation::BatchArchive));
        assert!(RestWriteResource::Reports.supports(RestWriteOperation::Patch));
        assert!(!RestWriteResource::Reports.supports(RestWriteOperation::BatchArchive));
        assert!(RestWriteResource::SuggestedAdUnit.supports(RestWriteOperation::BatchApprove));
        assert!(!RestWriteResource::SuggestedAdUnit.supports(RestWriteOperation::Patch));
    }

    #[test]
    fn batch_state_bodies_require_names() {
        assert!(
            validate_rest_write_body(RestWriteOperation::BatchActivate, &json!({"names": []}))
                .is_ok()
        );
        assert!(
            validate_rest_write_body(RestWriteOperation::BatchActivate, &json!({"requests": []}))
                .is_err()
        );
    }

    #[test]
    fn builds_rest_write_paths_and_query_params() {
        let client = AdManagerClient::from_settings(&Settings::default());
        let patch = client
            .build_rest_write_plan(
                "1234567",
                RestWriteResource::Reports,
                RestWriteOperation::Patch,
                Some("networks/1234567/reports/987654"),
                Some("displayName"),
                json!({"name": "networks/1234567/reports/987654", "displayName": "Delivery proof"}),
            )
            .expect("patch plan");
        assert_eq!(patch.method, "PATCH");
        assert_eq!(patch.path, "networks/1234567/reports/987654");
        assert_eq!(
            patch.query,
            vec![("updateMask".to_string(), "displayName".to_string())]
        );
        assert_eq!(
            patch.readback_path.as_deref(),
            Some("networks/1234567/reports/987654")
        );

        let submit = client
            .build_rest_write_plan(
                "1234567",
                RestWriteResource::Sites,
                RestWriteOperation::BatchSubmitForApproval,
                None,
                None,
                json!({"names": ["networks/1234567/sites/111"]}),
            )
            .expect("submit plan");
        assert_eq!(submit.method, "POST");
        assert_eq!(submit.path, "networks/1234567/sites:batchSubmitForApproval");
        assert!(submit.send_adjacent);
    }

    #[test]
    fn patch_resource_names_require_exactly_one_id_segment() {
        let client = AdManagerClient::from_settings(&Settings::default());
        for bad_name in [
            "networks/1234567/reports/",
            "networks/1234567/reports/987654/extra",
        ] {
            assert!(
                client
                    .build_rest_write_plan(
                        "1234567",
                        RestWriteResource::Reports,
                        RestWriteOperation::Patch,
                        Some(bad_name),
                        Some("displayName"),
                        json!({"name": bad_name, "displayName": "Delivery proof"}),
                    )
                    .is_err(),
                "{bad_name} should be rejected"
            );
        }
    }

    #[test]
    fn builds_soap_trafficking_envelope_and_endpoint() {
        let client = AdManagerClient::from_settings(&Settings::default());
        let plan = client
            .build_soap_trafficking_plan(
                "1234567",
                None,
                SoapTraffickingOperation::GetLineItemsByStatement,
                r#"<filterStatement><query>WHERE id = 42</query></filterStatement>"#,
            )
            .expect("soap plan");

        assert_eq!(plan.api_version, "v202605");
        assert_eq!(plan.service, "LineItemService");
        assert_eq!(plan.method, "getLineItemsByStatement");
        assert_eq!(
            plan.endpoint,
            "https://ads.google.com/apis/ads/publisher/v202605/LineItemService"
        );
        assert!(plan.envelope_xml.contains("<gam:RequestHeader>"));
        assert!(
            plan.envelope_xml
                .contains("<gam:networkCode>1234567</gam:networkCode>")
        );
        assert!(plan.envelope_xml.contains(
            r#"<getLineItemsByStatement xmlns="https://www.google.com/apis/ads/publisher/v202605">"#
        ));
        assert!(!plan.mutating);
    }

    #[test]
    fn yield_group_soap_operations_are_read_only() {
        let client = AdManagerClient::from_settings(&Settings::default());
        let plan = client
            .build_soap_trafficking_plan(
                "1234567",
                None,
                SoapTraffickingOperation::GetYieldGroupsByStatement,
                r#"<filterStatement><query>LIMIT 10</query></filterStatement>"#,
            )
            .expect("yield group read plan");

        assert_eq!(plan.service, "YieldGroupService");
        assert_eq!(plan.method, "getYieldGroupsByStatement");
        assert_eq!(
            plan.endpoint,
            "https://ads.google.com/apis/ads/publisher/v202605/YieldGroupService"
        );
        assert!(!plan.mutating);
        assert!(!plan.destructive);
        assert!(!plan.send_adjacent);
    }

    #[test]
    fn yield_partner_soap_read_allows_empty_payload() {
        let client = AdManagerClient::from_settings(&Settings::default());
        let plan = client
            .build_soap_trafficking_plan(
                "1234567",
                None,
                SoapTraffickingOperation::GetYieldPartners,
                "",
            )
            .expect("yield partner read plan");

        assert_eq!(plan.service, "YieldGroupService");
        assert_eq!(plan.method, "getYieldPartners");
        assert_eq!(plan.payload_xml, "");
        assert!(plan.envelope_xml.contains(
            r#"<getYieldPartners xmlns="https://www.google.com/apis/ads/publisher/v202605">"#
        ));
        assert!(!plan.mutating);
        assert!(!plan.destructive);
        assert!(!plan.send_adjacent);
    }

    #[test]
    fn soap_payload_guard_rejects_envelopes_headers_and_credentials() {
        for bad in [
            "<?xml version=\"1.0\"?><x/>",
            "<soapenv:Envelope></soapenv:Envelope>",
            "<RequestHeader><networkCode>123</networkCode></RequestHeader>",
            "<filterStatement>Authorization: Bearer secret</filterStatement>",
            "<authorization>Bearer secret</authorization>",
            "<accessToken>secret</accessToken>",
            "<refreshToken>secret</refreshToken>",
            "<clientSecret>secret</clientSecret>",
            "<privateKey>secret</privateKey>",
            "<!DOCTYPE x [<!ENTITY y SYSTEM \"file:///etc/passwd\">]>",
        ] {
            assert!(
                validate_soap_payload_xml(bad).is_err(),
                "{bad} should be rejected"
            );
        }
        assert!(
            validate_soap_payload_xml(
                r#"<filterStatement><query>WHERE status = 'ACTIVE'</query></filterStatement>"#
            )
            .is_ok()
        );
    }

    #[test]
    fn soap_impact_classifies_read_send_adjacent_and_destructive() {
        let read = classify_soap_impact(
            SoapTraffickingOperation::GetAvailabilityForecast,
            "<lineItemId>1</lineItemId>",
        );
        assert!(!read.mutating);
        assert!(!read.send_adjacent);

        let create_line_item =
            classify_soap_impact(SoapTraffickingOperation::CreateLineItems, "<lineItems/>");
        assert!(create_line_item.mutating);
        assert!(create_line_item.send_adjacent);
        assert!(!create_line_item.destructive);

        let pause = classify_soap_impact(
            SoapTraffickingOperation::PerformLineItemAction,
            r#"<lineItemAction xsi:type="PauseLineItems"/><filterStatement/>"#,
        );
        assert!(pause.destructive);
    }

    #[test]
    fn clip_xml_response_respects_utf8_boundaries() {
        let oversized = "A".repeat(MAX_SOAP_RESPONSE_XML_BYTES - 1) + "€tail";
        let (clipped, truncated) = clip_xml_response(oversized);
        assert!(truncated);
        assert!(clipped.ends_with("..."));
        assert!(clipped.is_char_boundary(clipped.len()));
    }

    #[test]
    fn extract_xml_tag_handles_attributes() {
        let xml = format!(
            r#"<soapenv:Fault xmlns:soapenv="{SOAP_ENVELOPE_NAMESPACE}"><faultstring xml:lang="en">boom</faultstring></soapenv:Fault>"#
        );
        assert_eq!(
            extract_xml_tag(&xml, "faultstring").as_deref(),
            Some("boom")
        );
        assert_eq!(
            extract_xml_tag(&xml, "Fault").as_deref(),
            Some(r#"<faultstring xml:lang="en">boom</faultstring>"#)
        );
    }
}
