//! Thin authenticated adapter for Google Ad Manager REST APIs.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gcp_auth::{CustomServiceAccount, TokenProvider};
use mcp_toolkit_auth::upstream_oauth::{
    RefreshTokenProvider, UpstreamOAuthError, google_authorized_user_adc_from_file,
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, Method, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use tokio::time::{Instant as TokioInstant, sleep_until, timeout_at};

use crate::config::{MAX_REPORT_POLL_TIMEOUT_MS, Settings};
use crate::error::AdManagerError;

pub const DEFAULT_SOAP_API_VERSION: &str = "v202605";
const MAX_SOAP_PAYLOAD_XML_BYTES: usize = 256 * 1024;
const MAX_SOAP_RESPONSE_XML_BYTES: usize = 200 * 1024;
pub(crate) const MAX_REPORT_OPERATION_RESPONSE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_REPORT_RESULT_RESPONSE_BYTES: usize = 512 * 1024;
pub(crate) const MAX_REPORT_RESULT_PAGE_SIZE: u32 = 1_000;
pub(crate) const MAX_REPORT_INITIAL_POLL_INTERVAL_MS: u64 = 30 * 1_000;
const MAX_REPORT_RESOURCE_NAME_BYTES: usize = 256;
const MAX_REPORT_NUMERIC_ID_BYTES: usize = 32;
const MAX_REPORT_OPERATION_ID_BYTES: usize = 128;
const MAX_REPORT_PAGE_TOKEN_BYTES: usize = 4 * 1024;
const SOAP_ENVELOPE_NAMESPACE: &str = concat!("http", "://schemas.xmlsoap.org/soap/envelope/");
const NETWORK_HIERARCHY_FIELDS: &str = "name,networkCode,effectiveRootAdUnit";
const AD_UNIT_HIERARCHY_FIELDS: &str = "adUnits.name,adUnits.parentAdUnit,adUnits.parentPath.parentAdUnit,adUnits.status,adUnits.hasChildren,adUnits.updateTime,nextPageToken";

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
#[non_exhaustive]
pub enum CatalogCollection {
    AdUnits,
    Orders,
    LineItems,
    Placements,
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
            Self::Placements => "placements",
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
            Self::Placements => "placements",
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
                "payload_xml must contain <statement> with a PQL query"
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

#[derive(Debug, Clone, Serialize)]
pub(crate) struct YieldGroupUpdateSoapRequest {
    pub network_code: String,
    pub api_version: String,
    pub service: &'static str,
    pub method: &'static str,
    pub endpoint: String,
    pub namespace: String,
    pub payload_xml: String,
    pub envelope_xml: String,
    pub target: String,
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
    pub report_name: String,
    pub report_result: String,
}

#[derive(Debug)]
pub(crate) struct ReportPollFailure {
    pub error: AdManagerError,
    pub operation: Option<Value>,
    pub expected_report_name: Option<String>,
    pub terminal: bool,
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
    #[cfg(test)]
    test_access_token: Option<Arc<str>>,
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
            #[cfg(test)]
            test_access_token: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_api_base_url(api_base_url: impl Into<Arc<str>>) -> Self {
        let mut client = Self::from_settings(&Settings::default());
        client.api_base_url = api_base_url.into();
        client.test_access_token = Some(Arc::from("test-access-token"));
        client
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

    pub(crate) async fn get_network_with_request_state(
        &self,
        network_code: &str,
    ) -> (Result<Value, AdManagerError>, bool) {
        let network_code = match validate_network_code(network_code) {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let token = match self.access_token().await {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let url = match absolute_api_url(&self.api_base_url, &format!("networks/{network_code}")) {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let mut request = self.http.request(Method::GET, url).bearer_auth(token);
        if let Some(quota_project) = &self.quota_project {
            request = request.header("x-goog-user-project", quota_project.as_ref());
        }
        request = request.query(&[("fields", NETWORK_HIERARCHY_FIELDS)]);
        (self.send_json(request).await, true)
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

    pub(crate) async fn list_ad_units_bounded_with_request_state(
        &self,
        network_code: &str,
        page_size: u32,
        page_token: Option<String>,
        max_response_bytes: usize,
    ) -> (Result<(Value, usize), AdManagerError>, bool) {
        let network_code = match validate_network_code(network_code) {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let token = match self.access_token().await {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let url = match absolute_api_url(
            &self.api_base_url,
            &format!("networks/{network_code}/adUnits"),
        ) {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let query = ad_unit_hierarchy_list_query(page_size, page_token);
        let mut request = self.http.request(Method::GET, url).bearer_auth(token);
        if let Some(quota_project) = &self.quota_project {
            request = request.header("x-goog-user-project", quota_project.as_ref());
        }
        request = request.query(&query);
        (
            self.send_json_bounded(request, max_response_bytes).await,
            true,
        )
    }

    pub async fn get_ad_unit(
        &self,
        network_code: &str,
        resource_name: &str,
    ) -> Result<Value, AdManagerError> {
        self.get_ad_unit_with_request_state(network_code, resource_name)
            .await
            .0
    }

    pub(crate) async fn get_ad_unit_with_request_state(
        &self,
        network_code: &str,
        resource_name: &str,
    ) -> (Result<Value, AdManagerError>, bool) {
        let network_code = match validate_network_code(network_code) {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let resource_name = match validate_resource_name(
            "ad_unit_resource_names",
            Some(resource_name),
            &network_code,
            "adUnits",
        ) {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let token = match self.access_token().await {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let url = match absolute_api_url(&self.api_base_url, &resource_name) {
            Ok(value) => value,
            Err(err) => return (Err(err), false),
        };
        let mut request = self.http.request(Method::GET, url).bearer_auth(token);
        if let Some(quota_project) = &self.quota_project {
            request = request.header("x-goog-user-project", quota_project.as_ref());
        }
        (self.send_json(request).await, true)
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
        let network_code = validate_numeric_identifier("network_code", network_code)?;
        let report_id = validate_numeric_identifier("report_id", report_id)?;
        let token = self.access_token().await?;
        let url = absolute_api_url(
            &self.api_base_url,
            &format!("networks/{network_code}/reports/{report_id}:run"),
        )?;
        let mut request = self
            .http
            .request(Method::POST, url)
            .bearer_auth(token)
            .body(Vec::new());
        if let Some(quota_project) = &self.quota_project {
            request = request.header("x-goog-user-project", quota_project.as_ref());
        }
        self.send_report_run_json_bounded(request, MAX_REPORT_OPERATION_RESPONSE_BYTES)
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
            if page_size == 0 || page_size > MAX_REPORT_RESULT_PAGE_SIZE {
                return Err(AdManagerError::invalid(
                    "page_size",
                    format!("must be between 1 and {MAX_REPORT_RESULT_PAGE_SIZE}"),
                ));
            }
            query.push(("pageSize", page_size.to_string()));
        }
        if let Some(page_token) = non_empty(page_token) {
            if page_token.len() > MAX_REPORT_PAGE_TOKEN_BYTES {
                return Err(AdManagerError::invalid(
                    "page_token",
                    format!("must be at most {MAX_REPORT_PAGE_TOKEN_BYTES} bytes"),
                ));
            }
            query.push(("pageToken", page_token));
        }
        let payload = self
            .get_json_bounded(
                &format!("{result_name}:fetchRows"),
                &query,
                MAX_REPORT_RESULT_RESPONSE_BYTES,
            )
            .await?;
        validate_report_result_rows_payload(&payload)?;
        Ok(payload)
    }

    pub async fn wait_for_report_result(
        &self,
        operation_name: &str,
        timeout: Duration,
        initial_interval: Duration,
    ) -> Result<CompletedReportRun, AdManagerError> {
        self.wait_for_report_result_with_state(
            operation_name,
            None,
            None,
            timeout,
            initial_interval,
        )
        .await
        .map_err(|failure| failure.error)
    }

    pub(crate) async fn wait_for_report_result_with_state(
        &self,
        operation_name: &str,
        expected_report_name: Option<&str>,
        initial_operation: Option<Value>,
        timeout: Duration,
        initial_interval: Duration,
    ) -> Result<CompletedReportRun, ReportPollFailure> {
        if timeout.is_zero() || timeout.as_millis() > u128::from(MAX_REPORT_POLL_TIMEOUT_MS) {
            return Err(ReportPollFailure {
                error: AdManagerError::invalid(
                    "poll_timeout_ms",
                    format!("must be between 1 and {MAX_REPORT_POLL_TIMEOUT_MS}"),
                ),
                operation: None,
                expected_report_name: None,
                terminal: true,
            });
        }
        if initial_interval < Duration::from_secs(5)
            || initial_interval.as_millis() > u128::from(MAX_REPORT_INITIAL_POLL_INTERVAL_MS)
        {
            return Err(ReportPollFailure {
                error: AdManagerError::invalid(
                    "initial_poll_interval_ms",
                    format!("must be between 5000 and {MAX_REPORT_INITIAL_POLL_INTERVAL_MS}"),
                ),
                operation: None,
                expected_report_name: None,
                terminal: true,
            });
        }
        let operation_name =
            validate_operation_name(operation_name).map_err(|error| ReportPollFailure {
                error,
                operation: None,
                expected_report_name: None,
                terminal: true,
            })?;
        let expected_report_name = expected_report_name
            .map(validate_report_name)
            .transpose()
            .map_err(|error| ReportPollFailure {
                error,
                operation: None,
                expected_report_name: None,
                terminal: true,
            })?;
        let deadline = TokioInstant::now() + timeout;
        let mut interval = initial_interval.max(Duration::from_secs(5));
        let mut bound_report_name = expected_report_name;
        let mut last_operation = None;
        let mut initial_operation = initial_operation;

        loop {
            let operation = if let Some(operation) = initial_operation.take() {
                operation
            } else {
                if TokioInstant::now() >= deadline {
                    return Err(ReportPollFailure {
                        error: AdManagerError::ReportRunTimeout {
                            operation_name: operation_name.to_string(),
                            timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                        },
                        operation: last_operation,
                        expected_report_name: bound_report_name,
                        terminal: false,
                    });
                }
                match timeout_at(
                    deadline,
                    self.get_json_bounded(
                        &operation_name,
                        &[],
                        MAX_REPORT_OPERATION_RESPONSE_BYTES,
                    ),
                )
                .await
                {
                    Ok(Ok(operation)) => operation,
                    Ok(Err(error)) => {
                        return Err(ReportPollFailure {
                            error,
                            operation: last_operation,
                            expected_report_name: bound_report_name,
                            terminal: false,
                        });
                    }
                    Err(_) => {
                        return Err(ReportPollFailure {
                            error: AdManagerError::ReportRunTimeout {
                                operation_name: operation_name.to_string(),
                                timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                            },
                            operation: last_operation,
                            expected_report_name: bound_report_name,
                            terminal: false,
                        });
                    }
                }
            };
            let (report_name, projected_operation) = match validate_report_operation_binding(
                &operation,
                Some(&operation_name),
                bound_report_name.as_deref(),
            ) {
                Ok(value) => value,
                Err(error) => {
                    return Err(ReportPollFailure {
                        error,
                        operation: last_operation,
                        expected_report_name: bound_report_name,
                        terminal: false,
                    });
                }
            };
            if bound_report_name.is_none() {
                bound_report_name = Some(report_name.clone());
            }
            last_operation = Some(projected_operation.clone());
            if operation
                .get("done")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                if let Some(error) = operation.get("error") {
                    return Err(ReportPollFailure {
                        error: AdManagerError::ReportRunFailed {
                            operation_name: operation_name.to_string(),
                            message: clip_message(error.to_string()),
                        },
                        operation: Some(projected_operation),
                        expected_report_name: bound_report_name,
                        terminal: true,
                    });
                }
                let report_result = operation
                    .get("response")
                    .and_then(|value| value.get("reportResult"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                if let Some(report_result) = report_result {
                    let report_result = validate_report_result_name(&report_result).map_err(|_| {
                        ReportPollFailure {
                            error: AdManagerError::UpstreamContract {
                                field: "operation.response.reportResult",
                                message: "must match networks/<networkCode>/reports/<reportId>/results/<resultId>"
                                    .to_string(),
                            },
                            operation: Some(projected_operation.clone()),
                            expected_report_name: bound_report_name.clone(),
                            terminal: false,
                        }
                    })?;
                    if !report_result.starts_with(&format!("{report_name}/results/")) {
                        return Err(ReportPollFailure {
                            error: AdManagerError::UpstreamContract {
                                field: "operation.response.reportResult",
                                message:
                                    "must belong to the report named by operation.metadata.report"
                                        .to_string(),
                            },
                            operation: Some(projected_operation),
                            expected_report_name: bound_report_name,
                            terminal: false,
                        });
                    }
                    return Ok(CompletedReportRun {
                        operation: projected_operation,
                        report_name,
                        report_result,
                    });
                }
                return Err(ReportPollFailure {
                    error: AdManagerError::ReportRunMissingResult {
                        operation_name: operation_name.to_string(),
                    },
                    operation: Some(projected_operation),
                    expected_report_name: bound_report_name,
                    terminal: true,
                });
            }

            let now = TokioInstant::now();
            if now >= deadline {
                return Err(ReportPollFailure {
                    error: AdManagerError::ReportRunTimeout {
                        operation_name: operation_name.to_string(),
                        timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                    },
                    operation: Some(projected_operation),
                    expected_report_name: bound_report_name,
                    terminal: false,
                });
            }

            sleep_until(std::cmp::min(deadline, now + interval)).await;
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

    pub(crate) fn build_yield_group_update_request(
        &self,
        network_code: &str,
        api_version: Option<&str>,
        payload_xml: &str,
    ) -> Result<YieldGroupUpdateSoapRequest, AdManagerError> {
        let network_code = validate_network_code(network_code)?;
        let api_version = validate_soap_api_version(api_version)?;
        let payload_xml = validate_soap_payload_xml(payload_xml)?;
        let service = "YieldGroupService";
        let method = "updateYieldGroups";
        let namespace = soap_namespace(&api_version);
        let endpoint = soap_service_url(&self.soap_base_url, &api_version, service)?;
        let envelope_xml = build_soap_envelope(&namespace, &network_code, method, &payload_xml);
        let target = format!("{service}.{method}");

        Ok(YieldGroupUpdateSoapRequest {
            network_code,
            api_version,
            service,
            method,
            endpoint,
            namespace,
            payload_xml,
            envelope_xml,
            target,
        })
    }

    pub(crate) async fn execute_yield_group_update_request(
        &self,
        request: &YieldGroupUpdateSoapRequest,
    ) -> Result<SoapTraffickingApplyResult, AdManagerError> {
        let token = self.access_token().await?;
        let mut upstream_request = self
            .http
            .request(Method::POST, request.endpoint.as_str())
            .bearer_auth(token)
            .header(CONTENT_TYPE, "text/xml; charset=utf-8")
            .header("SOAPAction", "")
            .body(request.envelope_xml.clone());
        if let Some(quota_project) = &self.quota_project {
            upstream_request =
                upstream_request.header("x-goog-user-project", quota_project.as_ref());
        }

        let (upstream_status, response_xml) = self.send_xml(upstream_request).await?;
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

    async fn get_json_bounded(
        &self,
        relative_or_absolute_path: &str,
        query: &[(&str, String)],
        max_response_bytes: usize,
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
        self.send_json_bounded(request, max_response_bytes)
            .await
            .map(|(value, _)| value)
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

    async fn send_json_bounded(
        &self,
        request: RequestBuilder,
        max_response_bytes: usize,
    ) -> Result<(Value, usize), AdManagerError> {
        let mut response = request.send().await?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > max_response_bytes as u64)
        {
            return Err(bounded_response_limit_error(status, max_response_bytes));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > max_response_bytes {
                return Err(bounded_response_limit_error(status, max_response_bytes));
            }
            bytes.extend_from_slice(&chunk);
        }
        let response_bytes = bytes.len();

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
            return Ok((Value::Null, 0));
        }
        Ok((serde_json::from_slice(&bytes)?, response_bytes))
    }

    async fn send_report_run_json_bounded(
        &self,
        request: RequestBuilder,
        max_response_bytes: usize,
    ) -> Result<Value, AdManagerError> {
        let mut response = request.send().await.map_err(report_run_handoff_error)?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > max_response_bytes as u64)
        {
            if status.is_client_error() {
                return Err(AdManagerError::UpstreamApi {
                    status: status.as_u16(),
                    message: format!(
                        "upstream error response exceeded the {max_response_bytes}-byte safety limit"
                    ),
                });
            }
            return Err(report_run_handoff_message(format!(
                "upstream response exceeded the {max_response_bytes}-byte safety limit"
            )));
        }
        let mut bytes = Vec::new();
        loop {
            let chunk = match response.chunk().await {
                Ok(value) => value,
                Err(error) if status.is_client_error() => {
                    return Err(AdManagerError::UpstreamApi {
                        status: status.as_u16(),
                        message: clip_message(format!(
                            "failed to read upstream error response: {error}"
                        )),
                    });
                }
                Err(error) => return Err(report_run_handoff_error(error)),
            };
            let Some(chunk) = chunk else {
                break;
            };
            if bytes.len().saturating_add(chunk.len()) > max_response_bytes {
                if status.is_client_error() {
                    return Err(AdManagerError::UpstreamApi {
                        status: status.as_u16(),
                        message: format!(
                            "upstream error response exceeded the {max_response_bytes}-byte safety limit"
                        ),
                    });
                }
                return Err(report_run_handoff_message(format!(
                    "upstream response exceeded the {max_response_bytes}-byte safety limit"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let message = String::from_utf8_lossy(&bytes).trim().to_string();
            let message = if message.is_empty() {
                "no upstream response body".to_string()
            } else {
                clip_message(message)
            };
            if status.is_client_error() {
                return Err(AdManagerError::UpstreamApi {
                    status: status.as_u16(),
                    message,
                });
            }
            return Err(report_run_handoff_message(format!(
                "upstream returned status {}: {message}",
                status.as_u16(),
            )));
        }
        if bytes.is_empty() {
            return Err(report_run_handoff_message(
                "upstream returned an empty report-run handoff".to_string(),
            ));
        }
        serde_json::from_slice(&bytes).map_err(report_run_handoff_error)
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
        #[cfg(test)]
        if let Some(token) = &self.test_access_token {
            return Ok(token.to_string());
        }

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

fn bounded_response_limit_error(
    status: reqwest::StatusCode,
    max_response_bytes: usize,
) -> AdManagerError {
    let message = format!("upstream response exceeded the {max_response_bytes}-byte safety limit");
    if status.is_success() {
        AdManagerError::UpstreamContract {
            field: "upstream_response",
            message,
        }
    } else {
        AdManagerError::UpstreamApi {
            status: status.as_u16(),
            message,
        }
    }
}

fn ad_unit_hierarchy_list_query(
    page_size: u32,
    page_token: Option<String>,
) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("pageSize", page_size.to_string()),
        ("orderBy", "name".to_string()),
        ("fields", AD_UNIT_HIERARCHY_FIELDS.to_string()),
    ];
    if let Some(page_token) = non_empty(page_token) {
        query.push(("pageToken", page_token));
    }
    query
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

pub(crate) fn validate_soap_api_version(value: Option<&str>) -> Result<String, AdManagerError> {
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
    soap_error_message_with_truncation(result).0
}

pub(crate) fn soap_error_message_with_truncation(
    result: &SoapTraffickingApplyResult,
) -> (String, bool) {
    if let Some(fault) = result
        .soap_fault
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return clip_message_with_truncation(fault.to_string());
    }
    let trimmed = result.upstream_response_xml.trim();
    if trimmed.is_empty() {
        ("no upstream SOAP response body".to_string(), false)
    } else {
        clip_message_with_truncation(trimmed.to_string())
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
    if trimmed.is_empty()
        || trimmed.len() > MAX_REPORT_NUMERIC_ID_BYTES
        || !trimmed.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(AdManagerError::invalid(
            field,
            format!(
                "must be a numeric identifier no longer than {MAX_REPORT_NUMERIC_ID_BYTES} bytes"
            ),
        ));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn validate_operation_name(value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    let segments = trimmed.split('/').collect::<Vec<_>>();
    let valid = matches!(
        segments.as_slice(),
        ["networks", network_code, "operations", "reports", "runs", operation_id]
            if trimmed.len() <= MAX_REPORT_RESOURCE_NAME_BYTES
                && !network_code.is_empty()
                && network_code.len() <= MAX_REPORT_NUMERIC_ID_BYTES
                && network_code.chars().all(|ch| ch.is_ascii_digit())
                && is_resource_id_segment(operation_id)
    );
    if !valid {
        return Err(AdManagerError::invalid(
            "operation_name",
            "must look like networks/<networkCode>/operations/reports/runs/<operationId>",
        ));
    }
    Ok(trimmed.to_string())
}

fn is_resource_id_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REPORT_OPERATION_ID_BYTES
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

pub(crate) fn validate_report_name(value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    let segments = trimmed.split('/').collect::<Vec<_>>();
    let valid = matches!(
        segments.as_slice(),
        ["networks", network_code, "reports", report_id]
            if trimmed.len() <= MAX_REPORT_RESOURCE_NAME_BYTES
                && !network_code.is_empty()
                && network_code.len() <= MAX_REPORT_NUMERIC_ID_BYTES
                && network_code.chars().all(|ch| ch.is_ascii_digit())
                && !report_id.is_empty()
                && report_id.len() <= MAX_REPORT_NUMERIC_ID_BYTES
                && report_id.chars().all(|ch| ch.is_ascii_digit())
    );
    if !valid {
        return Err(AdManagerError::invalid(
            "report_name",
            "must look like networks/<networkCode>/reports/<reportId>",
        ));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn validate_report_result_name(value: &str) -> Result<String, AdManagerError> {
    let trimmed = value.trim();
    let segments = trimmed.split('/').collect::<Vec<_>>();
    let valid = matches!(
        segments.as_slice(),
        ["networks", network_code, "reports", report_id, "results", result_id]
            if trimmed.len() <= MAX_REPORT_RESOURCE_NAME_BYTES
                && !network_code.is_empty()
                && network_code.len() <= MAX_REPORT_NUMERIC_ID_BYTES
                && network_code.chars().all(|ch| ch.is_ascii_digit())
                && !report_id.is_empty()
                && report_id.len() <= MAX_REPORT_NUMERIC_ID_BYTES
                && report_id.chars().all(|ch| ch.is_ascii_digit())
                && !result_id.is_empty()
                && result_id.len() <= MAX_REPORT_NUMERIC_ID_BYTES
                && result_id.chars().all(|ch| ch.is_ascii_digit())
    );
    if !valid {
        return Err(AdManagerError::invalid(
            "result_name",
            "must look like networks/<networkCode>/reports/<reportId>/results/<resultId>",
        ));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn canonical_report_name(
    network_code: &str,
    report_id: &str,
) -> Result<String, AdManagerError> {
    let network_code = validate_numeric_identifier("network_code", network_code)?;
    let report_id = validate_numeric_identifier("report_id", report_id)?;
    Ok(format!("networks/{network_code}/reports/{report_id}"))
}

fn report_run_handoff_error(error: impl std::fmt::Display) -> AdManagerError {
    report_run_handoff_message(error.to_string())
}

fn report_run_handoff_message(message: String) -> AdManagerError {
    AdManagerError::ReportRunHandoffUncertain {
        message: clip_message(message),
    }
}

fn validate_report_result_rows_payload(payload: &Value) -> Result<(), AdManagerError> {
    let Some(object) = payload.as_object() else {
        return Err(AdManagerError::UpstreamContract {
            field: "report_result_rows",
            message: "successful fetchRows response must be a JSON object".to_string(),
        });
    };
    if ![
        "rows",
        "runTime",
        "dateRanges",
        "comparisonDateRanges",
        "totalRowCount",
        "nextPageToken",
    ]
    .iter()
    .any(|field| object.contains_key(*field))
    {
        return Err(AdManagerError::UpstreamContract {
            field: "report_result_rows",
            message:
                "successful fetchRows response must contain documented report result metadata or rows"
                    .to_string(),
        });
    }
    if let Some(rows) = object.get("rows") {
        let Some(rows) = rows.as_array() else {
            return Err(AdManagerError::UpstreamContract {
                field: "report_result_rows.rows",
                message: "must be an array when present".to_string(),
            });
        };
        if rows.iter().any(|row| !row.is_object()) {
            return Err(AdManagerError::UpstreamContract {
                field: "report_result_rows.rows",
                message: "must contain only row objects".to_string(),
            });
        }
    }
    for field in ["dateRanges", "comparisonDateRanges"] {
        if let Some(ranges) = object.get(field)
            && !ranges
                .as_array()
                .is_some_and(|ranges| ranges.iter().all(Value::is_object))
        {
            return Err(AdManagerError::UpstreamContract {
                field: if field == "dateRanges" {
                    "report_result_rows.dateRanges"
                } else {
                    "report_result_rows.comparisonDateRanges"
                },
                message: "must be an array of objects when present".to_string(),
            });
        }
    }
    if object
        .get("runTime")
        .is_some_and(|value| !value.is_string())
    {
        return Err(AdManagerError::UpstreamContract {
            field: "report_result_rows.runTime",
            message: "must be an RFC 3339 timestamp string when present".to_string(),
        });
    }
    if object
        .get("totalRowCount")
        .is_some_and(|value| value.as_u64().is_none())
    {
        return Err(AdManagerError::UpstreamContract {
            field: "report_result_rows.totalRowCount",
            message: "must be a non-negative integer when present".to_string(),
        });
    }
    if let Some(next_page_token) = object.get("nextPageToken") {
        let Some(next_page_token) = next_page_token.as_str() else {
            return Err(AdManagerError::UpstreamContract {
                field: "report_result_rows.nextPageToken",
                message: "must be a string when present".to_string(),
            });
        };
        if next_page_token.len() > MAX_REPORT_PAGE_TOKEN_BYTES {
            return Err(AdManagerError::UpstreamContract {
                field: "report_result_rows.nextPageToken",
                message: format!("must be at most {MAX_REPORT_PAGE_TOKEN_BYTES} bytes"),
            });
        }
    }
    Ok(())
}

pub(crate) fn validate_report_run_handoff(
    operation: &Value,
    expected_report_name: &str,
) -> Result<(String, Value), AdManagerError> {
    validate_report_operation_binding(operation, None, Some(expected_report_name))
        .map(|(_, projected)| {
            let operation_name = projected
                .get("name")
                .and_then(Value::as_str)
                .expect("validated report operation projection has a name")
                .to_string();
            (operation_name, projected)
        })
        .map_err(|error| AdManagerError::ReportRunHandoffUncertain {
            message: error.to_string(),
        })
}

fn validate_report_operation_binding(
    operation: &Value,
    expected_operation_name: Option<&str>,
    expected_report_name: Option<&str>,
) -> Result<(String, Value), AdManagerError> {
    if !operation.is_object() {
        return Err(AdManagerError::UpstreamContract {
            field: "operation",
            message: "report operation response must be a JSON object".to_string(),
        });
    }
    if operation.get("done").is_some_and(|done| !done.is_boolean()) {
        return Err(AdManagerError::UpstreamContract {
            field: "operation.done",
            message: "must be a boolean when present".to_string(),
        });
    }
    if operation
        .get("metadata")
        .is_some_and(|metadata| !metadata.is_object())
    {
        return Err(AdManagerError::UpstreamContract {
            field: "operation.metadata",
            message: "must be an object when present".to_string(),
        });
    }
    if operation
        .get("response")
        .is_some_and(|response| !response.is_object())
    {
        return Err(AdManagerError::UpstreamContract {
            field: "operation.response",
            message: "must be an object when present".to_string(),
        });
    }
    if operation
        .get("error")
        .is_some_and(|error| !error.is_object())
    {
        return Err(AdManagerError::UpstreamContract {
            field: "operation.error",
            message: "must be an object when present".to_string(),
        });
    }
    let done = operation.get("done").and_then(Value::as_bool);
    let has_error = operation.get("error").is_some();
    let has_response = operation.get("response").is_some();
    if done != Some(true) && (has_error || has_response) {
        return Err(AdManagerError::UpstreamContract {
            field: "operation",
            message: "error or response is valid only when done=true".to_string(),
        });
    }
    if done == Some(true) && has_error && has_response {
        return Err(AdManagerError::UpstreamContract {
            field: "operation",
            message: "a completed operation must not contain both error and response".to_string(),
        });
    }
    if operation
        .get("response")
        .and_then(|response| response.get("reportResult"))
        .is_some_and(|report_result| !report_result.is_string())
    {
        return Err(AdManagerError::UpstreamContract {
            field: "operation.response.reportResult",
            message: "must be a report result resource-name string when present".to_string(),
        });
    }
    let operation_name = operation
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AdManagerError::UpstreamContract {
            field: "operation.name",
            message: "report operation response omitted its name".to_string(),
        })?;
    let operation_name =
        validate_operation_name(operation_name).map_err(|_| AdManagerError::UpstreamContract {
            field: "operation.name",
            message: "must match networks/<networkCode>/operations/reports/runs/<operationId>"
                .to_string(),
        })?;
    if expected_operation_name.is_some_and(|expected| expected != operation_name) {
        return Err(AdManagerError::UpstreamContract {
            field: "operation.name",
            message: "poll response name did not match the requested operation".to_string(),
        });
    }

    let report_binding = operation
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("report"));
    let report_name = match report_binding {
        Some(report_name) => {
            let report_name = report_name.as_str().ok_or_else(|| {
                AdManagerError::UpstreamContract {
                    field: "operation.metadata.report",
                    message: "must be a report resource-name string when present".to_string(),
                }
            })?;
            validate_report_name(report_name).map_err(|_| AdManagerError::UpstreamContract {
                field: "operation.metadata.report",
                message: "must match networks/<networkCode>/reports/<reportId>".to_string(),
            })?
        }
        None => expected_report_name
            .map(str::to_string)
            .ok_or_else(|| AdManagerError::UpstreamContract {
                field: "operation.metadata.report",
                message: "report operation response omitted its report binding and no expected report was supplied"
                    .to_string(),
            })?,
    };

    let operation_network = operation_name
        .split('/')
        .nth(1)
        .expect("validated report operation has a network segment");
    let report_network = report_name
        .split('/')
        .nth(1)
        .expect("validated report name has a network segment");
    if operation_network != report_network {
        return Err(AdManagerError::UpstreamContract {
            field: "operation.metadata.report",
            message: "report binding must use the same network as the operation".to_string(),
        });
    }
    if expected_report_name.is_some_and(|expected| expected != report_name) {
        return Err(AdManagerError::UpstreamContract {
            field: "operation.metadata.report",
            message: "report binding did not match the report requested by this run".to_string(),
        });
    }
    if let Some(report_result) = operation
        .get("response")
        .and_then(|response| response.get("reportResult"))
        .and_then(Value::as_str)
    {
        let report_result = validate_report_result_name(report_result).map_err(|_| {
            AdManagerError::UpstreamContract {
                field: "operation.response.reportResult",
                message: "must match networks/<networkCode>/reports/<reportId>/results/<resultId>"
                    .to_string(),
            }
        })?;
        if !report_result.starts_with(&format!("{report_name}/results/")) {
            return Err(AdManagerError::UpstreamContract {
                field: "operation.response.reportResult",
                message: "must belong to the report named by operation.metadata.report".to_string(),
            });
        }
    }

    let mut projected = project_report_operation(operation);
    projected["metadata"]["report"] = Value::String(report_name.clone());
    Ok((report_name, projected))
}

pub(crate) fn project_report_operation(operation: &Value) -> Value {
    let mut projected = json!({
        "name": projected_report_string(operation.get("name")),
        "metadata": {
            "@type": projected_report_string(operation
                .get("metadata")
                .and_then(|metadata| metadata.get("@type"))),
            "percentComplete": operation
                .get("metadata")
                .and_then(|metadata| metadata.get("percentComplete"))
                .and_then(Value::as_f64),
            "report": projected_report_string(operation
                .get("metadata")
                .and_then(|metadata| metadata.get("report"))),
        },
        "done": operation.get("done").and_then(Value::as_bool),
    });
    if let Some(error) = operation.get("error") {
        projected["error"] = json!({
            "code": error.get("code").and_then(Value::as_i64),
            "message": error
                .get("message")
                .and_then(Value::as_str)
                .map(|message| clip_message(message.to_string())),
        });
    }
    if let Some(response) = operation.get("response") {
        projected["response"] = json!({
            "@type": projected_report_string(operation
                .get("response")
                .and_then(|response| response.get("@type"))),
            "reportResult": projected_report_string(response.get("reportResult")),
        });
    }
    projected
}

fn projected_report_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|value| clip_message(value.to_string()))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clip_message(message: String) -> String {
    clip_message_with_truncation(message).0
}

fn clip_message_with_truncation(message: String) -> (String, bool) {
    let trimmed = message.trim();
    if trimmed.len() <= 800 {
        (trimmed.to_string(), false)
    } else {
        let mut end = 800;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        (format!("{}...", &trimmed[..end]), true)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::{
        AD_UNIT_HIERARCHY_FIELDS, AdManagerClient, CatalogCollection, MAX_SOAP_RESPONSE_XML_BYTES,
        NETWORK_HIERARCHY_FIELDS, RestWriteOperation, RestWriteResource, SOAP_ENVELOPE_NAMESPACE,
        SoapTraffickingOperation, ad_unit_hierarchy_list_query, classify_soap_impact, clip_message,
        clip_message_with_truncation, clip_xml_response, extract_xml_tag, project_report_operation,
        validate_operation_name, validate_report_operation_binding, validate_report_result_name,
        validate_report_result_rows_payload, validate_report_run_handoff, validate_rest_write_body,
        validate_soap_payload_xml,
    };
    use crate::{AdManagerError, Settings};
    use serde_json::{Value, json};

    fn serve_one_http_response(response: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().expect("local test address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).expect("read test request");
            let _ = stream.write_all(&response);
        });
        (format!("http://{address}/bounded"), handle)
    }

    #[test]
    fn collection_names_are_curated() {
        assert_eq!(CatalogCollection::AdUnits.as_str(), "ad_units");
        assert_eq!(CatalogCollection::LineItems.as_str(), "line_items");
        assert_eq!(CatalogCollection::Placements.as_str(), "placements");
        assert_eq!(CatalogCollection::Placements.response_field(), "placements");
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
    fn hierarchy_catalog_query_is_fixed_minimal_and_token_preserving() {
        let query = ad_unit_hierarchy_list_query(1_000, Some(" next-page ".to_string()));
        assert_eq!(
            query,
            vec![
                ("pageSize", "1000".to_string()),
                ("orderBy", "name".to_string()),
                ("fields", AD_UNIT_HIERARCHY_FIELDS.to_string()),
                ("pageToken", "next-page".to_string()),
            ]
        );
        assert_eq!(
            AD_UNIT_HIERARCHY_FIELDS,
            "adUnits.name,adUnits.parentAdUnit,adUnits.parentPath.parentAdUnit,adUnits.status,adUnits.hasChildren,adUnits.updateTime,nextPageToken"
        );
        assert_eq!(
            NETWORK_HIERARCHY_FIELDS,
            "name,networkCode,effectiveRootAdUnit"
        );
    }

    #[tokio::test]
    async fn bounded_json_reader_enforces_http_body_limit_before_decode() {
        let client = AdManagerClient::from_settings(&Settings::default());

        let body = br#"{"ok":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes()
        .into_iter()
        .chain(body.iter().copied())
        .collect::<Vec<_>>();
        let (url, server) = serve_one_http_response(response);
        let (value, bytes) = client
            .send_json_bounded(client.http.get(url), body.len())
            .await
            .expect("exact-boundary JSON response");
        server.join().expect("valid response server");
        assert_eq!(value, json!({"ok":true}));
        assert_eq!(bytes, body.len());

        let oversized = b"0123456789";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            oversized.len()
        )
        .into_bytes()
        .into_iter()
        .chain(oversized.iter().copied())
        .collect::<Vec<_>>();
        let (url, server) = serve_one_http_response(response);
        assert!(matches!(
            client.send_json_bounded(client.http.get(url), 5).await,
            Err(AdManagerError::UpstreamContract {
                field: "upstream_response",
                ..
            })
        ));
        server.join().expect("content-length response server");

        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nA\r\n0123456789\r\n0\r\n\r\n".to_vec();
        let (url, server) = serve_one_http_response(chunked);
        assert!(matches!(
            client.send_json_bounded(client.http.get(url), 5).await,
            Err(AdManagerError::UpstreamContract {
                field: "upstream_response",
                ..
            })
        ));
        server.join().expect("chunked response server");

        let invalid = b"not-json";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            invalid.len()
        )
        .into_bytes()
        .into_iter()
        .chain(invalid.iter().copied())
        .collect::<Vec<_>>();
        let (url, server) = serve_one_http_response(response);
        assert!(
            client
                .send_json_bounded(client.http.get(url), invalid.len())
                .await
                .is_err()
        );
        server.join().expect("invalid JSON response server");
    }

    #[tokio::test]
    async fn bounded_json_reader_preserves_non_success_status_when_body_is_oversized() {
        let client = AdManagerClient::from_settings(&Settings::default());
        let body = b"0123456789";
        let content_length_response = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes()
        .into_iter()
        .chain(body.iter().copied())
        .collect::<Vec<_>>();
        let (url, server) = serve_one_http_response(content_length_response);
        let error = client
            .send_json_bounded(client.http.get(url), 5)
            .await
            .expect_err("oversized 4xx Content-Length must fail");
        server.join().expect("oversized 4xx Content-Length server");
        assert!(matches!(
            error,
            AdManagerError::UpstreamApi { status: 400, ref message }
                if message == "upstream response exceeded the 5-byte safety limit"
        ));

        let chunked_response = b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nA\r\n0123456789\r\n0\r\n\r\n".to_vec();
        let (url, server) = serve_one_http_response(chunked_response);
        let error = client
            .send_json_bounded(client.http.get(url), 5)
            .await
            .expect_err("oversized chunked 4xx must fail");
        server.join().expect("oversized chunked 4xx server");
        assert!(matches!(
            error,
            AdManagerError::UpstreamApi { status: 404, ref message }
                if message == "upstream response exceeded the 5-byte safety limit"
        ));
    }

    #[tokio::test]
    async fn report_run_maps_post_dispatch_decode_and_body_failures_to_uncertain_handoffs() {
        for response in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 65537\r\nConnection: close\r\n\r\n".to_vec(),
        ] {
            let (base_url, server) = serve_one_http_response(response);
            let client = AdManagerClient::for_test_api_base_url(base_url);
            let error = client
                .run_report("123", "456")
                .await
                .expect_err("post-dispatch report failure must be uncertain");
            assert!(matches!(
                error,
                AdManagerError::ReportRunHandoffUncertain { .. }
            ));
            server.join().expect("report-run failure server");
        }
    }

    #[tokio::test]
    async fn report_run_distinguishes_definitive_4xx_from_uncertain_5xx() {
        for (status, expected_definitive) in [(400, true), (503, false)] {
            let body = br#"{"error":"run rejected"}"#;
            let response = format!(
                "HTTP/1.1 {status} Upstream Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes()
            .into_iter()
            .chain(body.iter().copied())
            .collect::<Vec<_>>();
            let (base_url, server) = serve_one_http_response(response);
            let client = AdManagerClient::for_test_api_base_url(base_url);
            let error = client
                .run_report("123", "456")
                .await
                .expect_err("report run status must fail");
            server.join().expect("report status server");
            if expected_definitive {
                assert!(matches!(
                    error,
                    AdManagerError::UpstreamApi { status: 400, .. }
                ));
            } else {
                assert!(matches!(
                    error,
                    AdManagerError::ReportRunHandoffUncertain { .. }
                ));
            }
        }

        let response =
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 65537\r\nConnection: close\r\n\r\n"
                .to_vec();
        let (base_url, server) = serve_one_http_response(response);
        let client = AdManagerClient::for_test_api_base_url(base_url);
        let error = client
            .run_report("123", "456")
            .await
            .expect_err("oversized 4xx body remains definitive");
        server.join().expect("oversized 4xx server");
        assert!(matches!(
            error,
            AdManagerError::UpstreamApi { status: 400, .. }
        ));
    }

    #[test]
    fn validates_operation_name_shape() {
        assert!(validate_operation_name("networks/123/operations/reports/runs/456").is_ok());
        assert!(validate_operation_name("reports/123").is_err());
        assert!(
            validate_operation_name("networks/123/reports/456?x=/operations/reports/runs/1")
                .is_err()
        );
        assert!(validate_operation_name("networks/123/operations/reports/runs/456/extra").is_err());
        assert!(validate_operation_name("networks/x/operations/reports/runs/456").is_err());
        assert!(
            validate_operation_name("networks/123/operations/reports/runs/run_456-abc").is_ok()
        );
        assert!(validate_operation_name("networks/123/operations/reports/runs/456?x=1").is_err());
        assert!(
            validate_operation_name(&format!(
                "networks/123/operations/reports/runs/{}",
                "x".repeat(129)
            ))
            .is_err()
        );
    }

    #[test]
    fn validates_report_result_shape() {
        assert!(validate_report_result_name("networks/123/reports/456/results/789").is_ok());
        assert!(validate_report_result_name("networks/123/reports/456").is_err());
        assert!(validate_report_result_name("networks/123/reports/456?x=/results/789").is_err());
        assert!(validate_report_result_name("networks/123/reports/456/results/789/extra").is_err());
        assert!(
            validate_report_result_name(&format!(
                "networks/123/reports/456/results/{}",
                "9".repeat(33)
            ))
            .is_err()
        );
    }

    #[test]
    fn report_result_rows_payload_requires_documented_success_shapes() {
        for invalid in [
            Value::Null,
            json!({}),
            json!({"unknown": true}),
            json!({"rows": {}}),
            json!({"rows": ["not-a-row"]}),
            json!({"nextPageToken": 42}),
            json!({"totalRowCount": "1"}),
            json!({"dateRanges": ["not-a-range"]}),
        ] {
            assert!(validate_report_result_rows_payload(&invalid).is_err());
        }
        for valid in [
            json!({"rows": []}),
            json!({"rows": [{"dimensionValues": [], "metricValueGroups": []}]}),
            json!({"totalRowCount": 0, "nextPageToken": "next"}),
        ] {
            validate_report_result_rows_payload(&valid).expect("valid fetchRows payload");
        }
    }

    #[test]
    fn report_run_handoff_binds_operation_and_report_identity() {
        let operation = json!({
            "name": "networks/123/operations/reports/runs/789",
            "metadata": {"report": "networks/123/reports/456"},
            "done": false,
            "ignored": "not projected",
        });
        let (operation_name, projected) =
            validate_report_run_handoff(&operation, "networks/123/reports/456")
                .expect("valid report handoff");
        assert_eq!(operation_name, "networks/123/operations/reports/runs/789");
        assert_eq!(projected["metadata"]["report"], "networks/123/reports/456");
        assert!(projected.get("ignored").is_none());

        let missing_metadata = json!({
            "name": "networks/123/operations/reports/runs/790",
            "done": false,
        });
        let (_, projected) =
            validate_report_run_handoff(&missing_metadata, "networks/123/reports/456")
                .expect("known report binding may fill omitted metadata");
        assert_eq!(projected["metadata"]["report"], "networks/123/reports/456");

        for invalid in [
            json!({
                "name": "networks/999/operations/reports/runs/789",
                "metadata": {"report": "networks/123/reports/456"},
            }),
            json!({
                "name": "networks/123/operations/reports/runs/789",
                "metadata": {"report": "networks/123/reports/999"},
            }),
            json!({"metadata": {"report": "networks/123/reports/456"}}),
        ] {
            assert!(matches!(
                validate_report_run_handoff(&invalid, "networks/123/reports/456"),
                Err(AdManagerError::ReportRunHandoffUncertain { .. })
            ));
        }
    }

    #[test]
    fn report_operation_rejects_invalid_long_running_operation_unions() {
        for invalid in [
            json!({
                "name": "networks/123/operations/reports/runs/789",
                "metadata": {"report": "networks/123/reports/456"},
                "done": false,
                "error": {"code": 13, "message": "failed"}
            }),
            json!({
                "name": "networks/123/operations/reports/runs/789",
                "metadata": {"report": "networks/123/reports/456"},
                "done": false,
                "response": {"reportResult": "networks/123/reports/456/results/987"}
            }),
            json!({
                "name": "networks/123/operations/reports/runs/789",
                "metadata": {"report": "networks/123/reports/456"},
                "done": true,
                "error": {"code": 13, "message": "failed"},
                "response": {"reportResult": "networks/123/reports/456/results/987"}
            }),
        ] {
            assert!(matches!(
                validate_report_operation_binding(
                    &invalid,
                    Some("networks/123/operations/reports/runs/789"),
                    Some("networks/123/reports/456"),
                ),
                Err(AdManagerError::UpstreamContract {
                    field: "operation",
                    ..
                })
            ));
        }
    }

    #[test]
    fn report_operation_projection_preserves_long_running_operation_union_presence() {
        let pending = project_report_operation(&json!({
            "name": "networks/123/operations/reports/runs/789",
            "metadata": {"report": "networks/123/reports/456"},
            "done": false,
        }));
        assert!(pending.get("error").is_none());
        assert!(pending.get("response").is_none());

        let succeeded = project_report_operation(&json!({
            "name": "networks/123/operations/reports/runs/789",
            "metadata": {"report": "networks/123/reports/456"},
            "done": true,
            "response": {"reportResult": "networks/123/reports/456/results/987"},
        }));
        assert!(succeeded.get("error").is_none());
        assert_eq!(
            succeeded["response"]["reportResult"],
            "networks/123/reports/456/results/987"
        );

        let failed = project_report_operation(&json!({
            "name": "networks/123/operations/reports/runs/789",
            "metadata": {"report": "networks/123/reports/456"},
            "done": true,
            "error": {"code": 13, "message": "report failed"},
        }));
        assert!(failed.get("response").is_none());
        assert_eq!(failed["error"]["code"], 13);
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
                r#"<statement><query>LIMIT 10</query></statement>"#,
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
    fn builds_internal_yield_group_update_request_shape() {
        let client = AdManagerClient::from_settings(&Settings::default());
        let request = client
            .build_yield_group_update_request(
                "1234567",
                None,
                r#"<yieldGroups><yieldGroupId>10</yieldGroupId></yieldGroups>"#,
            )
            .expect("yield group update request");

        assert_eq!(request.api_version, "v202605");
        assert_eq!(request.service, "YieldGroupService");
        assert_eq!(request.method, "updateYieldGroups");
        assert_eq!(
            request.endpoint,
            "https://ads.google.com/apis/ads/publisher/v202605/YieldGroupService"
        );
        assert!(request.envelope_xml.contains(
            r#"<updateYieldGroups xmlns="https://www.google.com/apis/ads/publisher/v202605">"#
        ));
        assert!(
            request
                .envelope_xml
                .contains("<yieldGroups><yieldGroupId>10</yieldGroupId></yieldGroups>")
        );
        assert!(!request.envelope_xml.contains("getYieldGroupsByStatement"));
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
    fn clip_message_respects_utf8_boundaries() {
        let source = "A".repeat(799) + "€tail";
        let (clipped, truncated) = clip_message_with_truncation(source.clone());
        assert!(truncated);
        assert_eq!(clipped, format!("{}...", "A".repeat(799)));
        assert!(clipped.is_char_boundary(clipped.len()));
        assert_eq!(clip_message(source), clipped);
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
