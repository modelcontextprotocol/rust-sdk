// Transport layer for rmcp_typescript binding
// Add #[wasm_bindgen] wrappers as needed for TS/JS

pub mod sse;

use std::pin::Pin;
use tokio::net::TcpStream;
use tokio::io::{Stdin, Stdout};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use napi::Env;
use crate::transport::sse::JsSseTransport;
use rmcp::transport::sse::ReqwestSseClient;
use rmcp::transport::SseTransport;
use reqwest;


#[napi]
pub enum JsTransportEnum {
    Tcp,
    Stdio,
    Sse,
}


pub enum JsTransportInner {
    Tcp(Option<Pin<Box<TcpStream>>>),
    Stdio(Option<(Stdin, Stdout)>),
    Sse(Option<SseTransport<ReqwestSseClient, reqwest::Error>>),
}

#[napi]
pub struct JsTransport {
    pub kind: JsTransportEnum,
    #[napi(skip)]
    pub inner: Option<JsTransportInner>,
}

#[napi]
impl JsTransport {
    #[napi(factory)]
    pub fn from_tcp() -> Self {
        JsTransport {
            kind: JsTransportEnum::Tcp,
            inner: Some(JsTransportInner::Tcp(None)),
        }
    }

    #[napi(factory)]
    pub fn from_stdio() -> Self {
        JsTransport {
            kind: JsTransportEnum::Stdio,
            inner: Some(JsTransportInner::Stdio(None)),
        }
    }

    #[napi(factory)]
    pub fn from_sse(sse: Reference<JsSseTransport>, env: Env) -> napi::Result<Self> {
        let mut sse = sse.clone(env)?;
        let inner = sse.inner.take().ok_or_else(|| napi::Error::from_reason("SSE transport not initialized"))?;
        Ok(JsTransport {
            kind: JsTransportEnum::Sse,
            inner: Some(JsTransportInner::Sse(Some(inner))),
        })
    }

    #[napi(getter)]
    pub fn kind(&self) -> JsTransportEnum {
        self.kind.clone()
    }

    #[napi]
    pub fn is_tcp(&self) -> bool {
        matches!(self.kind, JsTransportEnum::Tcp)
    }
    #[napi]
    pub fn is_stdio(&self) -> bool {
        matches!(self.kind, JsTransportEnum::Stdio)
    }
    #[napi]
    pub fn is_sse(&self) -> bool {
        matches!(self.kind, JsTransportEnum::Sse)
    }
}
