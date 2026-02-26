use std::collections::BTreeMap;
#[cfg(any(feature = "server", feature = "macros"))]
use std::marker::PhantomData;

#[cfg(any(feature = "server", feature = "macros"))]
use pastey::paste;
use serde::{Deserialize, Serialize};

use super::JsonObject;
pub type ExperimentalCapabilities = BTreeMap<String, JsonObject>;

/// MCP extension capabilities map.
///
/// Keys are extension identifiers in the format `{vendor-prefix}/{extension-name}`
/// (e.g., `io.modelcontextprotocol/ui`, `io.modelcontextprotocol/oauth-client-credentials`).
/// Values are per-extension settings objects. An empty object indicates support with no settings.
///
/// # Example
///
/// ```rust
/// use rmcp::model::ExtensionCapabilities;
/// use serde_json::json;
///
/// let mut extensions = ExtensionCapabilities::new();
/// extensions.insert(
///     "io.modelcontextprotocol/ui".to_string(),
///     serde_json::from_value(json!({
///         "mimeTypes": ["text/html;profile=mcp-app"]
///     })).unwrap()
/// );
/// ```
pub type ExtensionCapabilities = BTreeMap<String, JsonObject>;

// ── Well-known extension identifiers ────────────────────────────────────────

/// Extension identifier for OAuth client credentials flow (SEP-1046).
///
/// Both clients and servers advertise this extension to opt into
/// machine-to-machine authentication via the OAuth 2.0 client credentials
/// grant.  The associated settings object may contain:
///
/// - `grant_types_supported` – list of grant types (e.g. `["client_credentials"]`)
/// - `token_endpoint_auth_methods_supported` – list of auth methods
///   (e.g. `["client_secret_basic", "private_key_jwt"]`)
///
/// An empty settings object (`{}`) indicates support with default settings.
pub const EXTENSION_OAUTH_CLIENT_CREDENTIALS: &str =
    "io.modelcontextprotocol/oauth-client-credentials";

/// Settings for the OAuth client credentials extension (SEP-1046).
///
/// This struct provides typed access to the per-extension settings that
/// accompany the `io.modelcontextprotocol/oauth-client-credentials`
/// extension key in [`ExtensionCapabilities`].
///
/// # Example
///
/// ```rust
/// use rmcp::model::OAuthClientCredentialsSettings;
///
/// let settings = OAuthClientCredentialsSettings {
///     grant_types_supported: Some(vec!["client_credentials".into()]),
///     token_endpoint_auth_methods_supported: Some(vec![
///         "client_secret_basic".into(),
///         "private_key_jwt".into(),
///     ]),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OAuthClientCredentialsSettings {
    /// Supported grant types (e.g. `["client_credentials"]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_types_supported: Option<Vec<String>>,
    /// Supported token-endpoint authentication methods
    /// (e.g. `["client_secret_basic", "private_key_jwt"]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PromptsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ResourcesCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ToolsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RootsCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Task capabilities shared by client and server.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TasksCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<TaskRequestsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel: Option<JsonObject>,
}

/// Request types that support task-augmented execution.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TaskRequestsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingTaskCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<ElicitationTaskCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsTaskCapability>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SamplingTaskCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_message: Option<JsonObject>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ElicitationTaskCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<JsonObject>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ToolsTaskCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<JsonObject>,
}

impl TasksCapability {
    /// Default client tasks capability with sampling and elicitation support.
    pub fn client_default() -> Self {
        Self {
            list: Some(JsonObject::new()),
            cancel: Some(JsonObject::new()),
            requests: Some(TaskRequestsCapability {
                sampling: Some(SamplingTaskCapability {
                    create_message: Some(JsonObject::new()),
                }),
                elicitation: Some(ElicitationTaskCapability {
                    create: Some(JsonObject::new()),
                }),
                tools: None,
            }),
        }
    }

    /// Default server tasks capability with tools/call support.
    pub fn server_default() -> Self {
        Self {
            list: Some(JsonObject::new()),
            cancel: Some(JsonObject::new()),
            requests: Some(TaskRequestsCapability {
                sampling: None,
                elicitation: None,
                tools: Some(ToolsTaskCapability {
                    call: Some(JsonObject::new()),
                }),
            }),
        }
    }

