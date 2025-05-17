//! Model types for rmcp_typescript binding.
//!
//! This module defines core types (client info, implementation info, protocol version, etc.) exposed to TypeScript/JavaScript via napi bindings.
//! These types are used for configuring and interacting with the RMCP SDK from Node.js or browser environments.
//!
//! # Example (TypeScript)
//!
//! ```typescript
//! import { JsClientInfo, JsImplementation, ProtocolVersion } from 'rmcp-typescript';
//! const info = new JsClientInfo('2.0', capabilities, impl, env);
//! ```

pub mod tool;
pub mod resource;
pub mod prompt;
pub mod prompt_message;
pub mod capabilities;

use napi::bindgen_prelude::*;
use napi::JsObject;
use napi::NapiValue;
use napi_derive::napi;
use crate::model::capabilities::JsClientCapabilities;
use serde_json;
use crate::service::JsClient;
use crate::transport::{JsTransport, JsTransportInner};
use std::sync::Arc;
use rmcp::model::ClientInfo;
use rmcp::ServiceExt;


/// Client info for establishing a connection to an RMCP server.
///
/// Used to specify protocol version, capabilities, and implementation details.
///
/// # Example (TypeScript)
/// ```typescript
/// const info = new JsClientInfo('2.0', capabilities, impl, env);
/// ```
#[napi]
#[derive(Clone)]
pub struct JsClientInfo {
    #[napi(skip)]
    pub inner: ClientInfo,
}

#[napi]
impl JsClientInfo {
    /// Construct a new `JsClientInfo`.
    ///
    /// # Arguments
    /// * `protocol_version` - The protocol version string (e.g. "2.0").
    /// * `capabilities` - Reference to client capabilities.
    /// * `client_info` - Reference to implementation info.
    ///
    /// > **Note:** Any extra parameters (such as environment/context) are injected by napi and should NOT be passed by the TypeScript user.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const info = new JsClientInfo('2.0', capabilities, impl);
    /// ```
    #[napi(constructor)]
    pub fn new(protocol_version: String, capabilities: Reference<JsClientCapabilities>, client_info: Reference<JsImplementation>, env: Env) -> napi::Result<JsClientInfo> {
        println!("JsClientInfo::new received protocol_version: {}", protocol_version);
        println!("JsClientInfo::new received capabilities: valid");
        println!("JsClientInfo::new received client_info: valid");
        let protocol_version = serde_json::from_str(&format!("\"{}\"", protocol_version))
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        println!("JsClientInfo::new parsed protocol_version: {:?}", protocol_version);
        let capabilities = capabilities.clone(env)?.to_rust();
        println!("JsClientInfo::new capabilities after clone: {:?}", capabilities);
        let client_info = client_info.clone(env)?.to_rust();
        println!("JsClientInfo::new client_info after clone: {:?}", client_info);

        let inner = rmcp::model::ClientInfo {
            protocol_version,
            capabilities,
            client_info,
        };
        Ok(JsClientInfo { inner })
    }

    /// Get the protocol version for this client info.
    #[napi(getter)]
    pub fn protocol_version(&self) -> String {
        format!("{:?}", self.inner.protocol_version)
    }

    /// Get the client capabilities for this client info.
    #[napi(getter)]
    pub fn capabilities(&self) -> JsClientCapabilities {
        JsClientCapabilities::from_rust(&self.inner.capabilities)
    }

    /// Get the implementation info for this client info.
    #[napi(getter)]
    pub fn client_info(&self) -> JsImplementation {
        JsImplementation::from_rust(&self.inner.client_info)
    }

