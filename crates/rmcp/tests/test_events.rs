#![cfg(all(feature = "events", feature = "server", feature = "client"))]

use std::{
    borrow::Cow,
    future::{pending, ready},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::StreamExt;
use jiff::Timestamp;
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ErrorData, RoleClient, ServerHandler, ServiceError,
    ServiceExt,
    events::{
        client::{EventUpdate, EventsClient, SubscriptionOptions},
        server::{CheckQuery, CheckedBatch, Descriptor, EventsServer, Replacement},
        wire::{DeliveryMode, EventDescriptor, EventOccurrence, EventsError, PollParams},
    },
    model::{
        ClientConfig, ClientRequest, CustomRequest, CustomResult, ErrorCode, GetMeta, JsonObject,
        ProtocolVersion, ServerConfig, ServerNotification, ServerResult, SubscriptionFilter,
    },
    service::{PeerRequestOptions, RunningService, SubscriptionContext, SubscriptionSendError},
};
use serde_json::{Value, json};

const EXTENSION: &str = "io.modelcontextprotocol/events";
type EventContext = rmcp::service::RequestContext<rmcp::RoleServer>;

fn raw_descriptor(
    name: &str,
    mode: DeliveryMode,
    input: JsonObject,
    payload: JsonObject,
) -> Descriptor {
    Descriptor::raw(
        EventDescriptor::new(name, "", vec![mode], input.into(), payload.into()),
        |_| Ok(()),
    )
}

fn object(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

fn descriptor(name: &str) -> Descriptor {
    Descriptor::typed::<JsonObject, JsonObject>(name, "test event", [DeliveryMode::Poll])
}

fn register(runtime: &EventsServer, name: &str) {
    runtime
        .register_check(descriptor(name), |_, _, _| {
            ready(Ok(CheckedBatch::new("head")))
        })
        .unwrap();
}

#[derive(Clone, Default)]
struct App {
    events: Option<EventsServer>,
    app_settings: Option<JsonObject>,
    custom: bool,
    change_during_listen: bool,
}

impl ServerHandler for App {
    fn events(&self) -> Option<&EventsServer> {
        self.events.as_ref()
    }

    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        let extensions = info.capabilities.extensions.get_or_insert_default();
        extensions.insert("fixture/another-extension".into(), JsonObject::new());
        if let Some(settings) = &self.app_settings {
            extensions.insert(EXTENSION.into(), settings.clone());
        }
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25])
    }

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _: EventContext,
    ) -> Result<CustomResult, ErrorData> {
        if self.custom {
            Ok(CustomResult(json!({"fromApp": request.method})))
        } else {
            Err(ErrorData::new(
                ErrorCode::METHOD_NOT_FOUND,
                request.method,
                None,
            ))
        }
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        if self.events.is_some() {
            // The SDK still honors its own category.
            None
        } else {
            assert_eq!(requested.events_list_changed, Some(true));
            Some(requested.clone())
        }
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        assert!(matches!(
            context
                .sink()
                .send(ServerNotification::CustomNotification(
                    rmcp::model::CustomNotification::new("notifications/events/other", None),
                ))
                .await,
            Err(SubscriptionSendError::UnsupportedNotification(
                "custom notification"
            ))
        ));
        if self.change_during_listen {
            let runtime = self.events.as_ref().unwrap();
            // The ack has been sent; the runtime watcher has not been polled.
            register(runtime, "new-a");
            register(runtime, "new-b");
        }
        context.cancelled().await;
        Ok(())
    }
}

async fn exchange(
    app: impl ServerHandler,
    mode: ClientLifecycleMode,
) -> RunningService<RoleClient, ClientConfig> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let server = async move { app.serve(server_io).await.unwrap().waiting().await };
    #[cfg(feature = "local")]
    tokio::task::spawn_local(server);
    #[cfg(not(feature = "local"))]
    tokio::spawn(server);
    ClientConfig::default()
        .serve_with_lifecycle(client_io, mode)
        .await
        .unwrap()
}

fn modern() -> ClientLifecycleMode {
    ClientLifecycleMode::Discover {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
    }
}

async fn call(
    client: &rmcp::Peer<RoleClient>,
    method: &str,
    params: Option<Value>,
) -> Result<ServerResult, ServiceError> {
    client
        .send_request(ClientRequest::CustomRequest(CustomRequest::new(
            method, params,
        )))
        .await
}

macro_rules! events_test {
    ($name:ident, $body:block) => {
        events_test!($name, 5, $body);
    };
    ($name:ident, $seconds:expr, $body:block) => {
        #[tokio::test(start_paused = true)]
        async fn $name() {
            tokio::task::LocalSet::new()
                .run_until(tokio::time::timeout(Duration::from_secs($seconds), async $body))
                .await.expect("duplex exchange made no progress");
        }
    }
}

const SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";

fn push_descriptor(name: &str) -> Descriptor {
    Descriptor::typed::<JsonObject, JsonObject>(name, "", [DeliveryMode::Push, DeliveryMode::Poll])
}

struct Wire {
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
}

impl Wire {
    async fn connect(runtime: EventsServer) -> Self {
        use tokio::io::AsyncBufReadExt;
        let (server, client) = tokio::io::duplex(64 * 1024);
        tokio::task::spawn_local(async move {
            let app = App {
                events: Some(runtime),
                ..Default::default()
            };
            app.serve(server).await.unwrap().waiting().await.unwrap();
        });
        let (read, write) = tokio::io::split(client);
        let mut wire = Self {
            lines: tokio::io::BufReader::new(read).lines(),
            write,
        };
        wire.request(
            json!("discover"),
            "server/discover",
            json!({
                "protocolVersions":["2026-07-28"]
            }),
        )
        .await;
        let result = wire.receive().await;
        assert!(result["result"].is_object(), "{result}");
        wire
    }

    async fn request(&mut self, id: Value, method: &str, mut params: Value) {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientInfo":{"name":"wire","version":"1"},
            "io.modelcontextprotocol/clientCapabilities":{}
        });
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
    }

    async fn send(&mut self, message: Value) {
        use tokio::io::AsyncWriteExt;
        self.write
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    async fn receive(&mut self) -> Value {
        serde_json::from_str(&self.lines.next_line().await.unwrap().unwrap()).unwrap()
    }

    async fn frame(&mut self, id: Value, method: &str) -> Value {
        let message = self.receive().await;
        assert_eq!(
            message["method"],
            format!("notifications/events/{method}"),
            "{message}"
        );
        assert_eq!(message["params"]["_meta"][SUBSCRIPTION_ID], id);
        message["params"].clone()
    }
}