    pub fn supports_list(&self) -> bool {
        self.list.is_some()
    }

    pub fn supports_cancel(&self) -> bool {
        self.cancel.is_some()
    }

    pub fn supports_tools_call(&self) -> bool {
        self.requests
            .as_ref()
            .and_then(|r| r.tools.as_ref())
            .and_then(|t| t.call.as_ref())
            .is_some()
    }

    pub fn supports_sampling_create_message(&self) -> bool {
        self.requests
            .as_ref()
            .and_then(|r| r.sampling.as_ref())
            .and_then(|s| s.create_message.as_ref())
            .is_some()
    }

    pub fn supports_elicitation_create(&self) -> bool {
        self.requests
            .as_ref()
            .and_then(|r| r.elicitation.as_ref())
            .and_then(|e| e.create.as_ref())
            .is_some()
    }
}

/// Capability for handling elicitation requests from servers.
/// Elicitation allows servers to request interactive input from users during tool execution.
/// This capability indicates that a client can handle elicitation requests and present
/// appropriate UI to users for collecting the requested information.
///
/// Capability for form mode elicitation.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct FormElicitationCapability {
    /// Whether the client supports JSON Schema validation for elicitation responses.
    /// When true, the client will validate user input against the requested_schema
    /// before sending the response back to the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_validation: Option<bool>,
}

/// Capability for URL mode elicitation.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct UrlElicitationCapability {}

/// Elicitation allows servers to request interactive input from users during tool execution.
/// This capability indicates that a client can handle elicitation requests and present
/// appropriate UI to users for collecting the requested information.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ElicitationCapability {
    /// Whether client supports form-based elicitation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form: Option<FormElicitationCapability>,
    /// Whether client supports URL-based elicitation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<UrlElicitationCapability>,
}

/// Sampling capability with optional sub-capabilities (SEP-1577).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct SamplingCapability {
    /// Support for `tools` and `toolChoice` parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<JsonObject>,
    /// Support for `includeContext` (soft-deprecated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<JsonObject>,
}

///
/// # Builder
/// ```rust
/// # use rmcp::model::ClientCapabilities;
/// let cap = ClientCapabilities::builder()
///     .enable_experimental()
///     .enable_roots()
///     .enable_roots_list_changed()
///     .build();
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<ExperimentalCapabilities>,
    /// Optional MCP extensions that the client supports (SEP-1724).
    /// Keys are extension identifiers (e.g., `"io.modelcontextprotocol/ui"`),
    /// values are per-extension settings objects. An empty object indicates
    /// support with no settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapabilities>,
    /// Capability for LLM sampling requests (SEP-1577)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,
    /// Capability to handle elicitation requests from servers for interactive user input
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<ElicitationCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<TasksCapability>,
}

impl ClientCapabilities {
    /// Returns `true` if the client advertises support for the OAuth client
    /// credentials extension (SEP-1046).
    pub fn supports_oauth_client_credentials(&self) -> bool {
        self.extensions.as_ref().map_or(false, |ext| {
            ext.contains_key(EXTENSION_OAUTH_CLIENT_CREDENTIALS)
        })
    }

    /// Returns the typed [`OAuthClientCredentialsSettings`] advertised by the
    /// client, or `None` if the extension is not present or cannot be parsed.
    pub fn oauth_client_credentials_settings(&self) -> Option<OAuthClientCredentialsSettings> {
        let raw = self
            .extensions
            .as_ref()?
            .get(EXTENSION_OAUTH_CLIENT_CREDENTIALS)?;
        serde_json::from_value(serde_json::to_value(raw).ok()?).ok()
    }
}

///
/// ## Builder
/// ```rust
/// # use rmcp::model::ServerCapabilities;
/// let cap = ServerCapabilities::builder()
///     .enable_logging()
///     .enable_experimental()
///     .enable_prompts()
///     .enable_resources()
///     .enable_tools()
///     .enable_tool_list_changed()
///     .build();
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<ExperimentalCapabilities>,
    /// Optional MCP extensions that the server supports (SEP-1724).
    /// Keys are extension identifiers (e.g., `"io.modelcontextprotocol/apps"`),
    /// values are per-extension settings objects. An empty object indicates
    /// support with no settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completions: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<TasksCapability>,
}

