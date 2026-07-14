use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{
    CustomNotification, CustomRequest, Extensions, JsonObject, MetaObject, Notification,
    NotificationMetaObject, NotificationNoParam, Request, RequestMetaObject, RequestNoParam,
    RequestOptionalParam,
};

/// Wire-side view of `params`: the `_meta` map plus the remaining fields.
///
/// All metadata types are transparent wrappers over [`JsonObject`], so the
/// serde plumbing works on the raw map; call sites wrap/unwrap the typed
/// metadata ([`RequestMetaObject`] / [`NotificationMetaObject`]).
#[derive(Deserialize)]
struct WithMeta<'a, P> {
    _meta: Option<Cow<'a, JsonObject>>,
    #[serde(flatten)]
    _rest: P,
}

impl<P: Serialize> Serialize for WithMeta<'_, P> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        // Serialize _rest to a Value so we can inspect and strip any duplicate _meta
        let mut rest_value =
            serde_json::to_value(&self._rest).map_err(serde::ser::Error::custom)?;

        // Extract _meta from the serialized params (if it's an object containing one)
        let params_meta: Option<JsonObject> = rest_value
            .as_object_mut()
            .and_then(|obj| obj.remove("_meta"))
            .and_then(|v| serde_json::from_value(v).ok());

        // Merge: params-level _meta as base, extensions-level _meta overwrites on conflict
        let merged_meta = match (self._meta.as_deref(), params_meta) {
            (Some(ext_meta), Some(mut params_meta)) => {
                params_meta.extend(ext_meta.clone());
                Some(params_meta)
            }
            (Some(ext_meta), None) => Some(ext_meta.clone()),
            (None, Some(params_meta)) => Some(params_meta),
            (None, None) => None,
        };

        // Serialize as a flat map: single _meta + remaining params fields
        let rest_obj = match rest_value {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        let meta_count = usize::from(merged_meta.is_some());
        let mut map = serializer.serialize_map(Some(rest_obj.len() + meta_count))?;

        if let Some(meta) = &merged_meta {
            map.serialize_entry("_meta", meta)?;
        }

        for (k, v) in &rest_obj {
            map.serialize_entry(k, v)?;
        }

        map.end()
    }
}

#[derive(Serialize, Deserialize)]
struct Proxy<'a, M, P> {
    method: M,
    params: WithMeta<'a, P>,
}

#[derive(Serialize, Deserialize)]
struct ProxyOptionalParam<'a, M, P> {
    method: M,
    params: Option<WithMeta<'a, P>>,
}

#[derive(Serialize, Deserialize)]
struct ProxyNoParam<M> {
    method: M,
}

/// Combine the message-specific `_meta` map with a legacy [`MetaObject`]
/// extension (inserted through the deprecated `Meta` name), so pre-3.x code
/// does not silently lose metadata on the wire. On key conflicts the
/// message-specific map wins.
fn merge_legacy_meta<'a>(
    typed: Option<&'a JsonObject>,
    extensions: &'a Extensions,
) -> Option<Cow<'a, JsonObject>> {
    let legacy = extensions.get::<MetaObject>().map(|meta| &meta.0);
    match (typed, legacy) {
        (Some(typed), None) => Some(Cow::Borrowed(typed)),
        (None, Some(legacy)) => Some(Cow::Borrowed(legacy)),
        (Some(typed), Some(legacy)) => {
            let mut merged = legacy.clone();
            merged.extend(typed.clone());
            Some(Cow::Owned(merged))
        }
        (None, None) => None,
    }
}

/// Borrow the request `_meta` map from extensions, if any.
fn request_meta(extensions: &Extensions) -> Option<Cow<'_, JsonObject>> {
    let typed = extensions.get::<RequestMetaObject>().map(|meta| &meta.0.0);
    merge_legacy_meta(typed, extensions)
}

/// Borrow the notification `_meta` map from extensions, if any.
fn notification_meta(extensions: &Extensions) -> Option<Cow<'_, JsonObject>> {
    let typed = extensions
        .get::<NotificationMetaObject>()
        .map(|meta| &meta.0.0);
    merge_legacy_meta(typed, extensions)
}