events_test!(push_stdio_isolates_streams_and_fences_final_after_cancel, {
    let runtime = EventsServer::new();
    let first = runtime.register_emit(push_descriptor("first")).unwrap();
    let second = runtime.register_emit(push_descriptor("second")).unwrap();
    let mut wire = Wire::connect(runtime.clone()).await;
    wire.request(
        json!("one"),
        "events/stream",
        json!({"name":"first", "heartbeatMs":0}),
    )
    .await;
    let one = wire.frame(json!("one"), "active").await;
    assert_eq!(one["heartbeatMs"], 1_000);
    wire.request(
        json!(2),
        "events/stream",
        json!({"name":"second","heartbeatMs":90_000}),
    )
    .await;
    let two = wire.frame(json!(2), "active").await;
    assert_eq!(two["heartbeatMs"], 30_000);
    assert_ne!(one["cursor"], two["cursor"]);
    wire.request(json!(3), "events/list", json!({})).await;
    let unary = wire.receive().await;
    assert_eq!(unary["id"], 3);
    assert_eq!(unary["result"]["events"].as_array().unwrap().len(), 2);

    first
        .emit(
            Some("a".into()),
            Timestamp::now().to_string(),
            JsonObject::new(),
        )
        .unwrap();
    let event = wire.frame(json!("one"), "event").await;
    assert_eq!(event["eventId"], "a");
    assert_ne!(event["cursor"], one["cursor"]);
    second
        .emit(
            Some("b".into()),
            Timestamp::now().to_string(),
            JsonObject::new(),
        )
        .unwrap();
    assert_eq!(wire.frame(json!(2), "event").await["eventId"], "b");
    assert_eq!(
        wire.frame(json!("one"), "heartbeat").await["cursor"],
        event["cursor"]
    );

    wire.send(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"one"}}),
    )
    .await;
    wire.request(json!(4), "events/list", json!({})).await;
    let unary = wire.receive().await;
    assert_eq!(unary["id"], 4);
    assert_eq!(unary["result"]["events"].as_array().unwrap().len(), 2);
    first
        .emit(
            Some("discarded".into()),
            Timestamp::now().to_string(),
            JsonObject::new(),
        )
        .unwrap();
    second
        .emit(
            Some("continued".into()),
            Timestamp::now().to_string(),
            JsonObject::new(),
        )
        .unwrap();
    assert_eq!(wire.frame(json!(2), "event").await["eventId"], "continued");
    runtime.remove("second").unwrap();
    let terminated = wire.frame(json!(2), "terminated").await;
    assert_eq!(terminated["error"]["code"], -32023);
    assert_eq!(terminated["error"]["data"], json!({"kind":"event"}));
    assert_eq!(
        wire.receive().await,
        json!({
            "jsonrpc":"2.0","id":2,"result":{"resultType":"complete","_meta":{SUBSCRIPTION_ID:2}}
        })
    );
});

events_test!(push_gap_precedes_replay_and_regrants_heartbeat, {
    let runtime = EventsServer::new();
    let emit = runtime.register_emit(push_descriptor("source")).unwrap();
    let mut wire = Wire::connect(runtime).await;
    let now = Timestamp::now();
    wire.request(json!(1), "events/stream", json!({"name":"source"}))
        .await;
    let start = wire.frame(json!(1), "active").await["cursor"].clone();
    for (id, timestamp) in [
        ("a", now),
        ("old", now - Duration::from_secs(86_400)),
        ("b", now),
    ] {
        emit.emit(Some(id.into()), timestamp.to_string(), JsonObject::new())
            .unwrap();
        assert_eq!(wire.frame(json!(1), "event").await["eventId"], id);
    }
    wire.request(
        json!(2),
        "events/stream",
        json!({
            "name":"source", "cursor": start, "maxAgeMs": 10_000, "heartbeatMs":1234
        }),
    )
    .await;
    let active = wire.frame(json!(2), "active").await;
    assert_eq!(active["truncated"], false);
    assert_eq!(active["cursor"], start);
    let a = wire.frame(json!(2), "event").await;
    assert_eq!(a["eventId"], "a");
    let gap = wire.frame(json!(2), "active").await;
    assert_eq!(gap["truncated"], true);
    assert_eq!(gap["heartbeatMs"], 1234);
    assert_ne!(gap["cursor"], a["cursor"]);
    assert_eq!(wire.frame(json!(2), "event").await["eventId"], "b");
});

events_test!(
    push_recoverable_check_failure_then_authorization_revocation,
    35,
    {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
        };
        let runtime = EventsServer::new();
        let revoke = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(AtomicUsize::new(0));
        runtime
            .register_check(
                push_descriptor("source").authorize({
                    let revoke = revoke.clone();
                    move |_, _, mode| {
                        assert_eq!(mode, DeliveryMode::Push);
                        ready(if revoke.load(SeqCst) {
                            Err(EventsError::Forbidden {
                                reason: Some("database policy".into()),
                            })
                        } else {
                            Ok(())
                        })
                    }
                }),
                move |_, _, query| {
                    let mut batch = CheckedBatch::new("head");
                    batch.next_poll_ms = Some(1_000);
                    ready(match checks.fetch_add(1, SeqCst) {
                        0 => Ok(batch),
                        1 => Err(EventsError::Remote(Box::new(ErrorData::internal_error(
                            "fetch unavailable",
                            None,
                        )))),
                        2 => {
                            assert_eq!(query.cursor.as_deref(), Some("head"));
                            batch.end = "recovered".into();
                            batch.next_poll_ms = Some(60_000);
                            batch.events.push(EventOccurrence::new(
                                "recovered",
                                "source",
                                Timestamp::now().to_string(),
                                JsonObject::new(),
                            ));
                            Ok(batch)
                        }
                        _ => panic!("read must not run after revocation"),
                    })
                },
            )
            .unwrap();
        let mut wire = Wire::connect(runtime).await;
        let start = tokio::time::Instant::now();
        wire.request(json!(1), "events/stream", json!({"name":"source"}))
            .await;
        wire.frame(json!(1), "active").await;
        assert_eq!(wire.frame(json!(1), "error").await["error"]["code"], -32603);
        assert_eq!(wire.frame(json!(1), "event").await["eventId"], "recovered");
        revoke.store(true, SeqCst);
        let final_notice = wire.frame(json!(1), "terminated").await;
        assert_eq!(start.elapsed(), Duration::from_secs(30));
        assert_eq!(final_notice["error"]["code"], -32024);
        assert_eq!(
            final_notice["error"]["data"],
            json!({"reason":"database policy"})
        );
        assert_eq!(wire.receive().await["result"]["_meta"][SUBSCRIPTION_ID], 1);
    }
);

