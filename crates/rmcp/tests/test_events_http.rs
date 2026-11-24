#![cfg(all(
    feature = "events",
    feature = "server",
    feature = "client",
    feature = "transport-streamable-http-server",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local"),
))]

#[cfg(feature = "events-webhook")]
#[path = "common/events_webhook.rs"]
mod webhook;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::StreamExt;
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ServerHandler,
    events::{
        client::{EventUpdate, EventsClient, SubscriptionOptions},
        server::{CheckedBatch, Descriptor, EventsServer, Replacement},
        wire::{DeliveryMode, EventDescriptor, EventsError, PollParams},
    },
    model::{ClientConfig, JsonObject, ProtocolVersion},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService,
            session::{
                EventStore, EventStoreError, EventStream, ServerSseMessage,
                local::LocalSessionManager,
            },
        },
    },
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

type EventContext = rmcp::service::RequestContext<rmcp::RoleServer>;

#[derive(Clone)]
struct App(Option<EventsServer>, bool);

impl ServerHandler for App {
    fn events(&self) -> Option<&EventsServer> {
        self.0.as_ref()
    }

    async fn on_custom_request(
        &self,
        request: rmcp::model::CustomRequest,
        _: EventContext,
    ) -> Result<rmcp::model::CustomResult, rmcp::ErrorData> {
        if self.1 {
            if request
                .params
                .as_ref()
                .is_some_and(|params| params["fail"] == true)
            {
                return Err(rmcp::ErrorData::internal_error("app-owned error", None));
            }
            Ok(rmcp::model::CustomResult(
                json!({"appMethod": request.method}),
            ))
        } else {
            Err(rmcp::ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                request.method,
                None,
            ))
        }
    }
}

#[derive(Clone)]
struct Authenticated;

fn input_schema(annotation: &str) -> Arc<JsonObject> {
    Arc::new(
        json!({"type":"object","properties":{"region":{"type":"string","x-mcp-header":annotation}}})
            .as_object()
            .unwrap()
            .clone(),
    )
}

fn descriptor(name: &str, annotation: &str) -> Descriptor {
    Descriptor::raw(
        EventDescriptor::new(
            name,
            "",
            vec![DeliveryMode::Poll],
            input_schema(annotation),
            Arc::new(JsonObject::new()),
        ),
        |_| Ok(()),
    )
}

async fn server(app: App) -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let ct = CancellationToken::new();
    let service: StreamableHttpService<App, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(app.clone()),
        Default::default(),
        StreamableHttpServerConfig::default()
            .with_json_response(true)
            .with_cancellation_token(ct.child_token()),
    );
    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(axum::Extension(Authenticated));
    serve_router(router, ct).await
}

async fn serve_router(
    router: axum::Router,
    ct: CancellationToken,
) -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let stop = ct.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .unwrap();
    });
    (url, ct, task)
}

async fn raw(
    url: &str,
    method: &str,
    params: Value,
    headers: &[(&str, &str)],
) -> (reqwest::StatusCode, Value) {
    let response = raw_post(url, method, params, headers).send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap())
}

fn raw_post(
    url: &str,
    method: &str,
    params: Value,
    headers: &[(&str, &str)],
) -> reqwest::RequestBuilder {
    let mut params = params;
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name":"fixture","version":"1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let mut request = reqwest::Client::new()
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", method)
        .json(&json!({"jsonrpc":"2.0","id":"fixture","method":method,"params":params}));
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    request
}

fn push_source(name: &str) -> Descriptor {
    Descriptor::typed::<JsonObject, JsonObject>(name, "", [DeliveryMode::Push])
}

async fn push_server(
    app: App,
    store: Option<Arc<dyn EventStore>>,
) -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let ct = CancellationToken::new();
    let manager = LocalSessionManager::default();
    let manager = match store {
        Some(store) => manager.with_event_store(store),
        None => manager,
    };
    let service = StreamableHttpService::new(
        move || Ok(app.clone()),
        Arc::new(manager),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_sse_retry(Some(Duration::from_secs(1)))
            .with_cancellation_token(ct.child_token()),
    );
    serve_router(axum::Router::new().nest_service("/mcp", service), ct).await
}

