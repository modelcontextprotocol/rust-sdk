//! Explicit client access to a discovered Events catalog, polls, and subscriptions.

use std::{collections::HashMap, sync::Arc};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::{
    EXTENSION, LIST, POLL,
    wire::{
        DeliveryMode, EventsError, ListEventsResult, PollParams, PollResult, not_found, unsupported,
    },
};
use crate::{
    Peer, RoleClient, ServiceError,
    model::{
        ClientRequest, CustomRequest, DEFAULT_LIST_ALL_MAX_PAGES, JsonObject, ProtocolVersion,
        ServerResult,
    },
    service::PeerRequestOptions,
};

const REQUEST_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);

mod subscription;
#[cfg(feature = "events-webhook-receiver")]
mod webhook;
pub use subscription::{EventSubscription, EventUpdate, SubscriptionOptions};

/// A client for a peer that advertised the Events Beta via modern discovery.
///
/// Start the underlying service in Discover mode (or Auto with successful
/// discovery). Invoking these methods never opts a legacy peer into Events.
pub struct EventsClient {
    peer: Peer<RoleClient>,
    catalog: Arc<tokio::sync::Mutex<HashMap<String, CatalogEntry>>>,
}

#[derive(Clone)]
struct CatalogEntry {
    delivery: Vec<DeliveryMode>,
    input_schema: Arc<JsonObject>,
}

impl std::fmt::Debug for EventsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventsClient").finish_non_exhaustive()
    }
}

/// A validated catalog entry selected for this request; the HTTP worker
/// consumes it without maintaining its own Events catalog.
#[cfg(feature = "transport-streamable-http-client")]
#[derive(Clone)]
pub(crate) struct RoutingSchema(pub(crate) Option<Arc<crate::model::JsonObject>>);

impl EventsClient {
    /// Use a peer whose discovery result advertises Events.
    pub fn new(peer: Peer<RoleClient>) -> Self {
        Self {
            peer,
            catalog: Arc::default(),
        }
    }

    /// List one catalog page. Starting a new list invalidates cached routing schemas.
    pub async fn list_page(&self, cursor: Option<&str>) -> Result<ListEventsResult, EventsError> {
        self.require_advertisement()?;
        if cursor.is_none() {
            self.invalidate_catalog().await;
        }
        self.request_page(cursor).await
    }

    async fn request_page(&self, cursor: Option<&str>) -> Result<ListEventsResult, EventsError> {
        let params = cursor.map(|cursor| json!({ "cursor": cursor }));
        let request = CustomRequest::new(LIST, params);
        #[cfg(feature = "transport-streamable-http-client")]
        let request = {
            let mut request = request;
            request.extensions.insert(RoutingSchema(None));
            request
        };
        decode_result(self.request(request).await?)
    }

    /// Poll once. Persist a non-null cursor only after consuming the batch.
    pub async fn poll(&self, params: PollParams) -> Result<PollResult, EventsError> {
        self.require_advertisement()?;
        let body = encode_params(&params)?;
        self.with_route(&params.name, |entry| {
            let request = route(POLL, body.clone(), entry.input_schema);
            async move { decode_result(self.request(request).await?) }
        })
        .await
    }

    fn require_advertisement(&self) -> Result<(), EventsError> {
        if self.peer.peer_info().is_some_and(|peer| {
            peer.protocol_version >= ProtocolVersion::V_2026_07_28
                && peer
                    .capabilities
                    .extensions
                    .as_ref()
                    .is_some_and(|extensions| extensions.contains_key(EXTENSION))
        }) {
            Ok(())
        } else {
            Err(EventsError::MethodNotFound)
        }
    }

    async fn request(&self, request: CustomRequest) -> Result<ServerResult, EventsError> {
        self.peer
            .send_request_with_option(
                ClientRequest::CustomRequest(request),
                PeerRequestOptions::with_timeout(REQUEST_BUDGET),
            )
            .await
            .map_err(from_service)?
            .await_response()
            .await
            .map_err(from_service)
    }

