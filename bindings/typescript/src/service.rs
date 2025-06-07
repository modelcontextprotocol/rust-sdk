//! Service interface for rmcp_typescript binding.
//!
//! This module provides the main service interface (`JsClient`) for interacting with the RMCP SDK from TypeScript/JavaScript.
//! It exposes methods for retrieving server info, listing tools, and invoking tools, and is designed for use with Node.js via `napi` bindings.
//!
//! # Example (TypeScript)
//!
//! ```typescript
//! import { JsClient } from 'rmcp-typescript';
//! const client = new JsClient();
//! // await client.listAllTools();
//! // await client.callTool('toolName', { ... });
//! ```


use napi_derive::napi;
use serde_json;
use rmcp::service::RoleClient;
use rmcp::service::Peer;
use std::sync::Arc;
use crate::model::{JsInfo};
use rmcp::model::CallToolRequestParam;

/// Main client object for interacting with RMCP services from TypeScript/JavaScript.
///
/// Provides methods for retrieving server info, listing tools, and invoking tools.
#[napi]
#[derive(Clone)]
pub struct JsClient {
    #[napi(skip)]
    pub inner: Option<Arc<Peer<RoleClient>>>,
}

#[napi]
impl JsClient {
    #[napi(constructor)]
    pub fn new() -> JsClient {
        JsClient { inner: None }
    }

    /// Set the internal peer for this client (used internally after connecting).
    pub fn set_inner(&mut self, inner: Arc<Peer<RoleClient>>) {
        self.inner = Some(inner);
    }

    #[napi]
    /// Retrieve information about the connected server.
    ///
    /// > **Note:** Any extra parameters (such as environment/context) are injected by napi and should NOT be passed by the TypeScript user.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const info = await client.peerInfo();
    /// console.log(info.name, info.version);
    /// ```
    pub fn peer_info(&self) -> napi::Result<JsInfo> {
        match &self.inner {
            Some(peer) => {
                let info = peer.peer_info();
                Ok(JsInfo::server((*info).clone()))
            }
            None => Err(napi::Error::from_reason("Peer not initialized")),
        }
    }

    #[napi]
    /// List all available tools on the server.
    ///
    /// > **Note:** Any extra parameters (such as environment/context) are injected by napi and should NOT be passed by the TypeScript user.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const tools = await client.listAllTools();
    /// console.log(tools);
    /// ```
    pub async fn list_all_tools(&self) -> napi::Result<serde_json::Value> {
        match &self.inner {
            Some(peer) => {
                match peer.list_all_tools().await {
                    Ok(tools) => {
                        let tools_json = serde_json::to_value(tools)
                            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                        Ok(tools_json)
                    }
                    Err(e) => Err(napi::Error::from_reason(e.to_string())),
                }
            }
            None => Err(napi::Error::from_reason("Peer not initialized")),
        }
    }

    #[napi]
    /// Invoke a tool by name on the server.
    ///
    /// # Arguments
    /// * `name` - The tool name to invoke.
    /// * `arguments` - Optional arguments for the tool as a JSON object.
    ///
    /// > **Note:** Any extra parameters (such as environment/context) are injected by napi and should NOT be passed by the TypeScript user.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const result = await client.callTool('increment', { value: 1 });
    /// console.log(result);
    /// ```
    pub async fn call_tool(&self, name: String, arguments: Option<serde_json::Value>) -> napi::Result<serde_json::Value> {
        match &self.inner {
            Some(peer) => {
                let param = CallToolRequestParam {
                    name: name.into(),
                    arguments: arguments.and_then(|v| v.as_object().cloned()),
                };
                match peer.call_tool(param).await {
                    Ok(result) => {
                        let result_json = serde_json::to_value(result)
                            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                        Ok(result_json)
                    }
                    Err(e) => Err(napi::Error::from_reason(e.to_string())),
                }
            }
            None => Err(napi::Error::from_reason("Peer not initialized")),
        }
    }
}
