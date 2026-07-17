use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, GetMeta, JsonRpcNotification, JsonRpcRequest,
    NotificationMetaObject, RequestId, RequestMetaObject, ServerJsonRpcMessage, ServerNotification,
    SubscriptionFilter, SubscriptionsAcknowledgedNotification,
    SubscriptionsAcknowledgedNotificationParams, SubscriptionsListenRequest,
    SubscriptionsListenRequestParams, SubscriptionsListenResult, SubscriptionsListenResultMeta,
};
use serde_json::json;

#[test]
fn subscription_filter_serializes_only_opted_in_notifications() {
    let filter = SubscriptionFilter::builder()
        .tools_list_changed()
        .resource_subscription("file:///one")
        .resource_subscription("file:///two")
        .build();

    assert_eq!(
        serde_json::to_value(filter).expect("serialize filter"),
        json!({
            "toolsListChanged": true,
            "resourceSubscriptions": ["file:///one", "file:///two"],
        })
    );
}

#[test]
fn subscription_filter_subset_is_order_independent_and_ignores_false_flags() {
    let requested = SubscriptionFilter::builder()
        .tools_list_changed()
        .resource_subscriptions(["file:///one", "file:///two"])
        .build();
    let mut accepted = SubscriptionFilter::builder()
        .resource_subscriptions(["file:///two", "file:///one"])
        .build();
    accepted.tools_list_changed = Some(false);

    assert!(accepted.is_subset_of(&requested));
}

#[test]
fn subscription_filter_omits_empty_resource_intersection() {
    let requested = SubscriptionFilter::builder()
        .resource_subscription("file:///requested")
        .build();
    let accepted = SubscriptionFilter::builder()
        .resource_subscription("file:///different")
        .build();

    assert_eq!(
        serde_json::to_value(requested.intersection(&accepted)).expect("serialize intersection"),
        json!({})
    );
}

#[test]
fn listen_request_round_trips_required_fields_and_arbitrary_metadata() {
    let mut request = SubscriptionsListenRequest::new(SubscriptionsListenRequestParams::new(
        SubscriptionFilter::builder().prompts_list_changed().build(),
    ));
    let mut meta = RequestMetaObject::new();
    meta.insert("com.example/request".into(), json!("value"));
    request.extensions.insert(meta);
    let message = ClientJsonRpcMessage::request(
        ClientRequest::SubscriptionsListenRequest(request),
        RequestId::String("subscription-1".into()),
    );

    let value = serde_json::to_value(&message).expect("serialize listen request");
    assert_eq!(
        value,
        json!({
            "jsonrpc": "2.0",
            "id": "subscription-1",
            "method": "subscriptions/listen",
            "params": {
                "_meta": {
                    "com.example/request": "value",
                },
                "notifications": {
                    "promptsListChanged": true,
                },
            },
        })
    );

    let round_trip: ClientJsonRpcMessage =
        serde_json::from_value(value).expect("deserialize listen request");
    let ClientJsonRpcMessage::Request(JsonRpcRequest { request, .. }) = round_trip else {
        panic!("expected request");
    };
    let ClientRequest::SubscriptionsListenRequest(request) = request else {
        panic!("expected subscriptions/listen request");
    };
    assert_eq!(
        request
            .extensions
            .get::<RequestMetaObject>()
            .and_then(|meta| meta.get("com.example/request")),
        Some(&json!("value"))
    );
}