events_test!(push_age_floor_ends_after_immediate_backlog_drain, {
    use std::sync::{Arc, Mutex};
    let runtime = EventsServer::new();
    let reads = Arc::new(Mutex::new(Vec::new()));
    runtime
        .register_check(push_descriptor("source"), {
            let reads = reads.clone();
            move |_, _, query| {
                reads
                    .lock()
                    .unwrap()
                    .push((tokio::time::Instant::now(), query.age_floor.is_some()));
                let mut page = CheckedBatch::new("1");
                page.next_poll_ms = Some(1_000);
                match query.cursor.as_deref() {
                    Some("0") => page.has_more = true,
                    Some("1") => {
                        page.end = "2".into();
                        page.events.push(EventOccurrence::new(
                            "replay",
                            "source",
                            Timestamp::now().to_string(),
                            JsonObject::new(),
                        ));
                    }
                    Some("2") => {
                        page.end = "3".into();
                        page.events.push(EventOccurrence::new(
                            "live",
                            "source",
                            (Timestamp::now() - Duration::from_secs(86_400)).to_string(),
                            JsonObject::new(),
                        ));
                    }
                    other => panic!("unexpected cursor {other:?}"),
                }
                ready(Ok(page))
            }
        })
        .unwrap();
    let mut wire = Wire::connect(runtime).await;
    wire.request(
        json!(1),
        "events/stream",
        json!({"name":"source","cursor":"0","maxAgeMs":5_000}),
    )
    .await;
    assert_eq!(wire.frame(json!(1), "active").await["cursor"], "1");
    assert_eq!(wire.frame(json!(1), "event").await["eventId"], "replay");
    assert_eq!(wire.frame(json!(1), "event").await["eventId"], "live");
    let reads = reads.lock().unwrap();
    assert_eq!(
        reads.iter().map(|(_, floor)| *floor).collect::<Vec<_>>(),
        [true, true, false]
    );
    assert_eq!(reads[0].0, reads[1].0);
    assert_eq!(reads[2].0 - reads[1].0, Duration::from_secs(1));
});

events_test!(push_heartbeats_and_terminates_during_pending_read, {
    let runtime = EventsServer::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    runtime
        .register_check(push_descriptor("source"), {
            let entered = entered.clone();
            move |_, _, query| {
                let entered = entered.clone();
                async move {
                    if query.cursor.is_some() {
                        entered.notify_one();
                        return pending().await;
                    }
                    let mut page = CheckedBatch::new("head");
                    page.next_poll_ms = Some(1_000);
                    Ok(page)
                }
            }
        })
        .unwrap();
    let mut wire = Wire::connect(runtime.clone()).await;
    wire.request(
        json!(1),
        "events/stream",
        json!({"name":"source","heartbeatMs":1000}),
    )
    .await;
    wire.frame(json!(1), "active").await;
    entered.notified().await;
    assert_eq!(wire.frame(json!(1), "heartbeat").await["cursor"], "head");
    runtime.remove("source").unwrap();
    assert_eq!(
        wire.frame(json!(1), "terminated").await["error"]["code"],
        -32023
    );
});

events_test!(push_replacement, {
    let mode = DeliveryMode::Push;
    let changed = object(json!({"type":"object","required":["new"]}));
    for (schema, expected) in [
        (JsonObject::new(), "payloadSchema"),
        (changed, "inputSchema"),
    ] {
        let runtime = EventsServer::new();
        let source = || raw_descriptor("source", mode, JsonObject::new(), JsonObject::new());
        let emit = runtime.register_emit(source()).unwrap();
        let mut wire = Wire::connect(runtime.clone()).await;
        wire.request(json!(1), "events/poll", json!({"name":"source"}))
            .await;
        let response = wire.receive().await;
        assert_eq!(response["error"]["code"], -32026);
        assert_eq!(
            response["error"]["data"],
            json!({"feature":"deliveryMode","value":"poll"})
        );
        wire.request(json!(2), "events/stream", json!({"name":"source"}))
            .await;
        wire.frame(json!(2), "active").await;
        runtime.replace(source(), Replacement::Additive).unwrap();
        emit.emit(
            Some("still-open".into()),
            Timestamp::now().to_string(),
            JsonObject::new(),
        )
        .unwrap();
        assert_eq!(wire.frame(json!(2), "event").await["eventId"], "still-open");
        runtime
            .replace(
                raw_descriptor("source", mode, schema, object(json!({"type":"object"}))),
                Replacement::Breaking,
            )
            .unwrap();
        let terminated = wire.frame(json!(2), "terminated").await;
        assert_eq!(terminated["error"]["code"], -32026);
        assert_eq!(
            terminated["error"]["data"],
            json!({"feature":expected,"reason":"schema_changed"})
        );
        assert_eq!(wire.receive().await["result"]["_meta"][SUBSCRIPTION_ID], 2);
    }
});

events_test!(stdio_eof_ends_delivery_at_the_last_checkpoint, {
    use tokio::io::AsyncBufReadExt;
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let stop = tokio_util::sync::CancellationToken::new();
    let task = tokio::task::spawn_local({
        let stop = stop.clone();
        async move {
            let (read, write) = tokio::io::split(server_io);
            let mut wire = Wire {
                lines: tokio::io::BufReader::new(read).lines(),
                write,
            };
            let discover = wire.receive().await;
            assert_eq!(discover["method"], "server/discover");
            wire.send(json!({"jsonrpc":"2.0","id":discover["id"],"result":{
                "resultType":"complete","ttlMs":0,"cacheScope":"private",
                "supportedVersions":["2026-07-28"],"capabilities":{"extensions":{EXTENSION:{}}}
            }}))
            .await;
            let catalog = wire.receive().await;
            assert_eq!(catalog["method"], "events/list");
            wire.send(json!({"jsonrpc":"2.0","id":catalog["id"],"result":{
                "resultType":"complete","events":[
                    {"name":"source","delivery":["push"],"inputSchema":{},"payloadSchema":{}}
                ]
            }}))
            .await;
            let stream = wire.receive().await;
            assert_eq!(stream["method"], "events/stream");
            wire.send(
                json!({"jsonrpc":"2.0","method":"notifications/events/active","params":{
                    "cursor":"head","heartbeatMs":30_000,"_meta":{SUBSCRIPTION_ID:stream["id"]}
                }}),
            )
            .await;
            stop.cancelled().await;
            drop(wire);
        }
    });
    let client = ClientConfig::default()
        .serve_with_lifecycle(client_io, modern())
        .await
        .unwrap();
    let api = EventsClient::new(client.peer().clone());
    let mut events = api
        .subscribe("source", None, Default::default())
        .await
        .unwrap();
    let Some(EventUpdate::Checkpoint(saved)) = events.next().await else {
        panic!("missing activation");
    };
    stop.cancel();
    task.await.unwrap();
    client.waiting().await.unwrap();
    assert_eq!(saved, "head");
    assert!(matches!(events.next().await, Some(EventUpdate::Terminated(
        EventsError::Service(error),
    )) if matches!(*error, ServiceError::TransportClosed)));
    assert!(events.next().await.is_none());
});

