//! Transport layer for rmcp_typescript binding.
//!
//! This module provides transport types and utilities for connecting to RMCP servers from TypeScript/JavaScript.
//! It supports TCP, stdio, and SSE transports, and is designed for use with Node.js via `napi` bindings.
//!
//! # Example (TypeScript)
//!
//! ```typescript
//! import { JsTransport } from 'rmcp-typescript';
//! const tcpTransport = JsTransport.fromTcp();
//! const sseTransport = JsTransport.fromSse(sseInstance);
//! ```

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


/// Enum representing the type of transport used to connect to the RMCP server.
///
/// - `Tcp`: TCP socket transport
/// - `Stdio`: Standard input/output transport
/// - `Sse`: Server-sent events transport
///
/// # Example (TypeScript)
/// ```typescript
/// if (transport.kind === 'Sse') { ... }
/// ```
#[napi(string_enum)]
#[derive(PartialEq, Debug)]
pub enum JsTransportEnum {
    Tcp,
    Stdio,
    Sse,
}

/// Internal enum representing the underlying transport implementation.
///
/// Not exposed directly to TypeScript/JavaScript users.
pub enum JsTransportInner {
    Tcp(Option<Pin<Box<TcpStream>>>),
    Stdio(Option<(Stdin, Stdout)>),
    Sse(Option<SseTransport<ReqwestSseClient, reqwest::Error>>),
}

/// Transport handle for connecting to an RMCP server from TypeScript/JavaScript.
///
/// Use the factory methods to create a transport for TCP, stdio, or SSE.
///
/// # Example (TypeScript)
/// ```typescript
/// const tcpTransport = JsTransport.fromTcp();
/// const sseTransport = JsTransport.fromSse(sseInstance, env);
/// ```
#[napi]
pub struct JsTransport {
    /// The type of transport (TCP, Stdio, or SSE).
    pub kind: JsTransportEnum,
    #[napi(skip)]
    pub inner: Option<JsTransportInner>,
}

impl FromNapiValue for JsTransportInner {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
        unsafe {
            let obj = Object::from_napi_value(env, napi_val)?;
            let kind: Option<String> = obj.get("kind")?;
            match kind.as_deref() {
                Some("Tcp") => Ok(JsTransportInner::Tcp(None)),
                Some("Stdio") => Ok(JsTransportInner::Stdio(None)),
                Some("Sse") => Ok(JsTransportInner::Sse(None)),
                _ => Err(napi::Error::from_reason("Invalid transport kind")),
            }
        }
    }
}

impl FromNapiValue for JsTransport {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
        unsafe {
            let obj = Object::from_napi_value(env, napi_val)?;
            println!("FromNapiValue<JsTransport> starting");
            let keys: Vec<String> = Object::keys(&obj)?;
            println!("FromNapiValue<JsTransport> got keys: {:?}", keys);
            println!("FromNapiValue<JsTransport> about to get kind");
            
            // Get kind as string since we're using string_enum
            let kind_str: Option<String> = obj.get("kind")?;
            println!("FromNapiValue<JsTransport> got kind as string: {:?}", kind_str);
            let kind = match kind_str.as_deref() {
                Some("Tcp") => JsTransportEnum::Tcp,
                Some("Stdio") => JsTransportEnum::Stdio,
                Some("Sse") => JsTransportEnum::Sse,
                _ => return Err(napi::Error::from_reason("Invalid transport kind string")),
            };
            
            println!("FromNapiValue<JsTransport> matched kind: {:?}", kind);
            
            // For SSE transport, we need to preserve the inner field
            if kind == JsTransportEnum::Sse {
                println!("FromNapiValue<JsTransport> handling SSE transport");
                println!("FromNapiValue<JsTransport> checking for inner property");
                let sse_prop = obj.get::<&str, Reference<JsSseTransport>>("inner");
                println!("FromNapiValue<JsTransport> inner property exists: {}", sse_prop.is_ok());
                if let Some(sse) = sse_prop? {
                    println!("FromNapiValue<JsTransport> got SSE reference");
                    let mut sse = sse.clone(env.into())?;
                    let inner = sse.inner.take().ok_or_else(|| napi::Error::from_reason("SSE transport not initialized"))?;
                    println!("FromNapiValue<JsTransport> created SSE transport with inner");
                    return Ok(JsTransport {
                        kind,
                        inner: Some(JsTransportInner::Sse(Some(inner))),
                    });
                } else {
                    println!("FromNapiValue<JsTransport> no SSE reference found");
                    return Err(napi::Error::from_reason("SSE transport reference not found"));
                }
            }
            
            // For other transport types, use the default inner
            let inner = match kind {
                JsTransportEnum::Tcp => Some(JsTransportInner::Tcp(None)),
                JsTransportEnum::Stdio => Some(JsTransportInner::Stdio(None)),
                JsTransportEnum::Sse => Some(JsTransportInner::Sse(None)),
            };
            println!("FromNapiValue<JsTransport> created transport with inner is_some: {}", inner.is_some());
            
            Ok(JsTransport { kind, inner })
        }
    }
}

#[napi]
impl JsTransport {
    /// Create a TCP transport for connecting to an RMCP server.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const transport = JsTransport.fromTcp();
    /// ```
    #[napi(factory)]
    pub fn from_tcp() -> Self {
        JsTransport {
            kind: JsTransportEnum::Tcp,
            inner: Some(JsTransportInner::Tcp(None)),
        }
    }

    /// Create a stdio transport for connecting to an RMCP server.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const transport = JsTransport.fromStdio();
    /// ```
    #[napi(factory)]
    pub fn from_stdio() -> Self {
        JsTransport {
            kind: JsTransportEnum::Stdio,
            inner: Some(JsTransportInner::Stdio(None)),
        }
    }

    /// Create an SSE transport for connecting to an RMCP server.
    ///
    /// # Arguments
    /// * `sse` - An instance of `JsSseTransport` (as returned by `await JsSseTransport.start(endpoint)` in TypeScript).
    ///
    /// > **Note:** The `env` parameter is injected automatically by the napi binding and should NOT be provided by the user in TypeScript.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const sseEndpoint = 'http://localhost:8000/sse';
    /// const sseTransport = await JsSseTransport.start(sseEndpoint);
    /// const transport = JsTransport.fromSse(sseTransport);
    /// ```
    #[napi(factory)]
    pub fn from_sse(sse: Reference<JsSseTransport>, env: Env) -> napi::Result<Self> {
        println!("JsTransport.fromSse received sse transport");
        let mut sse = sse.clone(env)?;
        let inner = sse.inner.take().ok_or_else(|| napi::Error::from_reason("SSE transport not initialized"))?;
        println!("JsTransport.fromSse successful");
        Ok(JsTransport {
            kind: JsTransportEnum::Sse,
            inner: Some(JsTransportInner::Sse(Some(inner))),
        })
    }

    /// Get the kind of this transport (TCP, Stdio, or SSE).
    #[napi(getter)]
    pub fn kind(&self) -> JsTransportEnum {
        self.kind.clone()
    }

    /// Returns true if this transport is TCP.
    #[napi]
    pub fn is_tcp(&self) -> bool {
        matches!(self.kind, JsTransportEnum::Tcp)
    }
    /// Returns true if this transport is stdio.
    #[napi]
    pub fn is_stdio(&self) -> bool {
        matches!(self.kind, JsTransportEnum::Stdio)
    }
    /// Returns true if this transport is SSE.
    #[napi]
    pub fn is_sse(&self) -> bool {
        matches!(self.kind, JsTransportEnum::Sse)
    }
}
