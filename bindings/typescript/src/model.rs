pub mod tool;
pub mod resource;
pub mod prompt;
pub mod prompt_message;
pub mod capabilities;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use crate::model::capabilities::JsClientCapabilities;
use serde_json;


#[napi]
#[derive(Clone)]
pub struct JsClientInfo {
    #[napi(skip)]
    pub inner: rmcp::model::ClientInfo,
}

#[napi]
impl JsClientInfo {
    #[napi(constructor)]
    pub fn new(protocol_version: String, capabilities: Reference<JsClientCapabilities>, client_info: Reference<JsImplementation>, env: Env) -> napi::Result<JsClientInfo> {
        let protocol_version = serde_json::from_str(&format!("\"{}\"", protocol_version))
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let capabilities = capabilities.clone(env)?.to_rust();
        let client_info = client_info.clone(env)?.to_rust();

        let inner = rmcp::model::ClientInfo {
            protocol_version,
            capabilities,
            client_info,
        };
        Ok(JsClientInfo { inner })
    }

    #[napi(getter)]
    pub fn protocol_version(&self) -> String {
        format!("{:?}", self.inner.protocol_version)
    }

    #[napi(getter)]
    pub fn capabilities(&self) -> JsClientCapabilities {
        JsClientCapabilities::from_rust(&self.inner.capabilities)
    }

    #[napi(getter)]
    pub fn client_info(&self) -> JsImplementation {
        JsImplementation::from_rust(&self.inner.client_info)
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
}

#[napi]
#[derive(Clone, Debug, PartialEq)]
pub struct JsImplementation {
    pub name: String,
    pub version: String,
}

#[napi]
impl JsImplementation {
    #[napi(constructor)]
    pub fn new(name: String, version: String) -> JsImplementation {
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

#[napi]
#[derive(Clone)]
pub struct ProtocolVersion {
    pub value: String,
}

#[napi]
impl ProtocolVersion {
    #[napi(constructor)]
    pub fn new(value: String) -> ProtocolVersion {
        ProtocolVersion { value }
    }

    #[napi(getter)]
    pub fn value(&self) -> String {
        self.value.clone()
    }

    #[napi(js_name = "toString")]
    pub fn to_string(&self) -> String {
        self.value.clone()
    }

    #[napi]
    pub fn latest() -> ProtocolVersion {
        ProtocolVersion { value: "2.0".to_string() }
    }
}

#[napi]
pub fn json_rpc_version_2_0() -> String {
    "2.0".to_string()
}

#[napi]
#[derive(Clone)]
pub struct JsInfo {
    pub protocol_version: String,
    pub name: String,
    pub version: String,
}

#[napi]
impl JsInfo {
    #[napi(constructor)]
    pub fn new(protocol_version: String, name: String, version: String) -> JsInfo {
        JsInfo {
            protocol_version,
            name,
            version,
        }
    }

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