/// Build extensions holding a typed metadata map deserialized from `params._meta`.
fn extensions_with_meta<T>(meta: Option<Cow<'_, JsonObject>>) -> Extensions
where
    T: From<JsonObject> + Clone + Send + Sync + 'static,
{
    let mut extensions = Extensions::new();
    if let Some(meta) = meta {
        extensions.insert(T::from(meta.into_owned()));
    }
    extensions
}

impl<M, R> Serialize for Request<M, R>
where
    M: Serialize,
    R: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Proxy::serialize(
            &Proxy {
                method: &self.method,
                params: WithMeta {
                    _rest: &self.params,
                    _meta: request_meta(&self.extensions),
                },
            },
            serializer,
        )
    }
}

impl<'de, M, R> Deserialize<'de> for Request<M, R>
where
    M: Deserialize<'de>,
    R: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let body = Proxy::deserialize(deserializer)?;
        Ok(Request {
            extensions: extensions_with_meta::<RequestMetaObject>(body.params._meta),
            method: body.method,
            params: body.params._rest,
        })
    }
}

impl<M, R> Serialize for RequestOptionalParam<M, R>
where
    M: Serialize,
    R: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Proxy::serialize(
            &Proxy {
                method: &self.method,
                params: WithMeta {
                    _rest: &self.params,
                    _meta: request_meta(&self.extensions),
                },
            },
            serializer,
        )
    }
}

impl<'de, M, R> Deserialize<'de> for RequestOptionalParam<M, R>
where
    M: Deserialize<'de>,
    R: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let body = ProxyOptionalParam::<'_, _, Option<R>>::deserialize(deserializer)?;
        let mut params = None;
        let mut _meta = None;
        if let Some(body_params) = body.params {
            params = body_params._rest;
            _meta = body_params._meta;
        }
        Ok(RequestOptionalParam {
            extensions: extensions_with_meta::<RequestMetaObject>(_meta),
            method: body.method,
            params,
        })
    }
}

impl<M> Serialize for RequestNoParam<M>
where
    M: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Emit `params` only when metadata is present, so the wire shape of
        // meta-less requests stays `{"method": ...}`.
        match request_meta(&self.extensions) {
            Some(_meta) => Proxy::serialize(
                &Proxy {
                    method: &self.method,
                    params: WithMeta {
                        _meta: Some(_meta),
                        _rest: (),
                    },
                },
                serializer,
            ),
            None => ProxyNoParam::serialize(
                &ProxyNoParam {
                    method: &self.method,
                },
                serializer,
            ),
        }
    }
}

impl<'de, M> Deserialize<'de> for RequestNoParam<M>
where
    M: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let body = ProxyOptionalParam::<'_, _, Option<JsonObject>>::deserialize(deserializer)?;
        let _meta = body.params.and_then(|params| params._meta);
        Ok(RequestNoParam {
            extensions: extensions_with_meta::<RequestMetaObject>(_meta),
            method: body.method,
        })
    }
}

impl<M, R> Serialize for Notification<M, R>
where
    M: Serialize,
    R: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Proxy::serialize(
            &Proxy {
                method: &self.method,
                params: WithMeta {
                    _rest: &self.params,
                    _meta: notification_meta(&self.extensions),
                },
            },
            serializer,
        )
    }
}

impl<'de, M, R> Deserialize<'de> for Notification<M, R>
where
    M: Deserialize<'de>,
    R: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let body = ProxyOptionalParam::<'_, _, R>::deserialize(deserializer)?;
        let (_meta, params) = match body.params {
            Some(with_meta) => (with_meta._meta, with_meta._rest),
            None => {
                // JSON-RPC 2.0: params is optional. Treat absent params as {}.
                let empty = serde_json::Value::Object(serde_json::Map::new());
                let r = R::deserialize(empty).map_err(serde::de::Error::custom)?;
                (None, r)
            }
        };
        Ok(Notification {
            extensions: extensions_with_meta::<NotificationMetaObject>(_meta),
            method: body.method,
            params,
        })
    }
}

impl<M> Serialize for NotificationNoParam<M>
where
    M: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Emit `params` only when metadata is present, so the wire shape of
        // meta-less notifications stays `{"method": ...}`.
        match notification_meta(&self.extensions) {
            Some(_meta) => Proxy::serialize(
                &Proxy {
                    method: &self.method,
                    params: WithMeta {
                        _meta: Some(_meta),
                        _rest: (),
                    },
                },
                serializer,
            ),
            None => ProxyNoParam::serialize(
                &ProxyNoParam {
                    method: &self.method,
                },
                serializer,
            ),
        }
    }
}

impl<'de, M> Deserialize<'de> for NotificationNoParam<M>
where
    M: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let body = ProxyOptionalParam::<'_, _, Option<JsonObject>>::deserialize(deserializer)?;
        let _meta = body.params.and_then(|params| params._meta);
        Ok(NotificationNoParam {
            extensions: extensions_with_meta::<NotificationMetaObject>(_meta),
            method: body.method,
        })
    }
}