impl ServerCapabilities {
    /// Returns `true` if the server advertises support for the OAuth client
    /// credentials extension (SEP-1046).
    pub fn supports_oauth_client_credentials(&self) -> bool {
        self.extensions.as_ref().map_or(false, |ext| {
            ext.contains_key(EXTENSION_OAUTH_CLIENT_CREDENTIALS)
        })
    }

    /// Returns the typed [`OAuthClientCredentialsSettings`] advertised by the
    /// server, or `None` if the extension is not present or cannot be parsed.
    pub fn oauth_client_credentials_settings(&self) -> Option<OAuthClientCredentialsSettings> {
        let raw = self
            .extensions
            .as_ref()?
            .get(EXTENSION_OAUTH_CLIENT_CREDENTIALS)?;
        serde_json::from_value(serde_json::to_value(raw).ok()?).ok()
    }
}

#[cfg(any(feature = "server", feature = "macros"))]
macro_rules! builder {
    ($Target: ident {$($f: ident: $T: ty),* $(,)?}) => {
        paste! {
            #[derive(Default, Clone, Copy, Debug)]
            pub struct [<$Target BuilderState>]<
                $(const [<$f:upper>]: bool = false,)*
            >;
            #[derive(Debug, Default)]
            pub struct [<$Target Builder>]<S = [<$Target BuilderState>]> {
                $(pub $f: Option<$T>,)*
                pub state: PhantomData<S>
            }
            impl $Target {
                #[doc = "Create a new [`" $Target "`] builder."]
                pub fn builder() -> [<$Target Builder>] {
                    <[<$Target Builder>]>::default()
                }
            }
            impl<S> [<$Target Builder>]<S> {
                pub fn build(self) -> $Target {
                    $Target {
                        $( $f: self.$f, )*
                    }
                }
            }
            impl<S> From<[<$Target Builder>]<S>> for $Target {
                fn from(builder: [<$Target Builder>]<S>) -> Self {
                    builder.build()
                }
            }
        }
        builder!($Target @toggle $($f: $T,) *);

    };
    ($Target: ident @toggle $f0: ident: $T0: ty, $($f: ident: $T: ty,)*) => {
        builder!($Target @toggle [][$f0: $T0][$($f: $T,)*]);
    };
    ($Target: ident @toggle [$($ff: ident: $Tf: ty,)*][$fn: ident: $TN: ty][$fn_1: ident: $Tn_1: ty, $($ft: ident: $Tt: ty,)*]) => {
        builder!($Target @impl_toggle [$($ff: $Tf,)*][$fn: $TN][$fn_1: $Tn_1, $($ft:$Tt,)*]);
        builder!($Target @toggle [$($ff: $Tf,)* $fn: $TN,][$fn_1: $Tn_1][$($ft:$Tt,)*]);
    };
    ($Target: ident @toggle [$($ff: ident: $Tf: ty,)*][$fn: ident: $TN: ty][]) => {
        builder!($Target @impl_toggle [$($ff: $Tf,)*][$fn: $TN][]);
    };
    ($Target: ident @impl_toggle [$($ff: ident: $Tf: ty,)*][$fn: ident: $TN: ty][$($ft: ident: $Tt: ty,)*]) => {
        paste! {
            impl<
                $(const [<$ff:upper>]: bool,)*
                $(const [<$ft:upper>]: bool,)*
            > [<$Target Builder>]<[<$Target BuilderState>]<
                $([<$ff:upper>],)*
                false,
                $([<$ft:upper>],)*
            >> {
                pub fn [<enable_ $fn>](self) -> [<$Target Builder>]<[<$Target BuilderState>]<
                    $([<$ff:upper>],)*
                    true,
                    $([<$ft:upper>],)*
                >> {
                    [<$Target Builder>] {
                        $( $ff: self.$ff, )*
                        $fn: Some($TN::default()),
                        $( $ft: self.$ft, )*
                        state: PhantomData
                    }
                }
                pub fn [<enable_ $fn _with>](self, $fn: $TN) -> [<$Target Builder>]<[<$Target BuilderState>]<
                    $([<$ff:upper>],)*
                    true,
                    $([<$ft:upper>],)*
                >> {
                    [<$Target Builder>] {
                        $( $ff: self.$ff, )*
                        $fn: Some($fn),
                        $( $ft: self.$ft, )*
                        state: PhantomData
                    }
                }
            }
            // do we really need to disable some thing in builder?
            // impl<
            //     $(const [<$ff:upper>]: bool,)*
            //     $(const [<$ft:upper>]: bool,)*
            // > [<$Target Builder>]<[<$Target BuilderState>]<
            //     $([<$ff:upper>],)*
            //     true,
            //     $([<$ft:upper>],)*
            // >> {
            //     pub fn [<disable_ $fn>](self) -> [<$Target Builder>]<[<$Target BuilderState>]<
            //         $([<$ff:upper>],)*
            //         false,
            //         $([<$ft:upper>],)*
            //     >> {
            //         [<$Target Builder>] {
            //             $( $ff: self.$ff, )*
            //             $fn: None,
            //             $( $ft: self.$ft, )*
            //             state: PhantomData
            //         }
            //     }
            // }
        }
    }
}