    async fn invalidate_catalog(&self) {
        self.catalog.lock().await.clear();
    }

    async fn with_route<T, F: std::future::Future<Output = Result<T, EventsError>>>(
        &self,
        name: &str,
        request: impl Fn(CatalogEntry) -> F,
    ) -> Result<T, EventsError> {
        match request(self.descriptor_cached(name, false).await?).await {
            Err(EventsError::HeaderMismatch(_)) => {
                request(self.descriptor_cached(name, true).await?).await
            }
            result => result,
        }
    }

    async fn descriptor_cached(
        &self,
        name: &str,
        refresh: bool,
    ) -> Result<CatalogEntry, EventsError> {
        let mut cached = self.catalog.lock().await;
        if refresh || !cached.contains_key(name) {
            cached.clear();
            *cached = self.load_catalog().await?;
        }
        cached.get(name).cloned().ok_or_else(not_found)
    }

    async fn load_catalog(&self) -> Result<HashMap<String, CatalogEntry>, EventsError> {
        let mut catalog = HashMap::new();
        let mut cursor = None;
        for _ in 0..DEFAULT_LIST_ALL_MAX_PAGES {
            let page = self.request_page(cursor.as_deref()).await?;
            for descriptor in page.events {
                catalog.insert(
                    descriptor.name,
                    CatalogEntry {
                        delivery: descriptor.delivery,
                        input_schema: descriptor.input_schema,
                    },
                );
            }
            let Some(next) = page.next_cursor else {
                return Ok(catalog);
            };
            cursor = Some(next);
        }
        Err(from_service(ServiceError::ListPaginationExceeded {
            max_pages: DEFAULT_LIST_ALL_MAX_PAGES,
        }))
    }
}

fn from_service(error: ServiceError) -> EventsError {
    match error {
        ServiceError::McpError(error) => EventsError::from(error),
        error => EventsError::Service(Box::new(error)),
    }
}

fn decode_result<T: DeserializeOwned>(result: ServerResult) -> Result<T, EventsError> {
    let ServerResult::CustomResult(result) = result else {
        return Err(EventsError::UnexpectedResponse);
    };
    serde_json::from_value(result.0).map_err(|_| EventsError::UnexpectedResponse)
}

fn encode_params(params: &impl Serialize) -> Result<Value, EventsError> {
    serde_json::to_value(params)
        .map_err(|_| EventsError::InvalidParams("invalid Events parameters".into()))
}

fn route(method: &str, params: Value, schema: Arc<JsonObject>) -> CustomRequest {
    let request = CustomRequest::new(method, Some(params));
    #[cfg(feature = "transport-streamable-http-client")]
    let request = {
        let mut request = request;
        request.extensions.insert(RoutingSchema(Some(schema)));
        request
    };
    #[cfg(not(feature = "transport-streamable-http-client"))]
    let _ = schema;
    request
}

#[cfg(all(test, feature = "transport-streamable-http-client"))]
mod tests {
    use super::*;

    #[test]
    fn malformed_entries_are_excluded_before_their_headers_are_used() {
        let event = |name, field, annotation| {
            json!({
                "name": name, "delivery": ["poll"], "payloadSchema": {},
                "inputSchema": {"properties": {"scope": {"type": field, "x-mcp-header": annotation}}}
            })
        };
        let result: ListEventsResult = serde_json::from_value(json!({
            "resultType": "complete", "nextCursor": "next",
            "events": [
                event("valid", "string", "Scope"),
                event("invalid-token", "string", "bad token"),
                event("invalid-schema", "number", "Scope"),
                {"name":"missing-payload","delivery":["poll"],"inputSchema":{}}
            ]
        }))
        .unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].name, "valid");
        assert_eq!(result.next_cursor.as_deref(), Some("next"));
    }
}