events_test!(
    push_heartbeats_track_quiet_progress_without_skipping_partial_check_batch,
    {
        let runtime = EventsServer::new();
        runtime
            .register_check(push_descriptor("source"), |_, _, query| async move {
                let sample = |id| {
                    EventOccurrence::new(
                        id,
                        "source",
                        Timestamp::now().to_string(),
                        JsonObject::new(),
                    )
                };
                let mut batch = CheckedBatch::new("batch-end");
                batch.next_poll_ms = Some(1_000);
                match query.cursor.as_deref() {
                    Some("saved") => {
                        let mut event = sample("a");
                        event.meta = Some(object(
                            json!({SUBSCRIPTION_ID:"wrong", "otherExtension":true}),
                        ));
                        batch.events.push(event);
                        batch.events.push(sample("b"));
                    }
                    Some("batch-end") => batch.end = "quiet-progress".into(),
                    Some("quiet-progress") => return std::future::pending().await,
                    other => panic!("unexpected cursor {other:?}"),
                }
                Ok(batch)
            })
            .unwrap();
        let mut wire = Wire::connect(runtime).await;
        wire.request(
            json!(1),
            "events/stream",
            json!({
                "name":"source","cursor":"saved","heartbeatMs":1_500
            }),
        )
        .await;
        assert_eq!(wire.frame(json!(1), "active").await["cursor"], "saved");
        let first = wire.frame(json!(1), "event").await;
        assert_eq!(first["eventId"], "a");
        assert_eq!(first["cursor"], "saved");
        assert_eq!(first["_meta"]["otherExtension"], true);
        let second = wire.frame(json!(1), "event").await;
        assert_eq!(second["eventId"], "b");
        assert_eq!(second["cursor"], "batch-end");
        assert_eq!(
            wire.frame(json!(1), "heartbeat").await["cursor"],
            "quiet-progress"
        );
        assert_eq!(
            wire.frame(json!(1), "heartbeat").await["cursor"],
            "quiet-progress"
        );
    }
);

#[tokio::test(start_paused = true)]
async fn push_activation_is_bounded_without_pre_activation_frames() {
    tokio::task::LocalSet::new().run_until(async {
        let runtime = EventsServer::new();
        for (name, delay) in [("slow", 9), ("late", 11)] {
            runtime.register_check(push_descriptor(name), move |_, _, _| async move {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                Ok(CheckedBatch::new("head"))
            }).unwrap();
        }
        let mut wire = Wire::connect(runtime).await;
        let before = tokio::time::Instant::now();
        wire.request(json!(1), "events/stream", json!({"name":"slow"})).await;
        assert_eq!(wire.frame(json!(1), "active").await["cursor"], "head");
        assert_eq!(before.elapsed(), Duration::from_secs(9));
        wire.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}})).await;
        let before = tokio::time::Instant::now();
        wire.request(json!(2), "events/stream", json!({"name":"late"})).await;
        let error = wire.receive().await;
        assert_eq!(error["id"], 2);
        assert_eq!(error["error"]["code"], -32603);
        assert_eq!(before.elapsed(), Duration::from_secs(10));
    }).await;
}

events_test!(managed_poll_drains_progress_then_observes_floor, {
    use std::sync::{Arc, Mutex};
    let clock = Arc::new(Mutex::new(Vec::new()));
    let runtime = EventsServer::new();
    runtime
        .register_check(push_descriptor("source"), {
            let clock = clock.clone();
            move |_, _, query| {
                clock.lock().unwrap().push(tokio::time::Instant::now());
                let mut batch = CheckedBatch::new("p0");
                batch.next_poll_ms = Some(0);
                match query.cursor.as_deref() {
                    None => {}
                    Some("p0") => {
                        batch.end = "p1".into();
                        batch.has_more = true;
                    }
                    Some("p1") => {
                        batch.end = "p2".into();
                        batch.events.push(EventOccurrence::new(
                            "a",
                            "source",
                            Timestamp::now().to_string(),
                            JsonObject::new(),
                        ));
                        batch.events.push(EventOccurrence::new(
                            "b",
                            "source",
                            Timestamp::now().to_string(),
                            JsonObject::new(),
                        ));
                    }
                    Some("p2") => batch.end = "p3".into(),
                    other => panic!("unexpected cursor {other:?}"),
                }
                ready(Ok(batch))
            }
        })
        .unwrap();
    let client = exchange(
        App {
            events: Some(runtime),
            ..Default::default()
        },
        modern(),
    )
    .await;
    let client_api = EventsClient::new(client.peer().clone());
    let mut options = SubscriptionOptions::default();
    options.mode = Some(DeliveryMode::Poll);
    let mut events = client_api.subscribe("source", None, options).await.unwrap();
    assert!(matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "p0"));
    assert!(matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "p1"));
    assert!(
        matches!(events.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "a")
    );
    assert!(
        matches!(events.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "b")
    );
    assert_eq!(clock.lock().unwrap().len(), 3);
    assert!(matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "p2"));
    assert!(matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "p3"));
    let at = clock.lock().unwrap();
    assert_eq!(at[1] - at[0], Duration::from_secs(1));
    assert_eq!(at[2], at[1]);
    assert_eq!(at[3] - at[2], Duration::from_secs(1));
});

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Watchdog,
    Overflow,
    ClosingPush,
    PushActivation,
    Poll,
}

#[derive(Clone)]
struct ScriptedPeer {
    scenario: Scenario,
    calls: std::sync::Arc<std::sync::Mutex<Vec<(tokio::time::Instant, Value)>>>,
    go: Arc<tokio::sync::Notify>,
    sent: Arc<tokio::sync::Notify>,
    opens: Arc<std::sync::Mutex<Vec<(rmcp::model::RequestId, Value)>>>,
}

impl ScriptedPeer {
    fn new(scenario: Scenario) -> Self {
        Self {
            scenario,
            calls: Default::default(),
            go: Default::default(),
            sent: Default::default(),
            opens: Default::default(),
        }
    }
}