#[cfg(any(feature = "server", feature = "macros"))]
builder! {
    ServerCapabilities {
        experimental: ExperimentalCapabilities,
        extensions: ExtensionCapabilities,
        logging: JsonObject,
        completions: JsonObject,
        prompts: PromptsCapability,
        resources: ResourcesCapability,
        tools: ToolsCapability,
        tasks: TasksCapability
    }
}

#[cfg(any(feature = "server", feature = "macros"))]
impl<
    const E: bool,
    const EXT: bool,
    const L: bool,
    const C: bool,
    const P: bool,
    const R: bool,
    const TASKS: bool,
> ServerCapabilitiesBuilder<ServerCapabilitiesBuilderState<E, EXT, L, C, P, R, true, TASKS>>
{
    pub fn enable_tool_list_changed(mut self) -> Self {
        if let Some(c) = self.tools.as_mut() {
            c.list_changed = Some(true);
        }
        self
    }
}

#[cfg(any(feature = "server", feature = "macros"))]
impl<
    const E: bool,
    const EXT: bool,
    const L: bool,
    const C: bool,
    const R: bool,
    const T: bool,
    const TASKS: bool,
> ServerCapabilitiesBuilder<ServerCapabilitiesBuilderState<E, EXT, L, C, true, R, T, TASKS>>
{
    pub fn enable_prompts_list_changed(mut self) -> Self {
        if let Some(c) = self.prompts.as_mut() {
            c.list_changed = Some(true);
        }
        self
    }
}

#[cfg(any(feature = "server", feature = "macros"))]
impl<
    const E: bool,
    const EXT: bool,
    const L: bool,
    const C: bool,
    const P: bool,
    const T: bool,
    const TASKS: bool,
> ServerCapabilitiesBuilder<ServerCapabilitiesBuilderState<E, EXT, L, C, P, true, T, TASKS>>
{
    pub fn enable_resources_list_changed(mut self) -> Self {
        if let Some(c) = self.resources.as_mut() {
            c.list_changed = Some(true);
        }
        self
    }

    pub fn enable_resources_subscribe(mut self) -> Self {
        if let Some(c) = self.resources.as_mut() {
            c.subscribe = Some(true);
        }
        self
    }
}

#[cfg(any(feature = "server", feature = "macros"))]
builder! {
    ClientCapabilities{
        experimental: ExperimentalCapabilities,
        extensions: ExtensionCapabilities,
        roots: RootsCapabilities,
        sampling: SamplingCapability,
        elicitation: ElicitationCapability,
        tasks: TasksCapability,
    }
}

#[cfg(any(feature = "server", feature = "macros"))]
impl<const E: bool, const EXT: bool, const S: bool, const EL: bool, const TASKS: bool>
    ClientCapabilitiesBuilder<ClientCapabilitiesBuilderState<E, EXT, true, S, EL, TASKS>>
{
    pub fn enable_roots_list_changed(mut self) -> Self {
        if let Some(c) = self.roots.as_mut() {
            c.list_changed = Some(true);
        }
        self
    }
}