#[tokio::test]
async fn push_first_error_is_json_without_changing_app_owned_stream_responses() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let runtime = EventsServer::new();
        runtime
            .register_check(
                push_source("source")
                    .authorize(|_, _, _| async { Err(EventsError::Forbidden { reason: None }) }),
                |_, _, _| async { panic!("unauthorized stream must not read source") },
            )
            .unwrap();
        let (url, stop, task) = push_server(App(Some(runtime), false), None).await;
        for (name, code) in [("source", -32024), ("missing", -32023)] {
            let response = raw_post(
                &url,
                "events/stream",
                json!({"name":name}),
                &[("mcp-name", name)],
            )
            .send()
            .await
            .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "application/json");
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["error"]["code"], code);
        }
        stop.cancel();
        task.await.unwrap();

        let (url, stop, task) = push_server(App(None, true), None).await;
        let response = raw_post(&url, "events/stream", json!({"name":"custom"}), &[])
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("\"appMethod\":\"events/stream\"")
        );
        let response = raw_post(
            &url,
            "events/stream",
            json!({"name":"custom", "fail":true}),
            &[],
        )
        .send()
        .await
        .unwrap();
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        assert!(response.text().await.unwrap().contains("app-owned error"));
        stop.cancel();
        task.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn concurrent_push_http_streams_release_unary_slots_and_cancel_independently() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let runtime = EventsServer::new();
        let first = runtime.register_emit(push_source("first")).unwrap();
        let second = runtime.register_emit(push_source("second")).unwrap();
        let (url, stop, task) = push_server(App(Some(runtime), false), None).await;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url).max_concurrent_requests(1),
        );
        let client = ClientConfig::default().serve_with_lifecycle(
            transport, ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28]
            },
        ).await.unwrap();
        let api = EventsClient::new(client.peer().clone());
        let mut a = api.subscribe("first", None, SubscriptionOptions::default()).await.unwrap();
        let mut b = api.subscribe("second", None, SubscriptionOptions::default()).await.unwrap();
        let Some(EventUpdate::Checkpoint(a_start)) = a.next().await else { panic!("first activation"); };
        let Some(EventUpdate::Checkpoint(b_start)) = b.next().await else { panic!("second activation"); };
        assert_ne!(a_start, b_start);
        assert_eq!(api.list_page(None).await.unwrap().events.len(), 2);
        first.emit(Some("a".into()), chrono::Utc::now(), JsonObject::new()).unwrap();
        second.emit(Some("b".into()), chrono::Utc::now(), JsonObject::new()).unwrap();
        assert!(matches!(a.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "a"));
        assert!(matches!(b.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "b"));
        drop(a);
        assert!(matches!(b.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor != b_start));
        second.emit(Some("continued".into()), chrono::Utc::now(), JsonObject::new()).unwrap();
        assert!(matches!(b.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "continued"));
        assert_eq!(api.list_page(None).await.unwrap().events.len(), 2);
        drop(b);
        client.cancel().await.unwrap();
        stop.cancel();
        task.await.unwrap();
    }).await.unwrap();
}

#[derive(Default)]
struct CountingStore(AtomicUsize);

#[async_trait::async_trait]
impl EventStore for CountingStore {
    async fn store_event(&self, _: &str, _: &ServerSseMessage) -> Result<String, EventStoreError> {
        Ok(self.0.fetch_add(1, Ordering::SeqCst).to_string())
    }

    async fn replay_events_after(&self, _: &str) -> Result<EventStream, EventStoreError> {
        panic!("push is never resumed in the EventStore")
    }
}