impl ServerHandler for ScriptedPeer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.capabilities
            .extensions
            .get_or_insert_default()
            .insert(EXTENSION.into(), JsonObject::new());
        info
    }

    async fn on_custom_request(
        &self,
        request: CustomRequest,
        ctx: EventContext,
    ) -> Result<CustomResult, ErrorData> {
        if request.method == "events/list" {
            let delivery = if self.scenario == Scenario::Poll {
                json!(["poll"])
            } else {
                json!(["push", "poll"])
            };
            return Ok(CustomResult(json!({"resultType":"complete","events":[
                {"name":"source","delivery":delivery,"inputSchema":{},"payloadSchema":{}}
            ]})));
        }
        assert_eq!(
            request.method,
            if self.scenario == Scenario::Poll {
                "events/poll"
            } else {
                "events/stream"
            }
        );
        let params = request.params.unwrap();
        let call = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((tokio::time::Instant::now(), params.clone()));
            calls.len()
        };
        let problem = |code, message| ErrorData::new(ErrorCode(code), message, None);
        match (self.scenario, call) {
            (Scenario::PushActivation | Scenario::Poll, 1) => std::future::pending().await,
            (Scenario::PushActivation | Scenario::Poll, 2) => Err(problem(-32603, "transient")),
            (Scenario::PushActivation, 3) => Err(problem(-32024, "denied")),
            (Scenario::Poll, 3..=5) => Ok(CustomResult(json!({
                "resultType":"complete", "events":[], "cursor": if call == 5 { "next" } else { "head" },
                "hasMore":true, "nextPollMs":999_999
            }))),
            (Scenario::Poll, 6) => Err(problem(-32023, "removed")),
            (Scenario::Watchdog | Scenario::Overflow, _) => {
                let cursor = params["cursor"].clone();
                self.opens
                    .lock()
                    .unwrap()
                    .push((ctx.id.clone(), cursor.clone()));
                let start = if call == 1 { json!("head") } else { cursor };
                Self::send(&ctx, "active", json!({"cursor":start, "heartbeatMs":1_000})).await;
                match (self.scenario, call) {
                    (Scenario::Overflow, 1) => {
                        self.go.notified().await;
                        for index in 0..70 {
                            Self::event(&ctx, &format!("event{index}"), &format!("pos{index}"))
                                .await;
                        }
                        self.sent.notify_one();
                    }
                    (Scenario::Overflow, _) => {
                        Self::event(&ctx, "after-reconnect", "reconnected").await
                    }
                    _ => {
                        Self::event(&ctx, "a", "a-end").await;
                        if call > 1 {
                            Self::event(&ctx, "b", "b-end").await;
                        }
                    }
                }
                ctx.ct.cancelled().await;
                Err(problem(-32603, "cancelled"))
            }
            (Scenario::ClosingPush, 1..=2) => {
                Self::send(
                    &ctx,
                    "active",
                    json!({"cursor":"head","heartbeatMs":30_000}),
                )
                .await;
                Ok(CustomResult(
                    json!({"resultType":"complete","_meta":{SUBSCRIPTION_ID:ctx.id}}),
                ))
            }
            (Scenario::ClosingPush, 3) => {
                Self::send(&ctx, "unknown", json!({})).await;
                Self::send(
                    &ctx,
                    "active",
                    json!({"cursor":"fresh","truncated":true,"heartbeatMs":30_000}),
                )
                .await;
                Self::send(
                    &ctx,
                    "error",
                    json!({"error":{"code":-32603,"message":"recoverable"}}),
                )
                .await;
                let mut notification = rmcp::model::CustomNotification::new(
                    "notifications/events/event",
                    Some(json!({
                        "eventId":"live","name":"source","timestamp":"2026-07-28T00:00:00Z","data":{}, "cursor":"latest"
                    })),
                );
                notification
                    .get_meta_mut()
                    .set_subscription_id(ctx.id.clone());
                notification
                    .get_meta_mut()
                    .0
                    .0
                    .insert("app".into(), json!("visible"));
                ctx.peer
                    .send_notification(ServerNotification::CustomNotification(notification))
                    .await
                    .unwrap();
                Self::send(
                    &ctx,
                    "terminated",
                    json!({"error":{"code":-32024,"message":"denied"}}),
                )
                .await;
                ctx.ct.cancelled().await;
                Err(problem(-32603, "cancelled"))
            }
            _ => panic!("unexpected delivery request {call}"),
        }
    }
}

events_test!(
    bare_push_completion_reconnects_with_backoff_and_yields_gap_and_error,
    {
        let fixture = ScriptedPeer::new(Scenario::ClosingPush);
        let client = exchange(fixture.clone(), modern()).await;
        let api = EventsClient::new(client.peer().clone());
        let mut options = SubscriptionOptions::default();
        options.max_age_ms = Some(5_000);
        let mut events = api.subscribe("source", None, options).await.unwrap();
        assert!(
            matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "head")
        );
        for _ in 0..2 {
            assert!(
                matches!(events.next().await, Some(EventUpdate::RecoverableError(EventsError::Service(error)))
            if matches!(*error, ServiceError::TransportClosed))
            );
        }
        assert!(matches!(events.next().await, Some(EventUpdate::Gap)));
        assert!(
            matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "fresh")
        );
        assert!(
            matches!(events.next().await, Some(EventUpdate::RecoverableError(EventsError::Remote(error))) if error.code == ErrorCode::INTERNAL_ERROR)
        );
        let Some(EventUpdate::Occurrence(event)) = events.next().await else {
            panic!("missing occurrence");
        };
        assert_eq!(event.meta, Some(object(json!({"app":"visible"}))));
        assert!(
            matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "latest")
        );
        assert!(matches!(
            events.next().await,
            Some(EventUpdate::Terminated(EventsError::Forbidden { .. }))
        ));
        assert!(events.next().await.is_none());
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!((250..500).contains(&(calls[1].0 - calls[0].0).as_millis()));
        assert!((500..1000).contains(&(calls[2].0 - calls[1].0).as_millis()));
        assert!(
            calls[1..]
                .iter()
                .all(|(_, params)| params["cursor"] == "head")
        );
        assert!(calls.iter().all(|(_, params)| params["maxAgeMs"] == 5_000));
    }
);

events_test!(push_activation_deadline_and_retry_classifier, 20, {
    let client = exchange(ScriptedPeer::new(Scenario::PushActivation), modern()).await;
    let api = EventsClient::new(client.peer().clone());
    let mut events = api
        .subscribe("source", None, Default::default())
        .await
        .unwrap();
    let start = tokio::time::Instant::now();
    assert!(
        matches!(events.next().await, Some(EventUpdate::RecoverableError(EventsError::Service(error)))
        if matches!(*error, ServiceError::Timeout { timeout } if timeout == Duration::from_secs(15)))
    );
    assert_eq!(start.elapsed(), Duration::from_secs(15));
    assert!(
        matches!(events.next().await, Some(EventUpdate::RecoverableError(EventsError::Remote(error)))
        if error.code == ErrorCode::INTERNAL_ERROR)
    );
    assert!(matches!(
        events.next().await,
        Some(EventUpdate::Terminated(EventsError::Forbidden { .. }))
    ));
    assert!(events.next().await.is_none());
});

events_test!(managed_poll_retries_times_out_and_falls_back_to_poll, 22, {
    let fixture = ScriptedPeer::new(Scenario::Poll);
    let client = exchange(fixture.clone(), modern()).await;
    let api = EventsClient::new(client.peer().clone());
    let mut forced = SubscriptionOptions::default();
    forced.mode = Some(DeliveryMode::Push);
    assert!(matches!(api.subscribe("source", None, forced).await,
        Err(EventsError::Unsupported { value: Some(value), .. }) if value == "push"));
    let mut options = SubscriptionOptions::default();
    options.cursor = Some("saved".into());
    options.max_age_ms = Some(5_000);
    let mut events = api.subscribe("source", None, options).await.unwrap();
    let start = tokio::time::Instant::now();
    assert!(
        matches!(events.next().await, Some(EventUpdate::RecoverableError(EventsError::Service(error)))
        if matches!(*error, ServiceError::Timeout { timeout } if timeout == Duration::from_secs(15)))
    );
    assert_eq!(start.elapsed(), Duration::from_secs(15));
    assert!(
        matches!(events.next().await, Some(EventUpdate::RecoverableError(EventsError::Remote(error)))
        if error.code == ErrorCode::INTERNAL_ERROR)
    );
    assert!(
        matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "head")
    );
    assert!(
        matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "next")
    );
    assert!(matches!(
        events.next().await,
        Some(EventUpdate::Terminated(EventsError::NotFound { .. }))
    ));
    assert!(events.next().await.is_none());
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 6);
    assert_eq!(calls[2].0, calls[3].0);
    assert_eq!(calls[4].0 - calls[3].0, Duration::from_secs(1));
    assert_eq!(calls[4].0, calls[5].0);
    assert!(
        calls[..3]
            .iter()
            .all(|(_, params)| params["maxAgeMs"] == 5_000)
    );
    assert!(
        calls[3..]
            .iter()
            .all(|(_, params)| params["maxAgeMs"].is_null())
    );
});

