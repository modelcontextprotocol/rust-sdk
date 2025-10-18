use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{
    Extensions, Meta, Notification, NotificationNoParam, Request, RequestNoParam,
    RequestOptionalParam,
};
#[derive(Serialize, Deserialize)]
struct WithMeta<'a, P> {
    #[serde(skip_serializing_if = "Option::is_none")]
    _meta: Option<Cow<'a, Meta>>,
    #[serde(flatten)]
    _rest: P,
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

#[cfg(test)]
mod test {
    use serde_json::json;

    use super::*;
    use crate::model::{Extensions, ListToolsRequest, Meta};

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
    fn test_request_serialize_without_meta() {
        let req = Request {
            method: "test_method".to_string(),
            params: json!({"key": "value"}),
            extensions: Extensions::new(),
        };
        let serialized = serde_json::to_value(&req).unwrap();
        assert_eq!(serialized["method"], "test_method");
        assert_eq!(serialized["params"]["key"], "value");
        assert!(serialized["params"]["_meta"].is_null());
    }

    #[test]
    fn test_request_serialize_with_meta() {
        let mut extensions = Extensions::new();
        let mut meta = Meta::new();
        meta.insert("custom".to_string(), json!("data"));
        extensions.insert(meta);

        let req = Request {
            method: "test_method".to_string(),
            params: json!({"key": "value"}),
            extensions,
        };
        let serialized = serde_json::to_value(&req).unwrap();
        assert_eq!(serialized["params"]["_meta"]["custom"], "data");
    }

    #[test]
    fn test_request_deserialize() {
        let json_val = json!({
            "method": "test_method",
            "params": {"key": "value"}
        });
        let req: Request<String, serde_json::Value> = serde_json::from_value(json_val).unwrap();
        assert_eq!(req.method, "test_method");
        assert_eq!(req.params["key"], "value");
    }

    #[test]
    fn test_request_optional_param_serialize() {
        let req = RequestOptionalParam {
            method: "test_method".to_string(),
            params: Some(json!({"key": "value"})),
            extensions: Extensions::new(),
        };
        let serialized = serde_json::to_value(&req).unwrap();
        assert_eq!(serialized["method"], "test_method");
        assert!(serialized["params"].is_object());
    }

    #[test]
    fn test_request_optional_param_deserialize_with_params() {
        let json_val = json!({
            "method": "test_method",
            "params": {"key": "value"}
        });
        let req: RequestOptionalParam<String, serde_json::Value> =
            serde_json::from_value(json_val).unwrap();
        assert_eq!(req.method, "test_method");
        assert!(req.params.is_some());
    }

    #[test]
    fn test_request_optional_param_deserialize_without_params() {
        let json_val = json!({
            "method": "test_method"
        });
        let req: RequestOptionalParam<String, serde_json::Value> =
            serde_json::from_value(json_val).unwrap();
        assert_eq!(req.method, "test_method");
        assert!(req.params.is_none());
    }

    #[test]
    fn test_request_no_param_serialize() {
        let req = RequestNoParam {
            method: "test_method".to_string(),
            extensions: Extensions::new(),
        };
        let serialized = serde_json::to_value(&req).unwrap();
        assert_eq!(serialized["method"], "test_method");
        assert!(serialized.get("params").is_none());
    }

    #[test]
    fn test_request_no_param_deserialize() {
        let json_val = json!({
            "method": "test_method"
        });
        let req: RequestNoParam<String> = serde_json::from_value(json_val).unwrap();
        assert_eq!(req.method, "test_method");
    }

    #[test]
    fn test_notification_serialize() {
        let notif = Notification {
            method: "test_notification".to_string(),
            params: json!({"data": "test"}),
            extensions: Extensions::new(),
        };
        let serialized = serde_json::to_value(&notif).unwrap();
        assert_eq!(serialized["method"], "test_notification");
        assert_eq!(serialized["params"]["data"], "test");
    }

    #[test]
    fn test_notification_deserialize() {
        let json_val = json!({
            "method": "test_notification",
            "params": {"data": "test"}
        });
        let notif: Notification<String, serde_json::Value> =
            serde_json::from_value(json_val).unwrap();
        assert_eq!(notif.method, "test_notification");
        assert_eq!(notif.params["data"], "test");
    }

    #[test]
    fn test_notification_no_param_serialize() {
        let notif = NotificationNoParam {
            method: "test_notification".to_string(),
            extensions: Extensions::new(),
        };
        let serialized = serde_json::to_value(&notif).unwrap();
        assert_eq!(serialized["method"], "test_notification");
    }

    #[test]
    fn test_notification_no_param_deserialize() {
        let json_val = json!({
            "method": "test_notification"
        });
        let notif: NotificationNoParam<String> = serde_json::from_value(json_val).unwrap();
        assert_eq!(notif.method, "test_notification");
    }
}