    /// Serve the RMCP client using the provided transport.
    ///
    /// > **Note:** Any extra parameters (such as environment/context) are injected by napi and should NOT be passed by the TypeScript user.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const client = await info.serve(transport);
    /// ```
    #[napi]
    pub fn serve(&self, transport: Reference<JsTransport>, env: Env) -> napi::Result<JsObject> {
        let cloned_transport = transport.clone(env)?;
        println!("JsClientInfo::serve cloned_transport: {:?}", cloned_transport.kind);
        let this = self.clone();

        let result = napi::bindgen_prelude::execute_tokio_future(
            env.raw(),
            async move {
                this.serve_inner(cloned_transport).await
            },
            |raw_env, client| {
                let env = unsafe { Env::from_raw(raw_env) };
                let mut obj = env.create_object()?;
                obj.set_named_property("inner", client)?;
                unsafe { napi::bindgen_prelude::ToNapiValue::to_napi_value(raw_env, obj) }
            }
        )?;
        
        Ok(unsafe { JsObject::from_raw(env.raw(), result)? })
    }
}

impl JsClientInfo {
    pub fn to_rust(&self) -> rmcp::model::ClientInfo {
        self.inner.clone()
    }
    pub fn from_rust(core: &rmcp::model::ClientInfo) -> JsClientInfo {
        JsClientInfo {
            inner: core.clone(),
        }
    }

    async fn serve_inner(&self, mut transport: Reference<JsTransport>) -> napi::Result<JsClient> {
        println!("JsClientInfo::serve_inner starting");
        let info = self.inner.clone();
        
        match transport.inner.take() {
            Some(JsTransportInner::Tcp(stream)) => {
                println!("JsClientInfo::serve_inner received TCP stream");
                match stream {
                    Some(stream) => {
                        println!("JsClientInfo::serve_inner TCP stream is Some");
                        let running = info.serve(stream).await
                            .map_err(|e| napi::Error::from_reason(format!("TCP Serve IO error: {}", e)))?;
                        let peer = running.peer().clone();
                        let peer_arc = Arc::new(peer);
                        let mut js_client = JsClient::new();
                        js_client.set_inner(peer_arc);
                        Ok(js_client)
                    }
                    None => {
                        println!("JsClientInfo::serve_inner TCP stream is None");
                        Err(napi::Error::from_reason("TCP stream not initialized"))
                    }
                }
            }
            Some(JsTransportInner::Stdio(stdin_stdout)) => {
                println!("JsClientInfo::serve_inner received STDIO");
                match stdin_stdout {
                    Some((stdin, stdout)) => {
                        println!("JsClientInfo::serve_inner STDIO is Some");
                        let running = info.serve((stdin, stdout)).await
                            .map_err(|e| napi::Error::from_reason(format!("STDIO Serve IO error: {}", e)))?;
                        let peer = running.peer().clone();
                        let peer_arc = Arc::new(peer);
                        let mut js_client = JsClient::new();
                        js_client.set_inner(peer_arc);
                        Ok(js_client)
                    }
                    None => {
                        println!("JsClientInfo::serve_inner STDIO is None");
                        Err(napi::Error::from_reason("STDIO not initialized"))
                    }
                }
            }
            Some(JsTransportInner::Sse(sse)) => {
                println!("JsClientInfo::serve_inner received SSE");
                match sse {
                    Some(sse) => {
                        println!("JsClientInfo::serve_inner SSE is Some");
                        let running = info.serve(sse).await
                            .map_err(|e| napi::Error::from_reason(format!("SSE Serve IO error: {}", e)))?;
                        let peer = running.peer().clone();
                        let peer_arc = Arc::new(peer);
                        let mut js_client = JsClient::new();
                        js_client.set_inner(peer_arc);
                        Ok(js_client)
                    }
                    None => {
                        println!("JsClientInfo::serve_inner SSE is None");
                        Err(napi::Error::from_reason("SSE transport not initialized"))
                    }
                }
            }
            None => {
                println!("JsClientInfo::serve_inner transport inner is None");
                Err(napi::Error::from_reason("Transport not initialized"))
            }
        }
    }
}

/// Implementation info for the RMCP client (name and version).
///
/// # Example (TypeScript)
/// ```typescript
/// const impl = new JsImplementation('my-client', '1.0.0');
/// ```
#[napi]
#[derive(Clone, Debug, PartialEq)]
pub struct JsImplementation {
    pub name: String,
    pub version: String,
}

