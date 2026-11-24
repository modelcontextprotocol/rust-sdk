#![cfg(all(feature = "events", feature = "server", feature = "client"))]

use std::{
    borrow::Cow,
    future::{pending, ready},
    sync::Arc,
    time::Duration,
};

use chrono::Utc;
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ErrorData, RoleClient, ServerHandler, ServiceError,
    ServiceExt,
    events::{
        client::EventsClient,
        server::{CheckQuery, CheckedBatch, Descriptor, EventContext, EventsServer, Replacement},
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

fn raw_descriptor(name: &str, input: JsonObject, payload: JsonObject) -> Descriptor {
    use rmcp::events::wire::EventDescriptor;
    Descriptor::raw(
        EventDescriptor::new(
            name,
            "",
            vec![DeliveryMode::Poll],
            input.into(),
            payload.into(),
        ),
        |_| Ok(()),
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
            assert!(
                !(self.app_settings.as_ref().is_some_and(JsonObject::is_empty)
                    && requested.events_list_changed == Some(true)),
                "client must strip the category for an empty advertisement"
            );
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

async fn exchange(app: App, mode: ClientLifecycleMode) -> RunningService<RoleClient, ClientConfig> {
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
        #[tokio::test(start_paused = true)]
        async fn $name() {
            tokio::task::LocalSet::new()
                .run_until(tokio::time::timeout(Duration::from_secs(5), async $body))
                .await.expect("duplex exchange made no progress");
        }
    }
}

events_test!(emit_additive_replace_during_validation, {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use rmcp::events::wire::EventDescriptor;
    let runtime = EventsServer::new();
    let registry = runtime.clone();
    let revalidated = Arc::new(AtomicUsize::new(0));
    let probe = revalidated.clone();
    let wire = EventDescriptor::new(
        "emit",
        "",
        vec![DeliveryMode::Poll],
        Default::default(),
        Default::default(),
    );
    let previous = wire.clone();
    let emit = runtime
        .register_emit(Descriptor::raw(
            wire,
            |_| Ok(()),
            move |_| {
                let probe = probe.clone();
                registry.replace(
                    Descriptor::raw(
                        previous.clone(),
                        |_| Ok(()),
                        move |_| {
                            probe.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        },
                    ),
                    Replacement::Additive,
                )
            },
        ))
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
    let mut poll = PollParams::new("emit");
    poll.cursor = events.poll(poll.clone()).await.unwrap().cursor;
    emit.emit(Some("kept".into()), Utc::now(), JsonObject::new())
        .unwrap();
    assert_eq!(revalidated.load(Ordering::SeqCst), 1);
    assert_eq!(events.poll(poll).await.unwrap().events[0].event_id, "kept");
    client.cancel().await.unwrap();
});

events_test!(
    emit_byte_eviction_breaking_replacement_and_reregistration,
    {
        let runtime = EventsServer::new();
        let emit = runtime
            .register_emit(raw_descriptor("emit", JsonObject::new(), JsonObject::new()))
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
                Utc::now(),
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
                raw_descriptor("emit", input.clone(), JsonObject::new()),
                Replacement::Breaking,
            )
            .unwrap();
        assert!(
            matches!(events.poll(poll.clone()).await, Err(EventsError::Unsupported { feature, reason, .. })
        if feature == "inputSchema" && reason.as_deref() == Some("schema_changed"))
        );
        poll.cursor = events.poll(PollParams::new("emit")).await.unwrap().cursor;
        runtime
            .replace(
                raw_descriptor("emit", input, object(json!({"type":"object"}))),
                Replacement::Breaking,
            )
            .unwrap();
        assert!(
            matches!(events.poll(poll.clone()).await, Err(EventsError::Unsupported { feature, reason, .. })
        if feature == "payloadSchema" && reason.as_deref() == Some("schema_changed"))
        );
        runtime.remove("emit").unwrap();
        assert!(matches!(
            events.poll(poll.clone()).await,
            Err(EventsError::NotFound { .. })
        ));
        assert!(matches!(
            emit.emit(None, Utc::now(), JsonObject::new()),
            Err(EventsError::NotFound { .. })
        ));
        let new = runtime.register_emit(descriptor("emit")).unwrap();
        assert!(matches!(
            emit.emit(None, Utc::now(), JsonObject::new()),
            Err(EventsError::NotFound { .. })
        ));
        new.emit(Some("new".into()), Utc::now(), JsonObject::new())
            .unwrap();
        let gap = events.poll(poll.clone()).await.unwrap();
        assert!(gap.truncated && gap.has_more);
        poll.cursor = gap.cursor;
        assert_eq!(events.poll(poll).await.unwrap().events[0].event_id, "new");
        client.cancel().await.unwrap();
    }
);

events_test!(emit_transform_byte_cut_and_failure_preserve_cursor, {
    let runtime = EventsServer::new();
    let selected = descriptor("emit")
        .matches(|_, event, _| event.event_id != "filtered")
        .transform(|_, event, args| {
            if event.event_id == "2" && args.contains_key("fail") {
                return Err(EventsError::InvalidParams("transform fixture".into()));
            }
            Ok(object(json!({"blob":"x".repeat(100 * 1024)})))
        });
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
    emit.emit(Some("filtered".into()), Utc::now(), JsonObject::new())
        .unwrap();
    let quiet = events.poll(poll.clone()).await.unwrap();
    assert!(quiet.events.is_empty());
    assert_ne!(quiet.cursor, poll.cursor);
    poll.cursor = quiet.cursor;
    for index in 1..=3 {
        emit.emit(Some(index.to_string()), Utc::now(), JsonObject::new())
            .unwrap();
    }
    poll.arguments = Some(object(json!({"fail":true})));
    assert!(
        matches!(events.poll(poll.clone()).await, Err(EventsError::InvalidParams(reason)) if reason == "transform fixture")
    );
    poll.arguments = None;
    let first = events.poll(poll.clone()).await.unwrap();
    assert_eq!(
        first
            .events
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
    assert!(first.has_more);
    poll.cursor = first.cursor;
    let last = events.poll(poll).await.unwrap();
    assert_eq!(last.events[0].event_id, "3");
    assert!(!last.has_more);
    client.cancel().await.unwrap();
});

events_test!(oversized_transformed_occurrence_does_not_advance_replay, {
    let runtime = EventsServer::new();
    let emit = runtime
        .register_emit(
            descriptor("emit").transform(|_: &EventContext, event, args| {
                if args.contains_key("enlarge") {
                    Ok(object(json!({"blob":"x".repeat(256 * 1024 + 1)})))
                } else {
                    Ok(event.data.clone())
                }
            }),
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
    let mut poll = PollParams::new("emit");
    poll.cursor = events.poll(poll.clone()).await.unwrap().cursor;
    emit.emit(Some("kept".into()), Utc::now(), JsonObject::new())
        .unwrap();
    poll.arguments = Some(object(json!({"enlarge":true})));
    assert!(matches!(
        events.poll(poll.clone()).await,
        Err(EventsError::ResourceExhausted { limit, max: Some(262_144) }) if limit == "eventBytes"
    ));
    poll.arguments = None;
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
        let now = Utc::now();
        for (id, time) in [
            ("first", now),
            ("old", now - chrono::TimeDelta::hours(2)),
            ("last", now),
        ] {
            emit.emit(Some(id.into()), time, JsonObject::new()).unwrap();
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
            emit.emit(Some(format!("ring-{i}")), now, JsonObject::new())
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

events_test!(catalog_pagination_and_changes_before_the_watcher, {
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
    let next = page.next_cursor.unwrap();
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.name.clone())
            .collect::<Vec<_>>(),
        (0..32)
            .map(|index| format!("{index:02}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(events.list_page(Some(&next)).await.unwrap().events.len(), 8);
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
    assert!(matches!(
        events.list_page(Some(&next)).await,
        Err(EventsError::InvalidParams(_))
    ));
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
    let now = Utc::now();
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
                        now,
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

events_test!(breaking_replace_rejects_an_in_flight_check_result, {
    let runtime = EventsServer::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    runtime
        .register_check(descriptor("a"), {
            let (entered, release) = (entered.clone(), release.clone());
            move |_, _, _| {
                let (entered, release) = (entered.clone(), release.clone());
                async move {
                    entered.notify_one();
                    release.notified().await;
                    Ok(CheckedBatch::new("obsolete"))
                }
            }
        })
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
    let (result, ()) = tokio::join!(events.poll(PollParams::new("a")), async {
        entered.notified().await;
        let new = EventDescriptor::new(
            "a",
            "",
            vec![DeliveryMode::Poll],
            object(json!({"type":"object","additionalProperties":false})).into(),
            Default::default(),
        );
        runtime
            .replace(
                Descriptor::raw(new, |_| Ok(()), |_| Ok(())),
                Replacement::Breaking,
            )
            .unwrap();
        release.notify_one();
    });
    assert!(
        matches!(result, Err(EventsError::Unsupported { feature, reason, .. })
        if feature == "inputSchema" && reason.as_deref() == Some("schema_changed"))
    );
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
