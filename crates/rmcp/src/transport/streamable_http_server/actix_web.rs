use std::sync::Arc;

use actix_web::{
    HttpRequest, HttpResponse, Result,
    error::InternalError,
    http::{StatusCode, header},
    middleware,
    web::{self, Bytes, Data},
};
use futures::{Stream, StreamExt};
use tokio_stream::wrappers::ReceiverStream;

use super::{StreamableHttpServerConfig, session::SessionManager};
use crate::{
    RoleServer,
    model::{ClientJsonRpcMessage, ClientRequest},
    serve_server,
    service::serve_directly,
    transport::{
        OneshotTransport, TransportAdapterIdentity,
        common::http_header::{
            EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID,
            HEADER_X_ACCEL_BUFFERING, JSON_MIME_TYPE,
        },
    },
};

#[derive(Clone)]
pub struct StreamableHttpService<S, M = super::session::local::LocalSessionManager> {
    pub config: StreamableHttpServerConfig,
    session_manager: Arc<M>,
    service_factory: Arc<dyn Fn() -> Result<S, std::io::Error> + Send + Sync>,
}

impl<S, M> StreamableHttpService<S, M>
where
    S: crate::Service<RoleServer> + Send + 'static,
    M: SessionManager + 'static,
{
    pub fn new(
        service_factory: impl Fn() -> Result<S, std::io::Error> + Send + Sync + 'static,
        session_manager: Arc<M>,
        config: StreamableHttpServerConfig,
    ) -> Self {
        Self {
            config,
            session_manager,
            service_factory: Arc::new(service_factory),
        }
    }

    fn get_service(&self) -> Result<S, std::io::Error> {
        (self.service_factory)()
    }

    /// Configure actix_web routes for the streamable HTTP server
    pub fn configure(service: Arc<Self>) -> impl FnOnce(&mut web::ServiceConfig) {
        move |cfg: &mut web::ServiceConfig| {
            cfg.service(
                web::scope("/")
                    .app_data(Data::new(service.clone()))
                    .wrap(middleware::NormalizePath::trim())
                    .route("", web::get().to(Self::handle_get))
                    .route("", web::post().to(Self::handle_post))
                    .route("", web::delete().to(Self::handle_delete)),
            );
        }
    }

    async fn handle_get(
        req: HttpRequest,
        service: Data<Arc<StreamableHttpService<S, M>>>,
    ) -> Result<HttpResponse> {
        // Check accept header
        let accept = req
            .headers()
            .get(header::ACCEPT)
            .and_then(|h| h.to_str().ok());

        if !accept.is_some_and(|header| header.contains(EVENT_STREAM_MIME_TYPE)) {
            return Ok(HttpResponse::NotAcceptable()
                .body("Not Acceptable: Client must accept text/event-stream"));
        }

        // Check session id
        let session_id = req
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned().into());

        let Some(session_id) = session_id else {
            return Ok(HttpResponse::Unauthorized().body("Unauthorized: Session ID is required"));
        };

        tracing::debug!(%session_id, "GET request for SSE stream");

        // Check if session exists
        let has_session = service
            .session_manager
            .has_session(&session_id)
            .await
            .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?;

        if !has_session {
            return Ok(HttpResponse::Unauthorized().body("Unauthorized: Session not found"));
        }

        // Check if last event id is provided
        let last_event_id = req
            .headers()
            .get(HEADER_LAST_EVENT_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());

        // Get the appropriate stream
        let sse_stream: std::pin::Pin<Box<dyn Stream<Item = _> + Send>> =
            if let Some(last_event_id) = last_event_id {
                tracing::debug!(%session_id, %last_event_id, "Resuming stream from last event");
                Box::pin(
                    service
                        .session_manager
                        .resume(&session_id, last_event_id)
                        .await
                        .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?,
                )
            } else {
                tracing::debug!(%session_id, "Creating standalone stream");
                Box::pin(
                    service
                        .session_manager
                        .create_standalone_stream(&session_id)
                        .await
                        .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?,
                )
            };

        // Convert to SSE format
        let keep_alive = service.config.sse_keep_alive;
        let sse_stream = async_stream::stream! {
            let mut stream = sse_stream;
            let mut keep_alive_timer = keep_alive.map(|duration| tokio::time::interval(duration));

            loop {
                tokio::select! {
                    Some(msg) = stream.next() => {
                        let data = serde_json::to_string(&msg.message)
                            .unwrap_or_else(|_| "{}".to_string());
                        let mut output = String::new();
                        if let Some(id) = msg.event_id {
                            output.push_str(&format!("id: {}\n", id));
                        }
                        output.push_str(&format!("data: {}\n\n", data));
                        yield Ok::<_, actix_web::Error>(Bytes::from(output));
                    }
                    _ = async {
                        match keep_alive_timer.as_mut() {
                            Some(timer) => {
                                timer.tick().await;
                            }
                            None => {
                                std::future::pending::<()>().await;
                            }
                        }
                    } => {
                        yield Ok(Bytes::from(":ping\n\n"));
                    }
                    else => break,
                }
            }
        };

        Ok(HttpResponse::Ok()
            .content_type("text/event-stream")
            .append_header(("Cache-Control", "no-cache"))
            .append_header(("X-Accel-Buffering", "no"))
            .streaming(sse_stream))
    }

    async fn handle_post(
        req: HttpRequest,
        body: Bytes,
        service: Data<Arc<StreamableHttpService<S, M>>>,
    ) -> Result<HttpResponse> {
        // Check accept header
        let accept = req
            .headers()
            .get(header::ACCEPT)
            .and_then(|h| h.to_str().ok());

        if !accept.is_some_and(|header| {
            header.contains(JSON_MIME_TYPE) && header.contains(EVENT_STREAM_MIME_TYPE)
        }) {
            return Ok(HttpResponse::NotAcceptable().body(
                "Not Acceptable: Client must accept both application/json and text/event-stream",
            ));
        }

        // Check content type
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok());

        if !content_type.is_some_and(|header| header.starts_with(JSON_MIME_TYPE)) {
            return Ok(HttpResponse::UnsupportedMediaType()
                .body("Unsupported Media Type: Content-Type must be application/json"));
        }

        // Deserialize the message
        let mut message: ClientJsonRpcMessage = serde_json::from_slice(&body)
            .map_err(|e| InternalError::new(e, StatusCode::BAD_REQUEST))?;

        tracing::debug!(?message, "POST request with message");

        if service.config.stateful_mode {
            // Check session id
            let session_id = req
                .headers()
                .get(HEADER_SESSION_ID)
                .and_then(|v| v.to_str().ok());

            if let Some(session_id) = session_id {
                let session_id = session_id.to_owned().into();
                tracing::debug!(%session_id, "POST request with existing session");

                let has_session = service
                    .session_manager
                    .has_session(&session_id)
                    .await
                    .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?;

                if !has_session {
                    tracing::warn!(%session_id, "Session not found");
                    return Ok(HttpResponse::Unauthorized().body("Unauthorized: Session not found"));
                }

                // Note: In actix-web we can't inject request parts like in tower,
                // but session_id is already available through headers

                match message {
                    ClientJsonRpcMessage::Request(_) => {
                        let stream = service
                            .session_manager
                            .create_stream(&session_id, message)
                            .await
                            .map_err(|e| {
                                InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR)
                            })?;

                        // Convert to SSE format
                        let keep_alive = service.config.sse_keep_alive;
                        let sse_stream = async_stream::stream! {
                            let mut stream = Box::pin(stream);
                            let mut keep_alive_timer = keep_alive.map(|duration| tokio::time::interval(duration));

                            loop {
                                tokio::select! {
                                    Some(msg) = stream.next() => {
                                        let data = serde_json::to_string(&msg.message)
                                            .unwrap_or_else(|_| "{}".to_string());
                                        let mut output = String::new();
                                        if let Some(id) = msg.event_id {
                                            output.push_str(&format!("id: {}\n", id));
                                        }
                                        output.push_str(&format!("data: {}\n\n", data));
                                        yield Ok::<_, actix_web::Error>(Bytes::from(output));
                                    }
                                    _ = async {
                                        match keep_alive_timer.as_mut() {
                                            Some(timer) => {
                                                timer.tick().await;
                                            }
                                            None => {
                                                std::future::pending::<()>().await;
                                            }
                                        }
                                    } => {
                                        yield Ok(Bytes::from(":ping\n\n"));
                                    }
                                    else => break,
                                }
                            }
                        };

                        Ok(HttpResponse::Ok()
                            .content_type(EVENT_STREAM_MIME_TYPE)
                            .append_header((header::CACHE_CONTROL, "no-cache"))
                            .append_header((HEADER_X_ACCEL_BUFFERING, "no"))
                            .streaming(sse_stream))
                    }
                    ClientJsonRpcMessage::Notification(_)
                    | ClientJsonRpcMessage::Response(_)
                    | ClientJsonRpcMessage::Error(_) => {
                        // Handle notification
                        service
                            .session_manager
                            .accept_message(&session_id, message)
                            .await
                            .map_err(|e| {
                                InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR)
                            })?;

                        Ok(HttpResponse::Accepted().finish())
                    }
                    ClientJsonRpcMessage::BatchRequest(_)
                    | ClientJsonRpcMessage::BatchResponse(_) => {
                        Ok(HttpResponse::NotImplemented()
                            .body("Batch requests are not supported yet"))
                    }
                }
            } else {
                // No session id in stateful mode - create new session
                tracing::debug!("POST request without session, creating new session");

                let (session_id, transport) = service
                    .session_manager
                    .create_session()
                    .await
                    .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?;

                tracing::info!(%session_id, "Created new session");

                if let ClientJsonRpcMessage::Request(req) = &mut message {
                    if !matches!(req.request, ClientRequest::InitializeRequest(_)) {
                        return Ok(
                            HttpResponse::UnprocessableEntity().body("Expected initialize request")
                        );
                    }
                } else {
                    return Ok(
                        HttpResponse::UnprocessableEntity().body("Expected initialize request")
                    );
                }

                let service_instance = service
                    .get_service()
                    .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?;

                // Spawn a task to serve the session
                tokio::spawn({
                    let session_manager = service.session_manager.clone();
                    let session_id = session_id.clone();
                    async move {
                        let service = serve_server::<S, M::Transport, _, TransportAdapterIdentity>(
                            service_instance,
                            transport,
                        )
                        .await;
                        match service {
                            Ok(service) => {
                                let _ = service.waiting().await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to create service: {e}");
                            }
                        }
                        let _ = session_manager
                            .close_session(&session_id)
                            .await
                            .inspect_err(|e| {
                                tracing::error!("Failed to close session {session_id}: {e}");
                            });
                    }
                });

                // Get initialize response
                let response = service
                    .session_manager
                    .initialize_session(&session_id, message)
                    .await
                    .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?;

                // Return SSE stream with single response
                let sse_stream = async_stream::stream! {
                    yield Ok::<_, actix_web::Error>(Bytes::from(format!(
                        "data: {}\n\n",
                        serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string())
                    )));
                };

                Ok(HttpResponse::Ok()
                    .content_type(EVENT_STREAM_MIME_TYPE)
                    .append_header((header::CACHE_CONTROL, "no-cache"))
                    .append_header((HEADER_X_ACCEL_BUFFERING, "no"))
                    .append_header((HEADER_SESSION_ID, session_id.as_ref()))
                    .streaming(sse_stream))
            }
        } else {
            // Stateless mode
            tracing::debug!("POST request in stateless mode");

            match message {
                ClientJsonRpcMessage::Request(request) => {
                    tracing::debug!(?request, "Processing request in stateless mode");

                    // In stateless mode, handle the request directly
                    let service_instance = service
                        .get_service()
                        .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?;

                    let (transport, receiver) =
                        OneshotTransport::<RoleServer>::new(ClientJsonRpcMessage::Request(request));
                    let service_handle = serve_directly(service_instance, transport, None);

                    tokio::spawn(async move {
                        // Let the service process the request
                        let _ = service_handle.waiting().await;
                    });

                    // Convert receiver stream to SSE format
                    let sse_stream = ReceiverStream::new(receiver).map(|message| {
                        tracing::info!(?message);
                        let data =
                            serde_json::to_string(&message).unwrap_or_else(|_| "{}".to_string());
                        Ok::<_, actix_web::Error>(Bytes::from(format!("data: {}\n\n", data)))
                    });

                    // Add keep-alive if configured
                    let keep_alive = service.config.sse_keep_alive;
                    let sse_stream = async_stream::stream! {
                        let mut stream = Box::pin(sse_stream);
                        let mut keep_alive_timer = keep_alive.map(|duration| tokio::time::interval(duration));

                        loop {
                            tokio::select! {
                                Some(result) = stream.next() => {
                                    match result {
                                        Ok(data) => yield Ok(data),
                                        Err(e) => yield Err(e),
                                    }
                                }
                                _ = async {
                                    match keep_alive_timer.as_mut() {
                                        Some(timer) => {
                                            timer.tick().await;
                                        }
                                        None => {
                                            std::future::pending::<()>().await;
                                        }
                                    }
                                } => {
                                    yield Ok(Bytes::from(":ping\n\n"));
                                }
                                else => break,
                            }
                        }
                    };

                    Ok(HttpResponse::Ok()
                        .content_type(EVENT_STREAM_MIME_TYPE)
                        .append_header((header::CACHE_CONTROL, "no-cache"))
                        .append_header((HEADER_X_ACCEL_BUFFERING, "no"))
                        .streaming(sse_stream))
                }
                _ => Ok(HttpResponse::UnprocessableEntity().body("Unexpected message type")),
            }
        }
    }

    async fn handle_delete(
        req: HttpRequest,
        service: Data<Arc<StreamableHttpService<S, M>>>,
    ) -> Result<HttpResponse> {
        // Check session id
        let session_id = req
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned().into());

        let Some(session_id) = session_id else {
            return Ok(HttpResponse::Unauthorized().body("Unauthorized: Session ID is required"));
        };

        tracing::debug!(%session_id, "DELETE request to close session");

        // Close session
        service
            .session_manager
            .close_session(&session_id)
            .await
            .map_err(|e| InternalError::new(e, StatusCode::INTERNAL_SERVER_ERROR))?;

        tracing::info!(%session_id, "Session closed");

        Ok(HttpResponse::NoContent().finish())
    }
}