#[rstest::rstest]
#[case::active_without_store(false, true)]
#[case::active_with_store(true, true)]
#[case::activating_without_store(false, false)]
#[case::activating_with_store(true, false)]
#[tokio::test]
async fn abrupt_push_http_disconnect_cancels_source(
    #[case] event_store: bool,
    #[case] activate: bool,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let runtime = EventsServer::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let dropped = CancellationToken::new();
        runtime
            .register_check(push_source("source"), {
                let entered = entered.clone();
                let dropped = dropped.clone();
                move |_, _, query| {
                    let entered = entered.clone();
                    let dropped = dropped.clone();
                    async move {
                        if activate && query.cursor.as_deref() == Some("seed") {
                            let mut batch = CheckedBatch::new("ready");
                            batch.has_more = true;
                            return Ok(batch);
                        }
                        let _on_drop = dropped.drop_guard();
                        entered.notify_one();
                        std::future::pending().await
                    }
                }
            })
            .unwrap();
        let store = Arc::new(CountingStore::default());
        let events: Arc<dyn EventStore> = store.clone();
        let (url, stop, task) =
            push_server(App(Some(runtime), false), event_store.then_some(events)).await;
        let request = raw_post(
            &url,
            "events/stream",
            json!({"name":"source","cursor":"seed"}),
            &[("mcp-name", "source")],
        );
        if activate {
            let mut response = request.send().await.unwrap();
            assert_eq!(response.headers()["content-type"], "text/event-stream");
            let mut data = Vec::new();
            while !data.windows(2).any(|w| w == b"\n\n") {
                data.extend(response.chunk().await.unwrap().unwrap());
            }
            let data = String::from_utf8(data).unwrap();
            assert!(
                data.contains("\"method\":\"notifications/events/active\""),
                "{data}"
            );
            assert!(
                !data
                    .lines()
                    .any(|line| line.starts_with("id:") || line.starts_with("retry:")),
                "{data}"
            );
            entered.notified().await;
            drop(response);
        } else {
            let pending = tokio::spawn(request.send());
            entered.notified().await;
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
        }
        dropped.cancelled().await;
        assert_eq!(store.0.load(Ordering::SeqCst), 0);
        stop.cancel();
        task.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn push_http_eof_reopens_with_events_cursor_and_fresh_request_id() {
    use axum::{
        body::{Body, to_bytes},
        extract::Request,
        http::Response,
        middleware::{Next, from_fn},
    };

    tokio::time::timeout(Duration::from_secs(5), async {
        let runtime = EventsServer::new();
        runtime.register_emit(push_source("source")).unwrap();
        let ids = Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture = ids.clone();
        let middleware = from_fn(move |request: Request, next: Next| {
            let capture = capture.clone();
            async move {
                if request.headers().get("mcp-method").and_then(|header| header.to_str().ok()) != Some("events/stream") {
                    return next.run(request).await;
                }
                assert_eq!(request.method(), "POST");
                assert_eq!(request.headers()["mcp-name"], "source");
                assert!(!request.headers().contains_key("last-event-id"));
                let body = to_bytes(request.into_body(), 8192).await.unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                let id = request["id"].clone();
                let cursor = request["params"]["cursor"].clone();
                let first = {
                    let mut seen = capture.lock().unwrap();
                    seen.push((id.clone(), cursor));
                    seen.len() == 1
                };
                let frame = |method: &str, mut params: Value| {
                    params["_meta"] = json!({"io.modelcontextprotocol/subscriptionId":id});
                    let value = json!({
                        "jsonrpc":"2.0","method":format!("notifications/events/{method}"),"params":params
                    });
                    format!("data: {value}\n\n")
                };
                let event = |event: &str, cursor: &str| frame("event", json!({
                    "eventId":event, "name":"source", "timestamp":"2026-07-28T00:00:00Z",
                    "data":{}, "cursor":cursor
                }));
                let mut body = frame("active", json!({
                    "cursor":if first {"head"} else {"cursor-a"}, "heartbeatMs":30_000
                }));
                body += &event("a", "cursor-a");
                if !first {
                    body += &event("b", "cursor-b");
                    body += &frame("terminated", json!({
                        "error":{"code":-32024,"message":"Forbidden","data":{"reason":"revoked"}}
                    }));
                }
                Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(body)).unwrap()
            }
        });
        let stop = CancellationToken::new();
        let service: StreamableHttpService<App, LocalSessionManager> = StreamableHttpService::new(
            move || Ok(App(Some(runtime.clone()), false)),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_json_response(true)
                .with_legacy_session_mode(false)
                .with_cancellation_token(stop.child_token()),
        );
        let router = axum::Router::new().nest_service("/mcp", service).layer(middleware);
        let (url, stop, task) = serve_router(router, stop).await;
        let client = connect(url).await;
        let api = EventsClient::new(client.peer().clone());
        let mut events = api.subscribe("source", None, SubscriptionOptions::default()).await.unwrap();
        assert!(matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "head"));
        assert!(matches!(events.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "a"));
        assert!(matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "cursor-a"));
        assert!(matches!(events.next().await, Some(EventUpdate::RecoverableError(_))));
        for id in ["a", "b"] {
            assert!(matches!(events.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == id));
        }
        assert!(matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "cursor-b"));
        assert!(matches!(events.next().await, Some(EventUpdate::Terminated(EventsError::Forbidden { reason: Some(reason) })) if reason == "revoked"));
        assert!(events.next().await.is_none());
        {
            let seen = ids.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert_ne!(seen[0].0, seen[1].0);
            assert!(seen[0].1.is_null());
            assert_eq!(seen[1].1, "cursor-a");
        }
        client.cancel().await.unwrap();
        stop.cancel();
        task.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn headers_validate_and_client_relists_after_schema_replacement() {
    use base64::{Engine, prelude::BASE64_STANDARD};

    let runtime = EventsServer::new();
    let name = "é fixture";
    let region = " =?base64?hello?= ";
    let name_header = format!("=?base64?{}?=", BASE64_STANDARD.encode(name));
    let region_header = format!("=?base64?{}?=", BASE64_STANDARD.encode(region));
    let reads = Arc::new(AtomicUsize::new(0));
    let checks = Arc::new(AtomicUsize::new(0));
    let authorize = {
        let checks = checks.clone();
        move |context: EventContext, args: JsonObject, mode| {
            assert_eq!(mode, DeliveryMode::Poll);
            let parts = context
                .extensions
                .get::<axum::http::request::Parts>()
                .unwrap();
            assert!(parts.extensions.get::<Authenticated>().is_some());
            checks.fetch_add(1, Ordering::SeqCst);
            std::future::ready(if args.contains_key("deny") {
                Err(EventsError::Forbidden {
                    reason: Some("fixture denial".into()),
                })
            } else {
                Ok(())
            })
        }
    };
    runtime
        .register_check(descriptor(name, "Region").authorize(authorize.clone()), {
            let reads = reads.clone();
            move |_, args: JsonObject, _| {
                reads.fetch_add(1, Ordering::SeqCst);
                std::future::ready(if args.contains_key("busy") {
                    Err(EventsError::ResourceExhausted {
                        limit: "checkBudget".into(),
                        max: Some(1),
                    })
                } else {
                    Ok(CheckedBatch::new("now"))
                })
            }
        })
        .unwrap();

    let (url, stop, server) = server(App(Some(runtime.clone()), false)).await;
    let args = json!({"name":name,"arguments":{"region":region},"cursor":null});
    let (status, response) = raw(&url, "events/poll", args.clone(), &[]).await;
    assert_eq!(status, 400);
    assert_eq!(response["error"]["code"], -32020);
    for headers in [
        [
            ("mcp-name", "wrong"),
            ("mcp-param-region", region_header.as_str()),
        ],
        [
            ("mcp-name", name_header.as_str()),
            ("mcp-param-region", "wrong"),
        ],
    ] {
        let (status, response) = raw(&url, "events/poll", args.clone(), &headers).await;
        assert_eq!(status, 400);
        assert_eq!(response["error"]["code"], -32020);
    }
    for method in [
        "events/poll",
        "events/stream",
        "events/subscribe",
        "events/unsubscribe",
    ] {
        let (status, response) =
            raw(&url, method, args.clone(), &[("mcp-name", &name_header)]).await;
        assert_eq!(status, 400);
        assert_eq!(response["error"]["code"], -32020);
    }
    let (status, response) = raw(
        &url,
        "events/poll",
        args.clone(),
        &[
            ("mcp-name", &name_header),
            ("mcp-param-region", &region_header),
            ("mcp-param-region", &region_header),
        ],
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(response["error"]["code"], -32020);
    let mut deny = args.clone();
    deny["arguments"]["deny"] = json!(true);
    let (status, response) = raw(
        &url,
        "events/poll",
        deny,
        &[
            ("mcp-name", &name_header),
            ("mcp-param-region", &region_header),
        ],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["error"]["code"], -32024);
    assert_eq!(response["error"]["data"]["reason"], "fixture denial");
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let mut invalid_limit = args.clone();
    invalid_limit["maxEvents"] = json!(0);
    let (status, response) = raw(
        &url,
        "events/poll",
        invalid_limit,
        &[
            ("mcp-name", &name_header),
            ("mcp-param-region", &region_header),
        ],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["error"]["code"], -32602);
    let (status, response) = raw(
        &url,
        "events/stream",
        args.clone(),
        &[
            ("mcp-name", &name_header),
            ("mcp-param-region", &region_header),
        ],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["error"]["code"], -32026);
    assert_eq!(
        response["error"]["data"],
        json!({"feature":"deliveryMode","value":"push"})
    );
    let (status, response) = raw(
        &url,
        "events/poll",
        json!({"name":"missing"}),
        &[("mcp-name", "missing")],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["error"]["code"], -32023);
    assert_eq!(response["error"]["data"]["kind"], "event");
    let (status, response) = raw(&url, "events/list", json!({}), &[]).await;
    assert_eq!(status, 200);
    assert_eq!(response["result"]["events"][0]["name"], name);

    let client = connect(url.clone()).await;
    let events = EventsClient::new(client.peer().clone());
    let mut params = PollParams::new(name);
    params.arguments = Some(args["arguments"].as_object().unwrap().clone());
    assert_eq!(
        events.poll(params.clone()).await.unwrap().cursor.as_deref(),
        Some("now")
    );
    runtime
        .replace(
            descriptor(name, "Updated").authorize(authorize),
            Replacement::Additive,
        )
        .unwrap();
    // The client's cached Region header cannot pass the second authorization
    // unless it fetched the new descriptor and retried with Updated.
    assert_eq!(
        events.poll(params.clone()).await.unwrap().cursor.as_deref(),
        Some("now")
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(checks.load(Ordering::SeqCst), 3);
    let (status, response) = raw(
        &url,
        "events/poll",
        json!({"name": name, "arguments": {"busy": true}}),
        &[("mcp-name", &name_header)],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["error"]["code"], -32025);
    assert_eq!(
        response["error"]["data"],
        json!({"limit":"checkBudget","max":1})
    );
    assert!(matches!(
        runtime.replace(descriptor(name, "not a token"), Replacement::Additive),
        Err(EventsError::InvalidParams(_))
    ));

    client.cancel().await.unwrap();
    stop.cancel();
    server.await.unwrap();
}

#[tokio::test]
async fn push_reopens_after_relisting_changed_routing_headers() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let runtime = EventsServer::new();
        let annotated = |annotation: &str| {
            Descriptor::raw(
                EventDescriptor::new(
                    "source",
                    "",
                    vec![DeliveryMode::Push],
                    input_schema(annotation),
                    Default::default(),
                ),
                |_| Ok(()),
            )
        };
        runtime
            .register_check(annotated("Region"), |_, _, _| async {
                Ok(CheckedBatch::new("head"))
            })
            .unwrap();
        let (url, stop, task) = push_server(App(Some(runtime.clone()), false), None).await;
        let client = connect(url).await;
        let api = EventsClient::new(client.peer().clone());
        let arguments = json!({"region":"test"}).as_object().unwrap().clone();
        let mut stream = api
            .subscribe("source", Some(arguments.clone()), Default::default())
            .await
            .unwrap();
        assert!(
            matches!(stream.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "head")
        );
        drop(stream);
        runtime
            .replace(annotated("Updated"), Replacement::Additive)
            .unwrap();
        let mut stream = api
            .subscribe("source", Some(arguments.clone()), Default::default())
            .await
            .unwrap();
        assert!(
            matches!(stream.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "head")
        );

        runtime
            .replace(descriptor("source", "Updated"), Replacement::Breaking)
            .unwrap();
        assert!(matches!(stream.next().await, Some(EventUpdate::Terminated(
            EventsError::Unsupported { reason: Some(reason), .. }
        )) if reason == "schema_changed"));
        // The caller's cache must now know about the poll-only replacement.
        let mut options = SubscriptionOptions::default();
        options.mode = Some(DeliveryMode::Push);
        assert!(
            matches!(api.subscribe("source", Some(arguments), options).await,
            Err(EventsError::Unsupported { value: Some(value), .. }) if value == "push")
        );
        client.cancel().await.unwrap();
        stop.cancel();
        task.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn paginated_catalog_excludes_invalid_annotations() {
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, header},
        response::IntoResponse,
        routing::post,
    };
    #[derive(Default)]
    struct Observed {
        first: AtomicUsize,
        next: AtomicUsize,
        poll: AtomicUsize,
    }
    async fn serve(
        State(observed): State<Arc<Observed>>,
        headers: HeaderMap,
        bytes: Bytes,
    ) -> impl IntoResponse {
        let call: Value = serde_json::from_slice(&bytes).unwrap();
        let annotated = |name, header| {
            json!({
                "name": name, "delivery":["poll"], "payloadSchema": {},
                "inputSchema":{"properties":{
                    "count":{"type":"integer","x-mcp-header":header},
                    "enabled":{"type":"boolean","x-mcp-header":"Enabled"},
                }}
            })
        };
        let payload = match call["method"].as_str().unwrap() {
            "server/discover" => json!({"result":{
                "resultType":"complete","ttlMs":0,"cacheScope":"private",
                "supportedVersions":["2026-07-28"], "capabilities":{"extensions":{"io.modelcontextprotocol/events":{}}}
            }}),
            "events/list" => {
                assert!(!headers.contains_key("mcp-name"));
                if call["params"]["cursor"] == "next" {
                    observed.next.fetch_add(1, Ordering::SeqCst);
                    json!({"result":{"events":[annotated("valid","Count")]}})
                } else {
                    observed.first.fetch_add(1, Ordering::SeqCst);
                    json!({"result":{"events":[annotated("invalid","not a token")],"nextCursor":"next"}})
                }
            }
            "events/poll" => {
                let attempt = observed.poll.fetch_add(1, Ordering::SeqCst);
                assert_eq!(headers["mcp-name"], "valid");
                assert_eq!(headers["mcp-param-count"], "42");
                assert_eq!(headers["mcp-param-enabled"], "true");
                match attempt {
                    0 => json!({"error":{"code":-32020,"message":"routing changed"}}),
                    2 => json!({"result":{"resultType":"complete","cursor":"past-event","events":[
                        {"eventId":"unparseable","name":"valid","timestamp":"20261124T120000Z","data":{}}
                    ]}}),
                    _ => json!({"result":{"resultType":"complete","events":[],"cursor":"head"}}),
                }
            }
            other => panic!("unexpected fixture method {other}"),
        };
        let mut response = json!({"jsonrpc":"2.0","id":call["id"]});
        response
            .as_object_mut()
            .unwrap()
            .extend(payload.as_object().unwrap().clone());
        (
            [(header::CONTENT_TYPE, "application/json")],
            response.to_string(),
        )
    }
    let observed = Arc::new(Observed::default());
    let router = axum::Router::new()
        .route("/mcp", post(serve))
        .with_state(observed.clone());
    let (url, stop, task) = serve_router(router, CancellationToken::new()).await;
    let client = connect(url).await;
    let events = EventsClient::new(client.peer().clone());
    assert!(matches!(
        events.poll(PollParams::new("invalid")).await,
        Err(EventsError::NotFound { .. })
    ));
    assert_eq!(observed.poll.load(Ordering::SeqCst), 0);
    let mut params = PollParams::new("valid");
    params.arguments = json!({"count":42,"enabled":true}).as_object().cloned();
    events.poll(params.clone()).await.unwrap();
    assert_eq!(observed.poll.load(Ordering::SeqCst), 2);
    assert_eq!(observed.first.load(Ordering::SeqCst), 2);
    assert_eq!(observed.next.load(Ordering::SeqCst), 2);
    assert!(matches!(
        events.poll(params).await,
        Err(EventsError::UnexpectedResponse)
    ));
    client.cancel().await.unwrap();
    stop.cancel();
    task.await.unwrap();
}

