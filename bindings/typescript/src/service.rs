// Service interface for rmcp_typescript binding
// Add #[wasm_bindgen] wrappers as needed for TS/JS

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json;
use rmcp::service::RoleClient;
use rmcp::service::Peer;
use std::sync::Arc;
use crate::model::{JsClientInfo, JsInfo};

#[napi]
#[derive(Clone)]
pub struct JsPeer {
    #[napi(skip)]
    pub inner: Option<Arc<Peer<RoleClient>>>,
}

#[napi]
impl JsPeer {
    #[napi(constructor)]
    pub fn new() -> JsPeer {
        JsPeer { inner: None }
    }

    pub fn set_inner(&mut self, inner: Arc<Peer<RoleClient>>) {
        self.inner = Some(inner);
    }

    #[napi]
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
    pub async fn call_tool(&self, name: String, arguments: Option<serde_json::Value>) -> napi::Result<serde_json::Value> {
        match &self.inner {
            Some(peer) => {
                let param = rmcp::model::CallToolRequestParam {
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