#[cfg(any(feature = "server", feature = "macros"))]
impl<const E: bool, const EXT: bool, const R: bool, const EL: bool, const TASKS: bool>
    ClientCapabilitiesBuilder<ClientCapabilitiesBuilderState<E, EXT, R, true, EL, TASKS>>
{
    /// Enable tool calling in sampling requests
    pub fn enable_sampling_tools(mut self) -> Self {
        if let Some(c) = self.sampling.as_mut() {
            c.tools = Some(JsonObject::default());
        }
        self
    }

    /// Enable context inclusion in sampling (soft-deprecated)
    pub fn enable_sampling_context(mut self) -> Self {
        if let Some(c) = self.sampling.as_mut() {
            c.context = Some(JsonObject::default());
        }
        self
    }
}

#[cfg(all(feature = "elicitation", any(feature = "server", feature = "macros")))]
impl<const E: bool, const EXT: bool, const R: bool, const S: bool, const TASKS: bool>
    ClientCapabilitiesBuilder<ClientCapabilitiesBuilderState<E, EXT, R, S, true, TASKS>>
{
    /// Enable JSON Schema validation for elicitation responses in form mode.
    /// When enabled, the client will validate user input against the requested_schema
    /// before sending responses back to the server.
    pub fn enable_elicitation_schema_validation(mut self) -> Self {
        if let Some(c) = self.elicitation.as_mut() {
            c.form = Some(FormElicitationCapability {
                schema_validation: Some(true),
            });
        }
        self
    }
}

#[cfg(test)]
#[cfg(any(feature = "server", feature = "macros"))]
mod test {
    use super::*;
    #[test]
    fn test_builder() {
        let builder = <ServerCapabilitiesBuilder>::default()
            .enable_logging()
            .enable_experimental()
            .enable_prompts()
            .enable_resources()
            .enable_tools()
            .enable_tool_list_changed();
        assert_eq!(builder.logging, Some(JsonObject::default()));
        assert_eq!(builder.prompts, Some(PromptsCapability::default()));
        assert_eq!(builder.resources, Some(ResourcesCapability::default()));
        assert_eq!(
            builder.tools,
            Some(ToolsCapability {
                list_changed: Some(true),
            })
        );
        assert_eq!(
            builder.experimental,
            Some(ExperimentalCapabilities::default())
        );
        let client_builder = <ClientCapabilitiesBuilder>::default()
            .enable_experimental()
            .enable_roots()
            .enable_roots_list_changed()
            .enable_sampling();
        assert_eq!(
            client_builder.experimental,
            Some(ExperimentalCapabilities::default())
        );
        assert_eq!(
            client_builder.roots,
            Some(RootsCapabilities {
                list_changed: Some(true),
            })
        );
    }

    #[test]
    fn test_task_capabilities_deserialization() {
        // Test deserializing from the MCP spec format
        let json = serde_json::json!({
            "list": {},
            "cancel": {},
            "requests": {
                "tools": { "call": {} }
            }
        });

        let tasks: TasksCapability = serde_json::from_value(json).unwrap();
        assert!(tasks.list.is_some());
        assert!(tasks.cancel.is_some());
        assert!(tasks.requests.is_some());
        let requests = tasks.requests.unwrap();
        assert!(requests.tools.is_some());
        assert!(requests.tools.unwrap().call.is_some());
    }

    #[test]
    fn test_tasks_capability_client_default() {
        let tasks = TasksCapability::client_default();

        // Verify structure
        assert!(tasks.supports_list());
        assert!(tasks.supports_cancel());
        assert!(tasks.supports_sampling_create_message());
        assert!(tasks.supports_elicitation_create());
        assert!(!tasks.supports_tools_call());

        // Verify serialization matches expected format
        let json = serde_json::to_value(&tasks).unwrap();
        assert_eq!(json["list"], serde_json::json!({}));
        assert_eq!(json["cancel"], serde_json::json!({}));
        assert_eq!(
            json["requests"]["sampling"]["createMessage"],
            serde_json::json!({})
        );
        assert_eq!(
            json["requests"]["elicitation"]["create"],
            serde_json::json!({})
        );
    }

    #[test]
    fn test_tasks_capability_server_default() {
        let tasks = TasksCapability::server_default();

        // Verify structure
        assert!(tasks.supports_list());
        assert!(tasks.supports_cancel());
        assert!(tasks.supports_tools_call());
        assert!(!tasks.supports_sampling_create_message());
        assert!(!tasks.supports_elicitation_create());

        // Verify serialization matches expected format
        let json = serde_json::to_value(&tasks).unwrap();
        assert_eq!(json["list"], serde_json::json!({}));
        assert_eq!(json["cancel"], serde_json::json!({}));
        assert_eq!(json["requests"]["tools"]["call"], serde_json::json!({}));
    }

