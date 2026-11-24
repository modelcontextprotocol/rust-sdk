#![cfg(all(
    feature = "events",
    feature = "server",
    feature = "client",
    feature = "transport-streamable-http-server",
    feature = "transport-streamable-http-client-reqwest",
    not(feature = "local"),
))]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ServerHandler,
    events::{
        client::EventsClient,
        server::{CheckedBatch, Descriptor, EventContext, EventsServer, Replacement},
        wire::{DeliveryMode, EventDescriptor, EventsError, PollParams},
    },
    model::{ClientConfig, JsonObject, ProtocolVersion, SubscriptionFilter},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

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
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap())
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
    let (status, response) = raw(
        &url,
        "events/poll",
        json!({"name":name,"arguments":{"region":true}}),
        &[("mcp-name", &name_header)],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(response["error"]["code"], -32602);
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

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.clone()),
    );
    let client = ClientConfig::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();
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
async fn scripted_catalog_restarts_once_and_excludes_invalid_annotations() {
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
        stale: AtomicUsize,
        poll: AtomicUsize,
        allow_invalid: std::sync::atomic::AtomicBool,
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
                if call["params"]["cursor"] == "stale" {
                    observed.stale.fetch_add(1, Ordering::SeqCst);
                    json!({"error":{"code":-32602,"message":"stale page"}})
                } else {
                    let first = observed.first.fetch_add(1, Ordering::SeqCst);
                    let annotation = if observed.allow_invalid.load(Ordering::SeqCst) {
                        "Count"
                    } else {
                        "not a token"
                    };
                    let mut result = json!({"resultType":"complete","events":[
                        annotated("valid","Count"), annotated("invalid",annotation),
                    ]});
                    if first == 1 {
                        result["nextCursor"] = json!("stale");
                    }
                    json!({"result":result})
                }
            }
            "events/poll" => {
                let attempt = observed.poll.fetch_add(1, Ordering::SeqCst);
                assert_eq!(headers["mcp-name"], "valid");
                assert_eq!(headers["mcp-param-count"], "42");
                assert_eq!(headers["mcp-param-enabled"], "true");
                match attempt {
                    1 => json!({"error":{"code":-32020,"message":"routing changed"}}),
                    3 => json!({"result":{"resultType":"complete","cursor":"past-event","events":[
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
    let page = events.list_page(None).await.unwrap();
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.name.as_str())
            .collect::<Vec<_>>(),
        ["valid"]
    );
    let poll_missing = || async {
        assert!(matches!(
            events.poll(PollParams::new("invalid")).await,
            Err(EventsError::NotFound { .. })
        ));
    };
    poll_missing().await;
    poll_missing().await;
    assert_eq!(observed.first.load(Ordering::SeqCst), 3);
    assert_eq!(observed.stale.load(Ordering::SeqCst), 1);
    assert_eq!(observed.poll.load(Ordering::SeqCst), 0);
    observed.allow_invalid.store(true, Ordering::SeqCst);
    assert!(matches!(
        events.poll(PollParams::new("other")).await,
        Err(EventsError::NotFound { .. })
    ));
    poll_missing().await; // Another miss refresh does not clear remembered names.
    assert_eq!(observed.first.load(Ordering::SeqCst), 4);
    observed.allow_invalid.store(false, Ordering::SeqCst);
    let mut params = PollParams::new("valid");
    params.arguments = json!({"count":42.0,"enabled":true}).as_object().cloned();
    events.poll(params.clone()).await.unwrap();
    assert_eq!(observed.poll.load(Ordering::SeqCst), 1);
    events.poll(params.clone()).await.unwrap();
    assert_eq!(observed.poll.load(Ordering::SeqCst), 3);
    assert_eq!(observed.first.load(Ordering::SeqCst), 5);
    assert!(matches!(
        events.poll(params).await,
        Err(EventsError::UnexpectedResponse)
    ));
    poll_missing().await;
    poll_missing().await;
    assert_eq!(observed.first.load(Ordering::SeqCst), 6);
    events.list_page(None).await.unwrap();
    poll_missing().await;
    poll_missing().await;
    assert_eq!(observed.first.load(Ordering::SeqCst), 8);
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
async fn http_catalog_notifications_and_legacy_initialize() {
    let runtime = EventsServer::new();
    let (url, stop, task) = server(App(Some(runtime.clone()), false)).await;
    let response = reqwest::Client::new().post(&url)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","clientInfo":{"name":"fixture","version":"1"},"capabilities":{}
        }})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    // Legacy sessions may stream the handshake in SSE rather than a JSON body.
    let body = tokio::time::timeout(std::time::Duration::from_secs(5), response.text())
        .await
        .unwrap()
        .unwrap();
    assert!(body.contains("protocolVersion"));
    assert!(!body.contains("io.modelcontextprotocol/events"));

    let client = connect(url).await;
    let mut stream = client
        .listen(SubscriptionFilter::builder().events_list_changed().build())
        .await
        .unwrap();
    assert_eq!(stream.acknowledged().events_list_changed, Some(true));
    runtime
        .register_check(descriptor("after-ack", "Region"), |_, _, _| {
            std::future::ready(Ok(CheckedBatch::new("head")))
        })
        .unwrap();
    let notice = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(notice, rmcp::model::ServerNotification::CustomNotification(value) if value.method == "notifications/events/list_changed")
    );
    stream.cancel().await.unwrap();
    client.cancel().await.unwrap();
    stop.cancel();
    task.await.unwrap();
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
            ("mcp-param-count", "42.0"),
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
            ("mcp-param-count", "42.000000000000000001"),
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