async fn connect(url: String) -> rmcp::service::RunningService<rmcp::RoleClient, ClientConfig> {
    ClientConfig::default()
        .serve_with_lifecycle(
            StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url),
            ),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn numeric_and_boolean_mirrors_round_trip_through_tower() {
    let runtime = EventsServer::new();
    let schema = json!({"properties":{
        "count":{"type":"integer","x-mcp-header":"Count"},
        "enabled":{"type":"boolean","x-mcp-header":"Enabled"},
    }});
    let descriptor = Descriptor::raw(
        EventDescriptor::new(
            "counter",
            "",
            vec![DeliveryMode::Poll],
            schema.as_object().unwrap().clone().into(),
            Default::default(),
        ),
        |_| Ok(()),
    );
    runtime
        .register_check(descriptor, |_, _, _| {
            std::future::ready(Ok(CheckedBatch::new("head")))
        })
        .unwrap();
    let (url, stop, task) = server(App(Some(runtime), false)).await;
    let (status, result) = raw(
        &url,
        "events/poll",
        json!({"name":"counter","arguments":{"count":42,"enabled":true}}),
        &[
            ("mcp-name", "counter"),
            ("mcp-param-count", "42"),
            ("mcp-param-enabled", "true"),
        ],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(result["result"]["cursor"], "head");
    let (status, result) = raw(
        &url,
        "events/poll",
        json!({"name":"counter","arguments":{"count":42,"enabled":true}}),
        &[
            ("mcp-name", "counter"),
            ("mcp-param-count", "42.0"),
            ("mcp-param-enabled", "true"),
        ],
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(result["error"]["code"], -32020);
    let client = connect(url).await;
    let mut params = PollParams::new("counter");
    params.arguments = json!({"count":42.0,"enabled":false}).as_object().cloned();
    assert_eq!(
        EventsClient::new(client.peer().clone())
            .poll(params)
            .await
            .unwrap()
            .cursor
            .as_deref(),
        Some("head")
    );
    client.cancel().await.unwrap();
    stop.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn absent_runtime_is_method_not_found_over_modern_http() {
    let (url, stop, task) = server(App(None, false)).await;
    let (status, result) = raw(&url, "events/list", json!({}), &[]).await;
    assert_eq!(status, 404);
    assert_eq!(result["error"]["code"], -32601);
    stop.cancel();
    task.await.unwrap();

    let (url, stop, task) = server(App(None, true)).await;
    let (status, result) = raw(&url, "events/poll", json!({"name":"app-owned"}), &[]).await;
    assert_eq!(status, 200);
    assert_eq!(result["result"]["appMethod"], "events/poll");
    stop.cancel();
    task.await.unwrap();
}
