use std::{convert::Infallible, fmt::Display, sync::Arc, time::Duration};

use bytes::{Buf, Bytes};
use http::Response;
use http_body::Body;
use http_body_util::{BodyExt, Empty, Full, combinators::UnsyncBoxBody};
use sse_stream::{KeepAlive, Sse, SseBody};

use super::http_header::EVENT_STREAM_MIME_TYPE;
use crate::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};

pub type SessionId = Arc<str>;

pub fn session_id() -> SessionId {
    uuid::Uuid::new_v4().to_string().into()
}

pub const DEFAULT_AUTO_PING_INTERVAL: Duration = Duration::from_secs(15);

pub(crate) type BoxResponse = Response<UnsyncBoxBody<Bytes, Infallible>>;

pub(crate) fn accepted_response() -> Response<UnsyncBoxBody<Bytes, Infallible>> {
    Response::builder()
        .status(http::StatusCode::ACCEPTED)
        .body(Empty::new().boxed_unsync())
        .expect("valid response")
}
pin_project_lite::pin_project! {
    struct TokioTimer {
        #[pin]
        sleep: tokio::time::Sleep,
    }
}
impl Future for TokioTimer {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.project();
        this.sleep.poll(cx)
    }
}
impl sse_stream::Timer for TokioTimer {
    fn from_duration(duration: Duration) -> Self {
        Self {
            sleep: tokio::time::sleep(duration),
        }
    }

    fn reset(self: std::pin::Pin<&mut Self>, when: std::time::Instant) {
        let this = self.project();
        this.sleep.reset(tokio::time::Instant::from_std(when));
    }
}

#[derive(Debug, Clone)]
pub struct ServerSseMessage {
    pub event_id: Option<String>,
    pub message: Arc<ServerJsonRpcMessage>,
}

pub(crate) fn sse_stream_response(
    stream: impl futures::Stream<Item = ServerSseMessage> + Send + 'static,
    keep_alive: Option<Duration>,
) -> Response<UnsyncBoxBody<Bytes, Infallible>> {
    use futures::StreamExt;
    let stream = SseBody::new(stream.map(|message| {
        let data = serde_json::to_string(&message.message).expect("valid message");
        let mut sse = Sse::default().data(data);
        sse.id = message.event_id;
        Result::<Sse, Infallible>::Ok(sse)
    }));
    let stream = match keep_alive {
        Some(duration) => stream
            .with_keep_alive::<TokioTimer>(KeepAlive::new().interval(duration))
            .boxed_unsync(),
        None => stream.boxed_unsync(),
    };
    Response::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, EVENT_STREAM_MIME_TYPE)
        .header(http::header::CACHE_CONTROL, "no-cache")
        .body(stream)
        .expect("valid response")
}

pub(crate) const fn internal_error_response<E: Display>(
    context: &str,
) -> impl FnOnce(E) -> Response<UnsyncBoxBody<Bytes, Infallible>> {
    move |error| {
        tracing::error!("Internal server error when {context}: {error}");
        Response::builder()
            .status(http::StatusCode::INTERNAL_SERVER_ERROR)
            .body(
                Full::new(Bytes::from(format!(
                    "Encounter an error when {context}: {error}"
                )))
                .boxed_unsync(),
            )
            .expect("valid response")
    }
}

pub(crate) async fn expect_json<B>(
    body: B,
) -> Result<ClientJsonRpcMessage, Response<UnsyncBoxBody<Bytes, Infallible>>>
where
    B: Body + Send + 'static,
    B::Error: Display,
{
    match body.collect().await {
        Ok(bytes) => {
            match serde_json::from_reader::<_, ClientJsonRpcMessage>(bytes.aggregate().reader()) {
                Ok(message) => Ok(message),
                Err(e) => {
                    let response = Response::builder()
                        .status(http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
                        .body(
                            Full::new(Bytes::from(format!("fail to deserialize request body {e}")))
                                .boxed_unsync(),
                        )
                        .expect("valid response");
                    Err(response)
                }
            }
        }
        Err(e) => {
            let response = Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body(
                    Full::new(Bytes::from(format!("Failed to read request body: {e}")))
                        .boxed_unsync(),
                )
                .expect("valid response");
            Err(response)
        }
    }
}