#[test]
fn acknowledged_notification_round_trips_numeric_subscription_id_and_metadata() {
    let mut notification = SubscriptionsAcknowledgedNotification::new(
        SubscriptionsAcknowledgedNotificationParams::new(
            SubscriptionFilter::builder()
                .resources_list_changed()
                .build(),
        ),
    );
    let mut meta = NotificationMetaObject::new();
    meta.set_subscription_id(RequestId::Number(7));
    meta.insert("com.example/notification".into(), json!(true));
    notification.extensions.insert(meta);
    let message = ServerJsonRpcMessage::notification(
        ServerNotification::SubscriptionsAcknowledgedNotification(notification),
    );

    let value = serde_json::to_value(&message).expect("serialize acknowledgment");
    assert_eq!(
        value,
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/subscriptions/acknowledged",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/subscriptionId": 7,
                    "com.example/notification": true,
                },
                "notifications": {
                    "resourcesListChanged": true,
                },
            },
        })
    );

    let round_trip: ServerJsonRpcMessage =
        serde_json::from_value(value).expect("deserialize acknowledgment");
    let ServerJsonRpcMessage::Notification(JsonRpcNotification { notification, .. }) = round_trip
    else {
        panic!("expected notification");
    };
    assert_eq!(
        notification.get_meta().subscription_id(),
        Some(RequestId::Number(7))
    );
    assert_eq!(
        notification.get_meta().get("com.example/notification"),
        Some(&json!(true))
    );
}

#[test]
fn listen_result_requires_matching_string_subscription_id_and_preserves_metadata() {
    let mut meta = SubscriptionsListenResultMeta::new(RequestId::String("subscription-2".into()));
    meta.insert("com.example/result".into(), json!({ "reason": "shutdown" }));
    let result = SubscriptionsListenResult::new(meta);

    let value = serde_json::to_value(&result).expect("serialize listen result");
    assert_eq!(
        value,
        json!({
            "resultType": "complete",
            "_meta": {
                "io.modelcontextprotocol/subscriptionId": "subscription-2",
                "com.example/result": {
                    "reason": "shutdown",
                },
            },
        })
    );

    let round_trip: SubscriptionsListenResult =
        serde_json::from_value(value).expect("deserialize listen result");
    assert_eq!(
        round_trip.meta.subscription_id(),
        Some(RequestId::String("subscription-2".into()))
    );
    assert_eq!(
        round_trip.meta.get("com.example/result"),
        Some(&json!({ "reason": "shutdown" }))
    );
}

#[test]
fn listen_result_meta_returns_none_after_required_id_is_removed() {
    let mut meta = SubscriptionsListenResultMeta::new(RequestId::Number(1));
    meta.remove("io.modelcontextprotocol/subscriptionId");

    assert_eq!(meta.subscription_id(), None);
}

#[cfg(feature = "schemars")]
#[test]
fn subscription_schemas_mark_only_draft_required_fields_as_required() {
    let request_schema =
        serde_json::to_value(schemars::schema_for!(SubscriptionsListenRequestParams))
            .expect("request schema");
    let filter_schema =
        serde_json::to_value(schemars::schema_for!(SubscriptionFilter)).expect("filter schema");
    let acknowledgment_schema = serde_json::to_value(schemars::schema_for!(
        SubscriptionsAcknowledgedNotificationParams
    ))
    .expect("acknowledgment schema");
    let result_schema = serde_json::to_value(schemars::schema_for!(SubscriptionsListenResult))
        .expect("result schema");

    assert_eq!(
        request_schema["required"],
        json!(["_meta", "notifications"])
    );
    assert_eq!(
        request_schema["properties"]["_meta"]["required"],
        json!([
            "io.modelcontextprotocol/protocolVersion",
            "io.modelcontextprotocol/clientInfo",
            "io.modelcontextprotocol/clientCapabilities"
        ])
    );
    assert!(filter_schema.get("required").is_none());
    assert_eq!(
        filter_schema["properties"]["toolsListChanged"]["type"],
        "boolean"
    );
    assert_eq!(
        filter_schema["properties"]["resourceSubscriptions"]["type"],
        "array"
    );
    assert_eq!(acknowledgment_schema["required"], json!(["notifications"]));
    assert_eq!(
        acknowledgment_schema["properties"]["_meta"]["$ref"],
        "#/$defs/NotificationMetaObject"
    );
    assert_eq!(result_schema["required"], json!(["resultType", "_meta"]));
}
