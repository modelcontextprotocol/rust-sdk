use std::collections::BTreeMap;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json;
use rmcp::model::{ClientCapabilities, RootsCapabilities};

#[napi(object)]
pub struct JsPromptsCapability {
    pub list_changed: Option<bool>,
}

#[napi(object)]
pub struct JsResourcesCapability {
    pub subscribe: Option<bool>,
    pub list_changed: Option<bool>,
}

#[napi(object)]
pub struct JsToolsCapability {
    pub list_changed: Option<bool>,
}

#[napi]
#[derive(Default, Clone, Debug, PartialEq)]
pub struct JsRootsCapabilities {
    pub list_changed: Option<bool>,
}

#[napi]
impl JsRootsCapabilities {
    /// Construct a new roots capabilities object.
    ///
    /// > **Note:** Any extra parameters (such as environment/context) are injected by napi and should NOT be passed by the TypeScript user.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const roots = new JsRootsCapabilities();
    /// ```
    #[napi(constructor)]
    pub fn new() -> JsRootsCapabilities {
        JsRootsCapabilities {
            list_changed: None,
        }
    }

    pub fn to_rust(&self) -> RootsCapabilities {
        RootsCapabilities {
            list_changed: self.list_changed,
        }
    }
    pub fn from_rust(core: &RootsCapabilities) -> JsRootsCapabilities {
        JsRootsCapabilities {
            list_changed: core.list_changed,
        }
    }
}

impl FromNapiValue for JsRootsCapabilities {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
        unsafe {
            let obj = Object::from_napi_value(env, napi_val)?;
            let list_changed = obj.get("list_changed")?;
            Ok(JsRootsCapabilities { list_changed })
        }
    }
}

#[napi]
#[derive(Default, Clone, Debug, PartialEq)]
pub struct JsExperimentalCapabilities {
    #[napi(skip)]
    pub inner: BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
}

#[napi]
impl JsExperimentalCapabilities {
    /// Construct new experimental capabilities from a JSON object.
    ///
    /// > **Note:** Any extra parameters (such as environment/context) are injected by napi and should NOT be passed by the TypeScript user.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const experimental = JsExperimentalCapabilities.new({});
    /// ```
    #[napi(factory, ts_type = "Record<string, Record<string, any>>")]
    pub fn new(value: serde_json::Value) -> JsExperimentalCapabilities {
        let inner: BTreeMap<String, serde_json::Map<String, serde_json::Value>> = serde_json::from_value(value).unwrap_or_default();
        JsExperimentalCapabilities { inner }
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.inner).unwrap_or(serde_json::Value::Null)
    }
    pub fn from_js(value: serde_json::Value) -> JsExperimentalCapabilities {
        let inner: BTreeMap<String, serde_json::Map<String, serde_json::Value>> = serde_json::from_value(value).unwrap_or_default();
        JsExperimentalCapabilities { inner }
    }
}

impl JsExperimentalCapabilities {
    pub fn to_rust(&self) -> BTreeMap<String, serde_json::Map<String, serde_json::Value>> {
        self.inner.clone()
    }
    pub fn from_rust(core: &BTreeMap<String, serde_json::Map<String, serde_json::Value>>) -> JsExperimentalCapabilities {
        JsExperimentalCapabilities { inner: core.clone() }
    }
}

impl FromNapiValue for JsExperimentalCapabilities {
    unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> Result<Self> {
        unsafe {
            let value = serde_json::Value::from_napi_value(env, napi_val)?;
            Ok(Self::from_js(value))
        }
    }
}

#[napi]
#[derive(Clone, Debug, PartialEq)]
pub struct JsClientCapabilities {
    pub experimental: JsExperimentalCapabilities,
    pub roots: JsRootsCapabilities,
    pub sampling: Option<serde_json::Value>,
}

#[napi]
impl JsClientCapabilities {
    /// Construct a new client capabilities object.
    ///
    /// > **Note:** Any extra parameters (such as environment/context) are injected by napi and should NOT be passed by the TypeScript user.
    ///
    /// # Example (TypeScript)
    /// ```typescript
    /// const capabilities = new JsClientCapabilities(experimental, roots, null);
    /// ```
    #[napi(constructor)]
    pub fn new(
        experimental: JsExperimentalCapabilities,
        roots: JsRootsCapabilities,
        sampling: Option<serde_json::Value>,
    ) -> JsClientCapabilities {
        println!("JsClientCapabilities::new received experimental: {:?}", experimental);
        println!("JsClientCapabilities::new received roots: {:?}", roots);
        println!("JsClientCapabilities::new received sampling: {:?}", sampling);
        JsClientCapabilities { experimental, roots, sampling }
    }
}

impl JsClientCapabilities {
    pub fn to_rust(&self) -> ClientCapabilities {
        let sampling = match &self.sampling {
            Some(v) => serde_json::from_value(v.clone()).ok(),
            None => None,
        };
        ClientCapabilities {
            experimental: Some(self.experimental.to_rust()),
            roots: Some(self.roots.to_rust()),
            sampling,
        }
    }

    pub fn from_rust(core: &ClientCapabilities) -> JsClientCapabilities {
        JsClientCapabilities {
            experimental: core.experimental.as_ref().map(JsExperimentalCapabilities::from_rust).unwrap_or_default(),
            roots: core.roots.as_ref().map(JsRootsCapabilities::from_rust).unwrap_or_default(),
            sampling: core.sampling.as_ref().map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null)),
        }
    }
}