#[napi]
impl JsImplementation {
    /// Construct a new implementation info object.
    ///
    /// # Arguments
    /// * `name` - Name of the implementation.
    /// * `version` - Version string.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const impl = new JsImplementation('my-client', '1.0.0');
    /// ```
    #[napi(constructor)]
    pub fn new(name: String, version: String) -> JsImplementation {
        println!("JsImplementation::new received name: {}", name);
        println!("JsImplementation::new received version: {}", version);
        JsImplementation { name, version }
    }

    #[napi(getter)]
    pub fn name(&self) -> String {
        self.name.clone()
    }

    #[napi(getter)]
    pub fn version(&self) -> String {
        self.version.clone()
    }
}

impl Default for JsImplementation {
    fn default() -> Self {
        JsImplementation {
            name: env!("CARGO_CRATE_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

impl JsImplementation {
    pub fn to_rust(&self) -> rmcp::model::Implementation {
        rmcp::model::Implementation {
            name: self.name.clone(),
            version: self.version.clone(),
        }
    }
    pub fn from_rust(core: &rmcp::model::Implementation) -> JsImplementation {
        JsImplementation {
            name: core.name.clone(),
            version: core.version.clone(),
        }
    }
}

/// Protocol version wrapper for RMCP.
///
/// # Example (TypeScript)
/// ```typescript
/// const version = new ProtocolVersion('2.0');
/// ```
#[napi]
#[derive(Clone)]
pub struct ProtocolVersion {
    pub value: String,
}

#[napi]
impl ProtocolVersion {
    /// Construct a new protocol version.
    ///
    /// # Arguments
    /// * `value` - Version string (e.g. "2.0").
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const version = new ProtocolVersion('2.0');
    /// ```
    #[napi(constructor)]
    pub fn new(value: String) -> ProtocolVersion {
        ProtocolVersion { value }
    }

    /// Get the protocol version value.
    #[napi(getter)]
    pub fn value(&self) -> String {
        self.value.clone()
    }

    /// Convert this protocol version to a string.
    #[napi(js_name = "toString")]
    pub fn to_string(&self) -> String {
        self.value.clone()
    }

    /// Get the latest supported protocol version.
    #[napi]
    pub fn latest() -> ProtocolVersion {
        ProtocolVersion { value: "2.0".to_string() }
    }
}

#[napi]
pub fn json_rpc_version_2_0() -> String {
    "2.0".to_string()
}

/// Information about the RMCP server or client.
///
/// # Example (TypeScript)
/// ```typescript
/// const info = new JsInfo('2.0', 'my-server', '1.0.0');
/// ```
#[napi]
#[derive(Clone)]
pub struct JsInfo {
    pub protocol_version: String,
    pub name: String,
    pub version: String,
}

#[napi]
impl JsInfo {
    /// Construct a new info object.
    ///
    /// # Arguments
    /// * `protocol_version` - Protocol version string.
    /// * `name` - Name of the server/client.
    /// * `version` - Version string.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const info = new JsInfo('2.0', 'my-server', '1.0.0');
    /// ```
    #[napi(constructor)]
    pub fn new(protocol_version: String, name: String, version: String) -> JsInfo {
        JsInfo {
            protocol_version,
            name,
            version,
        }
    }

    /// Get the protocol version for this info.
    #[napi(getter)]
    pub fn protocol_version(&self) -> String {
        self.protocol_version.clone()
    }

    #[napi(getter)]
    pub fn name(&self) -> String {
        self.name.clone()
    }

    #[napi(getter)]
    pub fn version(&self) -> String {
        self.version.clone()
    }
}

impl JsInfo {
    pub fn server(info: rmcp::model::InitializeResult) -> Self {
        Self {
            protocol_version: format!("{:?}", info.protocol_version),
            name: info.server_info.name,
            version: info.server_info.version,
        }
    }

    pub fn client(param: rmcp::model::InitializeRequestParam) -> Self {
        Self {
            protocol_version: format!("{:?}", param.protocol_version),
            name: param.client_info.name,
            version: param.client_info.version,
        }
    }
}
