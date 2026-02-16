use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{
    CustomNotification, CustomRequest, Extensions, Meta, Notification, NotificationNoParam,
    Request, RequestNoParam, RequestOptionalParam,
};
#[derive(Deserialize)]
struct WithMeta<'a, P> {
    _meta: Option<Cow<'a, Meta>>,
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
        let params_meta: Option<Meta> = rest_value
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

impl<M, R> Serialize for Request<M, R>
where
    M: Serialize,
    R: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let extensions = &self.extensions;
        let _meta = extensions.get::<Meta>().map(Cow::Borrowed);
        Proxy::serialize(
            &Proxy {
                method: &self.method,
                params: WithMeta {
                    _rest: &self.params,
                    _meta,
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
        let _meta = body.params._meta.map(|m| m.into_owned());
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(Request {
            extensions,
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
        let extensions = &self.extensions;
        let _meta = extensions.get::<Meta>().map(Cow::Borrowed);
        Proxy::serialize(
            &Proxy {
                method: &self.method,
                params: WithMeta {
                    _rest: &self.params,
                    _meta,
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
            _meta = body_params._meta.map(|m| m.into_owned());
        }
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(RequestOptionalParam {
            extensions,
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
        let extensions = &self.extensions;
        let _meta = extensions.get::<Meta>().map(Cow::Borrowed);
        ProxyNoParam::serialize(
            &ProxyNoParam {
                method: &self.method,
            },
            serializer,
        )
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
        let body = ProxyNoParam::<_>::deserialize(deserializer)?;
        let extensions = Extensions::new();
        Ok(RequestNoParam {
            extensions,
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
        let extensions = &self.extensions;
        let _meta = extensions.get::<Meta>().map(Cow::Borrowed);
        Proxy::serialize(
            &Proxy {
                method: &self.method,
                params: WithMeta {
                    _rest: &self.params,
                    _meta,
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
        let body = Proxy::deserialize(deserializer)?;
        let _meta = body.params._meta.map(|m| m.into_owned());
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(Notification {
            extensions,
            method: body.method,
            params: body.params._rest,
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
        let extensions = &self.extensions;
        let _meta = extensions.get::<Meta>().map(Cow::Borrowed);
        ProxyNoParam::serialize(
            &ProxyNoParam {
                method: &self.method,
            },
            serializer,
        )
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
        let body = ProxyNoParam::<_>::deserialize(deserializer)?;
        let extensions = Extensions::new();
        Ok(NotificationNoParam {
            extensions,
            method: body.method,
        })
    }
}

impl Serialize for CustomRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let extensions = &self.extensions;
        let _meta = extensions.get::<Meta>().map(Cow::Borrowed);
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
            _meta = body_params._meta.map(|m| m.into_owned());
        }
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(CustomRequest {
            extensions,
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
        let extensions = &self.extensions;
        let _meta = extensions.get::<Meta>().map(Cow::Borrowed);
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
            _meta = body_params._meta.map(|m| m.into_owned());
        }
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(CustomNotification {
            extensions,
            method: body.method,
            params,
        })
    }
}

#[cfg(test)]
mod test {
    use serde_json::json;

    use crate::model::{
        CallToolRequest, CallToolRequestParams, CustomRequest, Extensions, ListToolsRequest, Meta,
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
        let mut ext_meta = Meta::new();
        ext_meta.0.insert("traceId".to_string(), json!("abc"));
        extensions.insert(ext_meta);

        let mut params_meta = Meta::new();
        params_meta.0.insert("progressToken".to_string(), json!(1));

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: Some(params_meta),
                name: "my_tool".into(),
                arguments: None,
                task: None,
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
        let mut ext_meta = Meta::new();
        ext_meta.0.insert("traceId".to_string(), json!("ext-only"));
        extensions.insert(ext_meta);

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: None,
                name: "my_tool".into(),
                arguments: None,
                task: None,
            },
        };

        let value = serde_json::to_value(&req).unwrap();
        let meta = value["params"]["_meta"].as_object().unwrap();
        assert_eq!(meta.get("traceId").unwrap(), "ext-only");
    }

    #[test]
    fn test_meta_only_from_params() {
        let mut params_meta = Meta::new();
        params_meta.0.insert("progressToken".to_string(), json!(42));

        let req = CallToolRequest {
            extensions: Extensions::new(),
            method: Default::default(),
            params: CallToolRequestParams {
                meta: Some(params_meta),
                name: "my_tool".into(),
                arguments: None,
                task: None,
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
        let mut ext_meta = Meta::new();
        ext_meta
            .0
            .insert("shared_key".to_string(), json!("from_extensions"));
        extensions.insert(ext_meta);

        let mut params_meta = Meta::new();
        params_meta
            .0
            .insert("shared_key".to_string(), json!("from_params"));
        params_meta
            .0
            .insert("params_only".to_string(), json!("kept"));

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: Some(params_meta),
                name: "my_tool".into(),
                arguments: None,
                task: None,
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
        let mut ext_meta = Meta::new();
        ext_meta
            .0
            .insert("traceId".to_string(), json!("round-trip"));
        extensions.insert(ext_meta);

        let req = CallToolRequest {
            extensions,
            method: Default::default(),
            params: CallToolRequestParams {
                meta: None,
                name: "my_tool".into(),
                arguments: Some(serde_json::Map::from_iter([("x".to_string(), json!(1))])),
                task: None,
            },
        };

        let serialized = serde_json::to_string(&req).unwrap();
        let deserialized: CallToolRequest = serde_json::from_str(&serialized).unwrap();

        // Extensions should have the meta after round-trip
        let meta = deserialized.extensions.get::<Meta>().unwrap();
        assert_eq!(meta.0.get("traceId").unwrap(), "round-trip");

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
        let mut ext_meta = Meta::new();
        ext_meta
            .0
            .insert("traceId".to_string(), json!("custom-ext"));
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
}