impl Serialize for CustomRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let _meta = request_meta(&self.extensions);
        let params = self.params.as_ref();

        let params = if _meta.is_some() || params.is_some() {
            Some(WithMeta {
                _meta,
                _rest: &self.params,
            })
        } else {
            None
        };

        ProxyOptionalParam::serialize(
            &ProxyOptionalParam {
                method: &self.method,
                params,
            },
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for CustomRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let body =
            ProxyOptionalParam::<'_, _, Option<serde_json::Value>>::deserialize(deserializer)?;
        let mut params = None;
        let mut _meta = None;
        if let Some(body_params) = body.params {
            params = body_params._rest;
            _meta = body_params._meta;
        }
        Ok(CustomRequest {
            extensions: extensions_with_meta::<RequestMetaObject>(_meta),
            method: body.method,
            params,
        })
    }
}

impl Serialize for CustomNotification {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let _meta = notification_meta(&self.extensions);
        let params = self.params.as_ref();

        let params = if _meta.is_some() || params.is_some() {
            Some(WithMeta {
                _meta,
                _rest: &self.params,
            })
        } else {
            None
        };

        ProxyOptionalParam::serialize(
            &ProxyOptionalParam {
                method: &self.method,
                params,
            },
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for CustomNotification {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let body =
            ProxyOptionalParam::<'_, _, Option<serde_json::Value>>::deserialize(deserializer)?;
        let mut params = None;
        let mut _meta = None;
        if let Some(body_params) = body.params {
            params = body_params._rest;
            _meta = body_params._meta;
        }
        Ok(CustomNotification {
            extensions: extensions_with_meta::<NotificationMetaObject>(_meta),
            method: body.method,
            params,
        })
    }
}

#[cfg(test)]
mod test {
    use serde_json::json;

    use crate::model::{
        CallToolRequest, CallToolRequestParams, CustomRequest, Extensions, InitializedNotification,
        ListToolsRequest, NotificationMetaObject, PingRequest, RequestMetaObject,
    };

    #[test]
    fn test_deserialize_lost_tools_request() {
        let _req: ListToolsRequest = serde_json::from_value(json!(
            {
                "method": "tools/list",
            }
        ))
        .unwrap();
    }

    #[test]
    fn test_no_duplicate_meta_both_sources() {
        // When both extensions and params contain _meta, the output should have
        // a single merged _meta key (not two separate ones).
        let mut extensions = Extensions::new();
        let mut ext_meta = RequestMetaObject::new();
        ext_meta.insert("traceId".to_string(), json!("abc"));
        extensions.insert(ext_meta);

        let mut params_meta = RequestMetaObject::new();
        params_meta.insert("progressToken".to_string(), json!(1));

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: Some(params_meta),
                name: "my_tool".into(),
                arguments: None,
                task: None,
                input_responses: None,
                request_state: None,
            },
        };

        let value = serde_json::to_value(&req).unwrap();
        let params = value.get("params").unwrap();

        // There should be exactly one _meta key (JSON objects naturally deduplicate)
        let meta = params.get("_meta").unwrap();

        // Both entries should be present in the merged _meta
        assert_eq!(meta.get("traceId").unwrap(), "abc");
        assert_eq!(meta.get("progressToken").unwrap(), 1);