impl ScriptedPeer {
    async fn send(context: &EventContext, method: &str, params: Value) {
        let mut note = rmcp::model::CustomNotification::new(
            format!("notifications/events/{method}"),
            Some(params),
        );
        note.get_meta_mut().set_subscription_id(context.id.clone());
        context
            .peer
            .send_notification(ServerNotification::CustomNotification(note))
            .await
            .unwrap();
    }

    async fn event(context: &EventContext, id: &str, cursor: &str) {
        Self::send(
            context,
            "event",
            json!({
                "eventId": id, "name": "source", "timestamp":"2026-07-28T00:00:00Z",
                "data": {}, "cursor": cursor
            }),
        )
        .await;
    }
}

events_test!(push_watchdog_reconnects_from_last_checkpoint, {
    let script = ScriptedPeer::new(Scenario::Watchdog);
    let client = exchange(script.clone(), modern()).await;
    let client_api = EventsClient::new(client.peer().clone());
    let mut events = client_api
        .subscribe("source", None, SubscriptionOptions::default())
        .await
        .unwrap();
    assert!(
        matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "head")
    );
    assert!(
        matches!(events.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "a")
    );
    assert!(
        matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "a-end")
    );
    let before = tokio::time::Instant::now();
    assert!(
        matches!(events.next().await, Some(EventUpdate::RecoverableError(EventsError::Service(e)))
        if matches!(*e, ServiceError::Timeout { timeout } if timeout == Duration::from_secs(3)))
    );
    assert_eq!(before.elapsed(), Duration::from_secs(3));
    assert!(
        matches!(events.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "a")
    );
    assert!(
        matches!(events.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "b")
    );
    let opens = script.opens.lock().unwrap();
    assert_eq!(opens.len(), 2);
    assert_ne!(opens[0].0, opens[1].0);
    assert!(opens[0].1.is_null());
    assert_eq!(opens[1].1, "a-end");
});

events_test!(
    push_overflow_is_observable_and_resumes_after_only_consumed_frames,
    {
        let script = ScriptedPeer::new(Scenario::Overflow);
        let client = exchange(script.clone(), modern()).await;
        let client_api = EventsClient::new(client.peer().clone());
        let mut events = client_api
            .subscribe("source", None, SubscriptionOptions::default())
            .await
            .unwrap();
        assert!(
            matches!(events.next().await, Some(EventUpdate::Checkpoint(cursor)) if cursor == "head")
        );
        script.go.notify_one();
        script.sent.notified().await;
        // The catalog reply crosses the ordered duplex input after the flood.
        assert_eq!(client_api.list_page(None).await.unwrap().events.len(), 1);
        let mut last = String::from("head");
        loop {
            match events.next().await.unwrap() {
                EventUpdate::Checkpoint(cursor) => last = cursor,
                EventUpdate::Occurrence(_) => {}
                EventUpdate::RecoverableError(EventsError::Service(error)) => {
                    assert!(matches!(
                        *error,
                        ServiceError::SubscriptionLagged { capacity: 64 }
                    ));
                    break;
                }
                other => panic!("expected observable overflow, got {other:?}"),
            }
        }
        assert_ne!(last, "pos69");
        assert!(
            matches!(events.next().await, Some(EventUpdate::Occurrence(event)) if event.event_id == "after-reconnect")
        );
        assert_eq!(script.opens.lock().unwrap()[1].1, last);
    }
);

events_test!(
    emit_byte_eviction_breaking_replacement_and_reregistration,
    {
        let runtime = EventsServer::new();
        let emit = runtime
            .register_emit(raw_descriptor(
                "emit",
                DeliveryMode::Poll,
                JsonObject::new(),
                JsonObject::new(),
            ))
            .unwrap();
        let client = exchange(
            App {
                events: Some(runtime.clone()),
                ..App::default()
            },
            modern(),
        )
        .await;
        let events = EventsClient::new(client.peer().clone());
        let mut poll = PollParams::new("emit");
        poll.cursor = events.poll(poll.clone()).await.unwrap().cursor;
        for index in 1..=3 {
            emit.emit(
                Some(index.to_string()),
                Timestamp::now().to_string(),
                object(json!({"blob":"x".repeat(100 * 1024)})),
            )
            .unwrap();
        }
        let gap = events.poll(poll.clone()).await.unwrap();
        assert!(gap.truncated && gap.has_more && gap.events.is_empty());
        poll.cursor = gap.cursor;
        let batch = events.poll(poll.clone()).await.unwrap();
        assert_eq!(
            batch
                .events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            ["2", "3"]
        );
        poll.cursor = batch.cursor;
        let input = object(json!({"type":"object"}));
        runtime
            .replace(
                raw_descriptor("emit", DeliveryMode::Poll, input.clone(), JsonObject::new()),
                Replacement::Breaking,
            )
            .unwrap();
        assert!(events.poll(poll.clone()).await.unwrap().truncated);
        poll.cursor = events.poll(PollParams::new("emit")).await.unwrap().cursor;
        runtime
            .replace(
                raw_descriptor(
                    "emit",
                    DeliveryMode::Poll,
                    input,
                    object(json!({"type":"object"})),
                ),
                Replacement::Breaking,
            )
            .unwrap();
        assert!(events.poll(poll.clone()).await.unwrap().truncated);
        runtime.remove("emit").unwrap();
        assert!(matches!(
            events.poll(poll.clone()).await,
            Err(EventsError::NotFound { .. })
        ));
        assert!(matches!(
            emit.emit(None, Timestamp::now().to_string(), JsonObject::new()),
            Err(EventsError::NotFound { .. })
        ));
        let new = runtime.register_emit(descriptor("emit")).unwrap();
        assert!(matches!(
            emit.emit(None, Timestamp::now().to_string(), JsonObject::new()),
            Err(EventsError::NotFound { .. })
        ));
        new.emit(
            Some("new".into()),
            Timestamp::now().to_string(),
            JsonObject::new(),
        )
        .unwrap();
        let gap = events.poll(poll.clone()).await.unwrap();
        assert!(gap.truncated && gap.has_more);
        poll.cursor = gap.cursor;
        assert_eq!(events.poll(poll).await.unwrap().events[0].event_id, "new");
        client.cancel().await.unwrap();
    }
);