    #[test]
    fn test_client_extensions_capability() {
        // Test building ClientCapabilities with extensions (MCP Apps support)
        let mut extensions = ExtensionCapabilities::new();
        extensions.insert(
            "io.modelcontextprotocol/ui".to_string(),
            serde_json::from_value(serde_json::json!({
                "mimeTypes": ["text/html;profile=mcp-app"]
            }))
            .unwrap(),
        );

        let capabilities = ClientCapabilities::builder()
            .enable_extensions_with(extensions)
            .enable_sampling()
            .build();

        // Verify serialization matches MCP Apps spec format
        let json = serde_json::to_value(&capabilities).unwrap();
        assert_eq!(
            json["extensions"]["io.modelcontextprotocol/ui"]["mimeTypes"],
            serde_json::json!(["text/html;profile=mcp-app"])
        );
        assert!(json["sampling"].is_object());
    }

    #[test]
    fn test_server_extensions_capability() {
        // Test building ServerCapabilities with extensions
        let mut extensions = ExtensionCapabilities::new();
        extensions.insert(
            "io.modelcontextprotocol/apps".to_string(),
            serde_json::from_value(serde_json::json!({})).unwrap(),
        );

        let capabilities = ServerCapabilities::builder()
            .enable_extensions_with(extensions)
            .enable_tools()
            .build();

        // Verify serialization
        let json = serde_json::to_value(&capabilities).unwrap();
        assert!(json["extensions"]["io.modelcontextprotocol/apps"].is_object());
        assert!(json["tools"].is_object());
    }

    #[test]
    fn test_extensions_deserialization() {
        // Test deserializing capabilities with extensions from JSON
        let json = serde_json::json!({
            "extensions": {
                "io.modelcontextprotocol/ui": {
                    "mimeTypes": ["text/html;profile=mcp-app"]
                }
            },
            "sampling": {}
        });

        let capabilities: ClientCapabilities = serde_json::from_value(json).unwrap();
        assert!(capabilities.extensions.is_some());
        let extensions = capabilities.extensions.unwrap();
        assert!(extensions.contains_key("io.modelcontextprotocol/ui"));
        let ui_ext = extensions.get("io.modelcontextprotocol/ui").unwrap();
        assert!(ui_ext.contains_key("mimeTypes"));
    }

    #[test]
    fn test_extensions_empty_settings() {
        // Test that empty extension settings work (indicates support with no settings)
        let mut extensions = ExtensionCapabilities::new();
        extensions.insert(
            "io.modelcontextprotocol/oauth-client-credentials".to_string(),
            JsonObject::new(),
        );

        let capabilities = ClientCapabilities::builder()
            .enable_extensions_with(extensions)
            .build();

        let json = serde_json::to_value(&capabilities).unwrap();
        assert_eq!(
            json["extensions"]["io.modelcontextprotocol/oauth-client-credentials"],
            serde_json::json!({})
        );
    }

    // ── OAuth client credentials capability advertisement tests (SEP-1046) ──

    #[test]
    fn test_client_supports_oauth_client_credentials_false_by_default() {
        let capabilities = ClientCapabilities::default();
        assert!(!capabilities.supports_oauth_client_credentials());
        assert!(capabilities.oauth_client_credentials_settings().is_none());
    }

    #[test]
    fn test_client_supports_oauth_client_credentials_empty_settings() {
        let mut extensions = ExtensionCapabilities::new();
        extensions.insert(
            EXTENSION_OAUTH_CLIENT_CREDENTIALS.to_string(),
            JsonObject::new(),
        );
        let capabilities = ClientCapabilities::builder()
            .enable_extensions_with(extensions)
            .build();

        assert!(capabilities.supports_oauth_client_credentials());
        // Empty settings should deserialize to the default
        let settings = capabilities.oauth_client_credentials_settings().unwrap();
        assert_eq!(settings, OAuthClientCredentialsSettings::default());
    }

