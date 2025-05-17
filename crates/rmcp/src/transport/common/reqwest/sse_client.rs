use std::sync::Arc;

use futures::StreamExt;
use reqwest::header::ACCEPT;
use sse_stream::SseStream;

use crate::transport::{
    SseClientTransport,
    common::http_header::{EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID},
    sse_client::{SseClient, SseClientConfig, SseTransportError},
};

impl SseClient for reqwest::Client {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: std::sync::Arc<str>,
        message: crate::model::ClientJsonRpcMessage,
        auth_token: Option<String>,
    ) -> Result<(), SseTransportError<Self::Error>> {
        let mut request_builder = self.post(uri.as_ref()).json(&message);
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        request_builder
            .send()
            .await
            .and_then(|resp| resp.error_for_status())
            .map_err(SseTransportError::from)
            .map(drop)
    }

    async fn get_stream(
        &self,
        uri: std::sync::Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
    ) -> Result<
        crate::transport::common::client_side_sse::BoxedSseResponse,
        SseTransportError<Self::Error>,
    > {
        let mut request_builder = self
            .get(uri.as_ref())
            .header(ACCEPT, EVENT_STREAM_MIME_TYPE);
        if let Some(auth_header) = auth_token {
            request_builder = request_builder.bearer_auth(auth_header);
        }
        if let Some(last_event_id) = last_event_id {
            request_builder = request_builder.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        let response = request_builder.send().await?;
        let response = response.error_for_status()?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) => {
                if !ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) {
                    return Err(SseTransportError::UnexpectedContentType(Some(ct.clone())));
                }
            }
            None => {
                return Err(SseTransportError::UnexpectedContentType(None));
            }
        }
        let event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
        Ok(event_stream)
    }
}

impl SseClientTransport<reqwest::Client> {
    pub async fn start(
        uri: impl Into<Arc<str>>,
    ) -> Result<Self, SseTransportError<reqwest::Error>> {
        SseClientTransport::start_with_client(
            reqwest::Client::default(),
            SseClientConfig {
                uri: uri.into(),
                ..Default::default()
            },
        )
        .await
    }
}