events_test!(emit_filter_advances_cursor, {
    let runtime = EventsServer::new();
    let selected = descriptor("emit").matches(|_, event, _| event.event_id != "filtered");
    assert!(matches!(
        runtime.register_check(selected.clone(), |_, _, _| ready(Ok(CheckedBatch::new(
            "head"
        )))),
        Err(EventsError::InvalidParams(_))
    ));
    let emit = runtime.register_emit(selected).unwrap();
    let client = exchange(
        App {
            events: Some(runtime),
            ..App::default()
        },
        modern(),
    )
    .await;
    let events = EventsClient::new(client.peer().clone());
    let mut poll = PollParams::new("emit");
    poll.cursor = events.poll(poll.clone()).await.unwrap().cursor;
    emit.emit(
        Some("filtered".into()),
        Timestamp::now().to_string(),
        JsonObject::new(),
    )
    .unwrap();
    let quiet = events.poll(poll.clone()).await.unwrap();
    assert!(quiet.events.is_empty());
    assert_ne!(quiet.cursor, poll.cursor);
    poll.cursor = quiet.cursor;
    emit.emit(
        Some("kept".into()),
        Timestamp::now().to_string(),
        JsonObject::new(),
    )
    .unwrap();
    let result = events.poll(poll).await.unwrap();
    assert_eq!(result.events[0].event_id, "kept");
    client.cancel().await.unwrap();
});

events_test!(oversized_emit_does_not_advance_replay, {
    let runtime = EventsServer::new();
    let emit = runtime.register_emit(descriptor("emit")).unwrap();
    let client = exchange(
        App {
            events: Some(runtime),
            ..App::default()
        },
        modern(),
    )
    .await;
    let events = EventsClient::new(client.peer().clone());
    let mut poll = PollParams::new("emit");
    poll.cursor = events.poll(poll.clone()).await.unwrap().cursor;
    emit.emit(
        Some("kept".into()),
        Timestamp::now().to_string(),
        JsonObject::new(),
    )
    .unwrap();
    assert!(matches!(
        emit.emit(None, Timestamp::now().to_string(), object(json!({"blob":"x".repeat(256 * 1024 + 1)}))),
        Err(EventsError::ResourceExhausted { limit, max: Some(262_144) }) if limit == "eventBytes"
    ));
    let result = events.poll(poll.clone()).await.unwrap();
    assert_eq!(result.events[0].event_id, "kept");
    assert_ne!(result.cursor, poll.cursor);
    client.cancel().await.unwrap();
});

events_test!(
    emit_age_gap_never_skips_earlier_items_and_count_limit_evicts,
    {
        let runtime = EventsServer::new();
        let emit = runtime.register_emit(descriptor("emit")).unwrap();
        let client = exchange(
            App {
                events: Some(runtime),
                ..App::default()
            },
            modern(),
        )
        .await;
        let events = EventsClient::new(client.peer().clone());
        let mut poll = PollParams::new("emit");
        poll.cursor = events.poll(poll.clone()).await.unwrap().cursor;
        let initial = poll.cursor.clone();
        let now = Timestamp::now();
        for (id, time) in [
            ("first", now),
            ("old", now - Duration::from_secs(7_200)),
            ("last", now),
        ] {
            emit.emit(Some(id.into()), time.to_string(), JsonObject::new())
                .unwrap();
        }
        poll.max_age_ms = Some(3_600_000);
        poll.max_events = Some(1);
        let first = events.poll(poll.clone()).await.unwrap();
        assert_eq!(first.events[0].event_id, "first");
        assert!(first.has_more && !first.truncated);
        poll.cursor = first.cursor;
        let gap = events.poll(poll.clone()).await.unwrap();
        assert!(gap.events.is_empty() && gap.truncated && gap.has_more);
        poll.cursor = gap.cursor;
        assert_eq!(
            events.poll(poll.clone()).await.unwrap().events[0].event_id,
            "last"
        );
        for i in 0..260 {
            emit.emit(
                Some(format!("ring-{i}")),
                now.to_string(),
                JsonObject::new(),
            )
            .unwrap();
        }
        poll.cursor = initial;
        let gap = events.poll(poll.clone()).await.unwrap();
        assert!(gap.truncated && gap.has_more);
        poll.cursor = gap.cursor;
        assert_eq!(
            events.poll(poll).await.unwrap().events[0].event_id,
            "ring-4"
        );
        client.cancel().await.unwrap();
    }
);

events_test!(runtime_off_preserves_app_advertisement_and_handlers, {
    let client = exchange(
        App {
            custom: true,
            app_settings: Some(object(json!({"listChanged":true,"app":"owned"}))),
            ..App::default()
        },
        modern(),
    )
    .await;
    let info = client.peer_info().unwrap();
    assert_eq!(
        info.capabilities.extensions.as_ref().unwrap()[EXTENSION]["app"],
        "owned"
    );
    assert!(matches!(call(&client, "events/list", None).await.unwrap(),
        ServerResult::CustomResult(result) if result.0["fromApp"] == "events/list"));
    let mut stream = client
        .listen(SubscriptionFilter::builder().events_list_changed().build())
        .await
        .unwrap();
    assert_eq!(stream.acknowledged().events_list_changed, Some(true));
    stream.cancel().await.unwrap();
    client.cancel().await.unwrap();
    let client = exchange(
        App {
            app_settings: Some(JsonObject::new()),
            ..App::default()
        },
        modern(),
    )
    .await;
    let mut stream = client
        .listen(SubscriptionFilter::builder().events_list_changed().build())
        .await
        .unwrap();
    assert_eq!(stream.acknowledged().events_list_changed, None);
    stream.cancel().await.unwrap();
    client.cancel().await.unwrap();
});

events_test!(
    installed_runtime_only_uses_modern_discovery_and_reserved_methods,
    {
        let runtime = EventsServer::new();
        register(&runtime, "a");
        for mode in [
            ClientLifecycleMode::Initialize,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2025_11_25],
            },
        ] {
            let client = exchange(
                App {
                    events: Some(runtime.clone()),
                    custom: true,
                    app_settings: Some(JsonObject::new()),
                    ..App::default()
                },
                mode,
            )
            .await;
            let info = client.peer_info().unwrap();
            assert!(
                !info
                    .capabilities
                    .extensions
                    .as_ref()
                    .unwrap()
                    .contains_key(EXTENSION)
            );
            assert!(
                matches!(call(&client, "events/list", None).await, Err(ServiceError::McpError(error)) if error.code == ErrorCode::METHOD_NOT_FOUND)
            );
            client.cancel().await.unwrap();
        }
        let client = exchange(
            App {
                events: Some(runtime.clone()),
                custom: true,
                ..App::default()
            },
            modern(),
        )
        .await;
        for (method, code) in [
            ("events/foo", -32601),
            ("events/subscribe", -32026),
            ("events/unsubscribe", -32026),
        ] {
            assert!(
                matches!(call(&client, method, Some(json!({"name":"a"}))).await, Err(ServiceError::McpError(error)) if error.code.0 == code)
            );
        }
        assert!(
            matches!(call(&client, "fixture/method", None).await.unwrap(), ServerResult::CustomResult(result) if result.0["fromApp"] == "fixture/method")
        );
        runtime.remove("a").unwrap();
        assert!(
            matches!(call(&client, "events/poll", Some(json!({"name":"a"}))).await,
        Err(ServiceError::McpError(error)) if error.code.0 == -32023)
        );
        client.cancel().await.unwrap();
    }
);

#[derive(Clone, Default)]
struct EndlessCatalog(Arc<AtomicUsize>);