        // Verify the raw JSON string has exactly one occurrence of "_meta"
        let raw = serde_json::to_string(&req).unwrap();
        assert_eq!(
            raw.matches("\"_meta\"").count(),
            1,
            "Expected exactly one _meta key in serialized output, got: {}",
            raw
        );
    }

    #[test]
    fn test_meta_only_from_extensions() {
        let mut extensions = Extensions::new();
        let mut ext_meta = RequestMetaObject::new();
        ext_meta.insert("traceId".to_string(), json!("ext-only"));
        extensions.insert(ext_meta);

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: None,
                name: "my_tool".into(),
                arguments: None,
                task: None,
                input_responses: None,
                request_state: None,
            },
        };

        let value = serde_json::to_value(&req).unwrap();
        let meta = value["params"]["_meta"].as_object().unwrap();
        assert_eq!(meta.get("traceId").unwrap(), "ext-only");
    }

    #[test]
    fn test_meta_only_from_params() {
        let mut params_meta = RequestMetaObject::new();
        params_meta.insert("progressToken".to_string(), json!(42));

        let req = CallToolRequest {
            extensions: Extensions::new(),
            method: Default::default(),
            params: CallToolRequestParams {
                meta: Some(params_meta),
                name: "my_tool".into(),
                arguments: None,
                task: None,
                input_responses: None,
                request_state: None,
            },
        };

        let value = serde_json::to_value(&req).unwrap();
        let meta = value["params"]["_meta"].as_object().unwrap();
        assert_eq!(meta.get("progressToken").unwrap(), 42);
    }

    #[test]
    fn test_no_meta_emitted_when_neither_source() {
        let req = CallToolRequest {
            extensions: Extensions::new(),
            method: Default::default(),
            params: CallToolRequestParams {
                meta: None,
                name: "my_tool".into(),
                arguments: None,
                task: None,
                input_responses: None,
                request_state: None,
            },
        };

        let value = serde_json::to_value(&req).unwrap();
        assert!(
            value["params"].get("_meta").is_none(),
            "Expected no _meta when neither source is populated"
        );
    }

    #[test]
    fn test_extensions_meta_takes_priority_on_conflict() {
        // When both sources have the same key, extensions should win.
        let mut extensions = Extensions::new();
        let mut ext_meta = RequestMetaObject::new();
        ext_meta.insert("shared_key".to_string(), json!("from_extensions"));
        extensions.insert(ext_meta);

        let mut params_meta = RequestMetaObject::new();
        params_meta.insert("shared_key".to_string(), json!("from_params"));
        params_meta.insert("params_only".to_string(), json!("kept"));

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: Some(params_meta),
                name: "my_tool".into(),
                arguments: None,
                task: None,
                input_responses: None,
                request_state: None,
            },
        };

        let value = serde_json::to_value(&req).unwrap();
        let meta = value["params"]["_meta"].as_object().unwrap();
        assert_eq!(meta.get("shared_key").unwrap(), "from_extensions");
        assert_eq!(meta.get("params_only").unwrap(), "kept");
    }

    #[test]
    fn test_round_trip_preserves_meta() {
        let mut extensions = Extensions::new();
        let mut ext_meta = RequestMetaObject::new();
        ext_meta.insert("traceId".to_string(), json!("round-trip"));
        extensions.insert(ext_meta);

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: None,
                name: "my_tool".into(),
                arguments: Some(serde_json::Map::from_iter([("x".to_string(), json!(1))])),
                task: None,
                input_responses: None,
                request_state: None,
            },
        };

        let serialized = serde_json::to_string(&req).unwrap();
        let deserialized: CallToolRequest = serde_json::from_str(&serialized).unwrap();

        // Extensions should have the meta after round-trip
        let meta = deserialized.extensions.get::<RequestMetaObject>().unwrap();
        assert_eq!(meta.get("traceId").unwrap(), "round-trip");

        // Params should be preserved
        assert_eq!(deserialized.params.name, "my_tool");
        assert_eq!(
            deserialized
                .params
                .arguments
                .as_ref()
                .unwrap()
                .get("x")
                .unwrap(),
            &json!(1)
        );
    }

    #[test]
    fn test_custom_request_no_duplicate_meta() {
        // CustomRequest uses Option<Value> as params — verify no duplicate _meta.
        let mut extensions = Extensions::new();
        let mut ext_meta = RequestMetaObject::new();
        ext_meta.insert("traceId".to_string(), json!("custom-ext"));
        extensions.insert(ext_meta);

        let params = Some(json!({
            "_meta": { "progressToken": 99 },
            "foo": "bar"
        }));

        let req = CustomRequest {
            extensions,
            method: "custom/method".into(),
            params,
        };

        let raw = serde_json::to_string(&req).unwrap();
        assert_eq!(
            raw.matches("\"_meta\"").count(),
            1,
            "Expected exactly one _meta key in CustomRequest output, got: {}",
            raw
        );

        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let meta = value["params"]["_meta"].as_object().unwrap();
        assert_eq!(meta.get("traceId").unwrap(), "custom-ext");
        assert_eq!(meta.get("progressToken").unwrap(), 99);
    }

    #[test]
    fn test_request_no_param_meta_round_trip() {
        // Ping-shaped requests must carry `params._meta` on the wire.
        let mut extensions = Extensions::new();
        let mut meta = RequestMetaObject::new();
        meta.insert("traceId".to_string(), json!("ping-trace"));
        extensions.insert(meta);

        let req = PingRequest {
            method: Default::default(),
            extensions,
        };

        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["params"]["_meta"]["traceId"], json!("ping-trace"));

        let deserialized: PingRequest = serde_json::from_value(value).unwrap();
        let meta = deserialized
            .extensions
            .get::<RequestMetaObject>()
            .expect("meta should survive the round-trip");
        assert_eq!(meta.get("traceId").unwrap(), &json!("ping-trace"));
    }

    #[test]
    fn test_request_no_param_without_meta_has_no_params_key() {
        let req = PingRequest {
            method: Default::default(),
            extensions: Extensions::new(),
        };
        let value = serde_json::to_value(&req).unwrap();
        assert!(
            value.get("params").is_none(),
            "meta-less no-param requests must keep the historical wire shape: {value}"
        );
    }

    #[test]
    fn test_notification_no_param_meta_round_trip() {
        // Initialized-shaped notifications must carry `params._meta` on the wire.
        let mut extensions = Extensions::new();
        let mut meta = NotificationMetaObject::new();
        meta.insert("traceId".to_string(), json!("init-trace"));
        extensions.insert(meta);

        let notification = InitializedNotification {
            method: Default::default(),
            extensions,
        };

        let value = serde_json::to_value(&notification).unwrap();
        assert_eq!(value["params"]["_meta"]["traceId"], json!("init-trace"));

        let deserialized: InitializedNotification = serde_json::from_value(value).unwrap();
        let meta = deserialized
            .extensions
            .get::<NotificationMetaObject>()
            .expect("meta should survive the round-trip");
        assert_eq!(meta.get("traceId").unwrap(), &json!("init-trace"));
    }

    #[test]
    fn test_notification_no_param_without_meta_has_no_params_key() {
        let notification = InitializedNotification {
            method: Default::default(),
            extensions: Extensions::new(),
        };
        let value = serde_json::to_value(&notification).unwrap();
        assert!(
            value.get("params").is_none(),
            "meta-less no-param notifications must keep the historical wire shape: {value}"
        );
    }

    #[test]
    fn test_no_param_ignores_unknown_params_fields() {
        // Old/foreign peers may send params without _meta; both shapes must parse.
        let _req: PingRequest =
            serde_json::from_value(json!({"method": "ping", "params": {}})).unwrap();
        let _req: PingRequest =
            serde_json::from_value(json!({"method": "ping", "params": {"unknown": 1}})).unwrap();
        let _req: PingRequest = serde_json::from_value(json!({"method": "ping"})).unwrap();
    }

    #[test]
    fn test_legacy_meta_extension_still_serializes() {
        // Pre-3.x code inserts `MetaObject` into extensions through the
        // deprecated `Meta` name; its metadata must not be silently dropped.
        let mut extensions = Extensions::new();
        let mut legacy = crate::model::MetaObject::new();
        legacy.insert("traceId".to_string(), json!("legacy"));
        extensions.insert(legacy);

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: None,
                name: "my_tool".into(),
                arguments: None,
                task: None,
                input_responses: None,
                request_state: None,
            },
        };

        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["params"]["_meta"]["traceId"], json!("legacy"));
    }

    #[test]
    fn test_typed_meta_wins_over_legacy_extension_on_conflict() {
        let mut extensions = Extensions::new();
        let mut legacy = crate::model::MetaObject::new();
        legacy.insert("shared".to_string(), json!("legacy"));
        legacy.insert("legacy_only".to_string(), json!("kept"));
        extensions.insert(legacy);
        let mut typed = RequestMetaObject::new();
        typed.insert("shared".to_string(), json!("typed"));
        extensions.insert(typed);

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: None,
                name: "my_tool".into(),
                arguments: None,
                task: None,
                input_responses: None,
                request_state: None,
            },
        };

        let value = serde_json::to_value(&req).unwrap();
        let meta = value["params"]["_meta"].as_object().unwrap();
        assert_eq!(meta.get("shared").unwrap(), "typed");
        assert_eq!(meta.get("legacy_only").unwrap(), "kept");
    }

    #[test]
    fn test_arbitrary_meta_keys_round_trip_unchanged() {
        let input = json!({
            "method": "tools/call",
            "params": {
                "_meta": {
                    "progressToken": 5,
                    "vendor.example/custom": {"nested": ["a", 1, null]},
                    "another-key": true
                },
                "name": "my_tool"
            }
        });
        let req: CallToolRequest = serde_json::from_value(input.clone()).unwrap();
        let output = serde_json::to_value(&req).unwrap();
        assert_eq!(input, output);
    }
}