    #[test]
    fn test_client_supports_oauth_client_credentials_with_settings() {
        let mut extensions = ExtensionCapabilities::new();
        extensions.insert(
            EXTENSION_OAUTH_CLIENT_CREDENTIALS.to_string(),
            serde_json::from_value(serde_json::json!({
                "grant_types_supported": ["client_credentials"],
                "token_endpoint_auth_methods_supported": ["client_secret_basic", "private_key_jwt"]
            }))
            .unwrap(),
        );
        let capabilities = ClientCapabilities::builder()
            .enable_extensions_with(extensions)
            .build();

        assert!(capabilities.supports_oauth_client_credentials());
        let settings = capabilities.oauth_client_credentials_settings().unwrap();
        assert_eq!(
            settings.grant_types_supported,
            Some(vec!["client_credentials".to_string()])
        );
        assert_eq!(
            settings.token_endpoint_auth_methods_supported,
            Some(vec![
                "client_secret_basic".to_string(),
                "private_key_jwt".to_string(),
            ])
        );
    }

    #[test]
    fn test_server_supports_oauth_client_credentials_false_by_default() {
        let capabilities = ServerCapabilities::default();
        assert!(!capabilities.supports_oauth_client_credentials());
        assert!(capabilities.oauth_client_credentials_settings().is_none());
    }

    #[test]
    fn test_server_supports_oauth_client_credentials_empty_settings() {
        let mut extensions = ExtensionCapabilities::new();
        extensions.insert(
            EXTENSION_OAUTH_CLIENT_CREDENTIALS.to_string(),
            JsonObject::new(),
        );
        let capabilities = ServerCapabilities::builder()
            .enable_extensions_with(extensions)
            .build();

        assert!(capabilities.supports_oauth_client_credentials());
        let settings = capabilities.oauth_client_credentials_settings().unwrap();
        assert_eq!(settings, OAuthClientCredentialsSettings::default());
    }

    #[test]
    fn test_server_supports_oauth_client_credentials_with_settings() {
        let mut extensions = ExtensionCapabilities::new();
        extensions.insert(
            EXTENSION_OAUTH_CLIENT_CREDENTIALS.to_string(),
            serde_json::from_value(serde_json::json!({
                "grant_types_supported": ["client_credentials"],
                "token_endpoint_auth_methods_supported": ["private_key_jwt"]
            }))
            .unwrap(),
        );
        let capabilities = ServerCapabilities::builder()
            .enable_extensions_with(extensions)
            .enable_tools()
            .build();

        assert!(capabilities.supports_oauth_client_credentials());
        let settings = capabilities.oauth_client_credentials_settings().unwrap();
        assert_eq!(
            settings.grant_types_supported,
            Some(vec!["client_credentials".to_string()])
        );
        assert_eq!(
            settings.token_endpoint_auth_methods_supported,
            Some(vec!["private_key_jwt".to_string()])
        );
        // Other capabilities should still work
        assert!(capabilities.tools.is_some());
    }

    #[test]
    fn test_oauth_client_credentials_settings_roundtrip() {
        let settings = OAuthClientCredentialsSettings {
            grant_types_supported: Some(vec!["client_credentials".into()]),
            token_endpoint_auth_methods_supported: Some(vec![
                "client_secret_basic".into(),
                "private_key_jwt".into(),
            ]),
        };
        let json = serde_json::to_value(&settings).unwrap();
        let deserialized: OAuthClientCredentialsSettings =
            serde_json::from_value(json.clone()).unwrap();
        assert_eq!(settings, deserialized);

        // Verify JSON shape
        assert_eq!(
            json["grant_types_supported"],
            serde_json::json!(["client_credentials"])
        );
        assert_eq!(
            json["token_endpoint_auth_methods_supported"],
            serde_json::json!(["client_secret_basic", "private_key_jwt"])
        );
    }

    #[test]
    fn test_oauth_client_credentials_settings_default_omits_none() {
        let settings = OAuthClientCredentialsSettings::default();
        let json = serde_json::to_value(&settings).unwrap();
        // Default settings should serialize to empty object (skip_serializing_if)
        assert_eq!(json, serde_json::json!({}));
    }

    #[test]
    fn test_extension_constant_value() {
        assert_eq!(
            EXTENSION_OAUTH_CLIENT_CREDENTIALS,
            "io.modelcontextprotocol/oauth-client-credentials"
        );
    }
}