impl ServerHandler for EndlessCatalog {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        let extensions = info.capabilities.extensions.get_or_insert_default();
        extensions.insert(EXTENSION.into(), JsonObject::new());
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    async fn on_custom_request(
        &self,
        _: CustomRequest,
        _: EventContext,
    ) -> Result<CustomResult, ErrorData> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(CustomResult(json!({"events":[],"nextCursor":""})))
    }
}

events_test!(catalog_walk_stops_at_the_page_cap, {
    let server = EndlessCatalog::default();
    let client = exchange(server.clone(), modern()).await;
    let events = EventsClient::new(client.peer().clone());
    assert!(matches!(
        events.poll(PollParams::new("source")).await,
        Err(EventsError::Service(error))
            if matches!(*error, ServiceError::ListPaginationExceeded { max_pages: 64 })
    ));
    assert_eq!(server.0.load(Ordering::SeqCst), 64);
    client.cancel().await.unwrap();
});

events_test!(catalog_change_before_watcher, {
    let runtime = EventsServer::new();
    for index in (0..40).rev() {
        register(&runtime, &format!("{index:02}"));
    }
    let client = exchange(
        App {
            events: Some(runtime),
            change_during_listen: true,
            ..App::default()
        },
        modern(),
    )
    .await;
    let events = EventsClient::new(client.peer().clone());
    let page = events.list_page(None).await.unwrap();
    assert!(page.next_cursor.is_none());
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.name.clone())
            .collect::<Vec<_>>(),
        (0..40)
            .map(|index| format!("{index:02}"))
            .collect::<Vec<_>>()
    );
    assert!(matches!(
        events.list_page(Some("garbage")).await,
        Err(EventsError::InvalidParams(_))
    ));
    let mut stream = client
        .listen(
            SubscriptionFilter::builder()
                .events_list_changed()
                .tools_list_changed()
                .build(),
        )
        .await
        .unwrap();
    assert_eq!(stream.acknowledged().events_list_changed, Some(true));
    assert_eq!(stream.acknowledged().tools_list_changed, None);
    let notice = stream.next().await.unwrap().unwrap();
    assert!(
        matches!(notice, ServerNotification::CustomNotification(ref n) if n.method == "notifications/events/list_changed")
    );
    assert_eq!(
        notice.get_meta().subscription_id().as_ref(),
        Some(stream.id())
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(1), stream.next())
            .await
            .is_err(),
        "revisions coalesce"
    );
    stream.cancel().await.unwrap();
    client.cancel().await.unwrap();
});

events_test!(
    additive_replace_during_authorization_and_new_catalog_name,
    {
        let runtime = EventsServer::new();
        let registry = runtime.clone();
        runtime
            .register_check(
                descriptor("a").authorize(move |_, _, mode| {
                    assert_eq!(mode, DeliveryMode::Poll);
                    ready(registry.replace(descriptor("a"), Replacement::Additive))
                }),
                |_, _, _| ready(Ok(CheckedBatch::new("head"))),
            )
            .unwrap();
        let client = exchange(
            App {
                events: Some(runtime.clone()),
                ..App::default()
            },
            modern(),
        )
        .await;
        let events = EventsClient::new(client.peer().clone());
        assert_eq!(
            events
                .poll(PollParams::new("a"))
                .await
                .unwrap()
                .cursor
                .as_deref(),
            Some("head")
        );
        register(&runtime, "b");
        assert_eq!(
            events
                .poll(PollParams::new("b"))
                .await
                .unwrap()
                .cursor
                .as_deref(),
            Some("head")
        );
        client.cancel().await.unwrap();
    }
);

events_test!(check_byte_cut_and_age_floor_including_large_unsigned_age, {
    let runtime = EventsServer::new();
    let now = Timestamp::now();
    runtime
        .register_check(
            descriptor("a"),
            move |ctx, args, query: CheckQuery| async move {
                assert!(!ctx.ct.is_cancelled());
                assert_eq!(query.max_bytes, 256 * 1024);
                if query.cursor.is_none() {
                    assert!(query.age_floor.is_none());
                    return Ok(CheckedBatch::new("head"));
                }
                assert_eq!(query.age_floor.is_some(), args.contains_key("floor"));
                let mut batch = CheckedBatch::new("3");
                let first = if query.cursor.as_deref() == Some("2") {
                    3
                } else {
                    1
                };
                for index in first..=3 {
                    let mut event = EventOccurrence::new(
                        index.to_string(),
                        "a",
                        now.to_string(),
                        object(json!({"data":"x".repeat(100 * 1024)})),
                    );
                    event.cursor = Some(index.to_string());
                    batch.events.push(event);
                }
                Ok(batch)
            },
        )
        .unwrap();
    let client = exchange(
        App {
            events: Some(runtime),
            ..App::default()
        },
        modern(),
    )
    .await;
    let events = EventsClient::new(client.peer().clone());
    let mut poll = PollParams::new("a");
    poll.max_age_ms = Some(3_600_000);
    poll.cursor = events.poll(poll.clone()).await.unwrap().cursor;
    poll.arguments = Some(object(json!({"floor": true})));
    let cut = events.poll(poll.clone()).await.unwrap();
    assert_eq!(cut.events.len(), 2);
    assert!(cut.events.iter().all(|event| event.cursor.is_none()));
    assert_eq!(cut.cursor.as_deref(), Some("2"));
    assert!(cut.has_more);
    poll.cursor = cut.cursor;
    poll.max_age_ms = Some(u64::MAX);
    poll.arguments = None;
    let last = events.poll(poll).await.unwrap();
    assert_eq!(last.events[0].event_id, "3");
    assert!(!last.has_more);
    client.cancel().await.unwrap();
});

events_test!(cancellation_drops_an_in_flight_check, {
    struct OnDrop {
        token: tokio_util::sync::CancellationToken,
        observed: Arc<tokio::sync::Notify>,
    }
    impl Drop for OnDrop {
        fn drop(&mut self) {
            assert!(self.token.is_cancelled());
            self.observed.notify_one();
        }
    }
    let runtime = EventsServer::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let stopped = Arc::new(tokio::sync::Notify::new());
    runtime
        .register_check(descriptor("a"), {
            let (entered, stopped) = (entered.clone(), stopped.clone());
            move |ctx, _, _| {
                let (entered, stopped) = (entered.clone(), stopped.clone());
                async move {
                    let _on_drop = OnDrop {
                        token: ctx.ct,
                        observed: stopped,
                    };
                    entered.notify_one();
                    pending::<Result<CheckedBatch, EventsError>>().await
                }
            }
        })
        .unwrap();
    let client = exchange(
        App {
            events: Some(runtime),
            ..App::default()
        },
        modern(),
    )
    .await;
    let request = CustomRequest::new("events/poll", Some(json!({"name":"a"})));
    let handle = client
        .send_request_with_option(
            ClientRequest::CustomRequest(request),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    entered.notified().await;
    handle
        .cancel(Some("test cancellation".into()))
        .await
        .unwrap();
    stopped.notified().await;
    client.cancel().await.unwrap();
});
