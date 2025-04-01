use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::{Extensions, Meta, Notification, NotificationNoParam, Request, RequestNoParam};

// serde helper type
#[derive(Serialize, Deserialize)]
struct WithMeta<'a, T> {
    _meta: Option<Cow<'a, Meta>>,
    #[serde(flatten)]
    _rest: T,
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
        let body = WithMeta { _meta, _rest: self };
        WithMeta::serialize(&body, serializer)
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
        let body = WithMeta::deserialize(deserializer)?;
        let _meta = body._meta.map(|m| m.into_owned());
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(Request {
            extensions,
            ..body._rest
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
        let body = WithMeta { _meta, _rest: self };
        WithMeta::serialize(&body, serializer)
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
        let body = WithMeta::deserialize(deserializer)?;
        let _meta = body._meta.map(|m| m.into_owned());
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(RequestNoParam {
            extensions,
            ..body._rest
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
        let body = WithMeta { _meta, _rest: self };
        WithMeta::serialize(&body, serializer)
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
        let body = WithMeta::deserialize(deserializer)?;
        let _meta = body._meta.map(|m| m.into_owned());
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(Notification {
            extensions,
            ..body._rest
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
        let body = WithMeta { _meta, _rest: self };
        WithMeta::serialize(&body, serializer)
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
        let body = WithMeta::deserialize(deserializer)?;
        let _meta = body._meta.map(|m| m.into_owned());
        let mut extensions = Extensions::new();
        if let Some(meta) = _meta {
            extensions.insert(meta);
        }
        Ok(NotificationNoParam {
            extensions,
            ..body._rest
        })
    }
}
