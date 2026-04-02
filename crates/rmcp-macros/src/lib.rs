#![doc = include_str!("../README.md")]

#[allow(unused_imports)]
use proc_macro::TokenStream;

mod common;
mod prompt;
mod prompt_handler;
mod prompt_router;
mod task_handler;
mod tool;
mod tool_handler;
mod tool_router;
/// # tool
///
/// This macro is used to mark a function as a tool handler.
///
/// This will generate a function that return the attribute of this tool, with type `rmcp::model::Tool`.
///
/// ## Usage
///
/// | field             | type                       | usage |
/// | :-                | :-                         | :-    |
/// | `name`            | `String`                   | The name of the tool. If not provided, it defaults to the function name. |
/// | `description`     | `String`                   | A description of the tool. The document of this function will be used. |
/// | `input_schema`    | `Expr`                     | A JSON Schema object defining the expected parameters for the tool. If not provide, if will use the json schema of its argument with type `Parameters<T>` |
/// | `annotations`     | `ToolAnnotationsAttribute` | Additional tool information. Defaults to `None`. |
///
/// ## Example
///
/// ```rust,ignore
/// #[tool(name = "my_tool", description = "This is my tool", annotations(title = "我的工具", read_only_hint = true))]
/// pub async fn my_tool(param: Parameters<MyToolParam>) {
///     // handling tool request
/// }
/// ```
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, input: TokenStream) -> TokenStream {
    tool::tool(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// # tool_router
///
/// This macro is used to generate a tool router based on functions marked with `#[rmcp::tool]` in an implementation block.
///
/// It creates a function that returns a `ToolRouter` instance.
///
/// The generated function is used by `#[tool_handler]` by default (via `Self::tool_router()`),
/// so in most cases you do not need to store the router in a field.
///
/// ## Usage
///
/// | field     | type          | usage |
/// | :-        | :-            | :-    |
/// | `router`  | `Ident`       | The name of the router function to be generated. Defaults to `tool_router`. |
/// | `vis`     | `Visibility`  | The visibility of the generated router function. Defaults to empty. |
///
/// ## Example
///
/// ```rust,ignore
/// #[tool_router]
/// impl MyToolHandler {
///     #[tool]
///     pub fn my_tool() {
///
///     }
/// }
///
/// // #[tool_handler] calls Self::tool_router() automatically
/// #[tool_handler]
/// impl ServerHandler for MyToolHandler {}
/// ```
///
/// Or specify the visibility and router name, which would be helpful when you want to combine multiple routers into one:
///
/// ```rust,ignore
/// mod a {
///     #[tool_router(router = tool_router_a, vis = "pub")]
///     impl MyToolHandler {
///         #[tool]
///         fn my_tool_a() {
///             
///         }
///     }
/// }
///
/// mod b {
///     #[tool_router(router = tool_router_b, vis = "pub")]
///     impl MyToolHandler {
///         #[tool]
///         fn my_tool_b() {
///             
///         }
///     }
/// }
///
/// impl MyToolHandler {
///     fn new() -> Self {
///         Self {
///             tool_router: self::tool_router_a() + self::tool_router_b(),
///         }
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn tool_router(attr: TokenStream, input: TokenStream) -> TokenStream {
    tool_router::tool_router(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// # tool_handler
///
/// This macro generates the `call_tool`, `list_tools`, `get_tool`, and (optionally)
/// `get_info` methods for a `ServerHandler` implementation, using a `ToolRouter`.
///
/// ## Usage
///
/// | field          | type     | usage |
/// | :-             | :-       | :-    |
/// | `router`       | `Expr`   | The expression to access the `ToolRouter` instance. Defaults to `Self::tool_router()`. |
/// | `meta`         | `Expr`   | Optional metadata for `ListToolsResult`. |
/// | `name`         | `String` | Custom server name. Defaults to `CARGO_CRATE_NAME`. |
/// | `version`      | `String` | Custom server version. Defaults to `CARGO_PKG_VERSION`. |
/// | `instructions` | `String` | Optional human-readable instructions about using this server. |
///
/// ## Minimal example (no boilerplate)
///
/// The macro automatically generates `get_info()` with tools capability enabled
/// and reads the server name/version from `Cargo.toml`:
///
/// ```rust,ignore
/// struct TimeServer;
///
/// #[tool_router]
/// impl TimeServer {
///     #[tool(description = "Get current time")]
///     async fn get_time(&self) -> String { "12:00".into() }
/// }
///
/// #[tool_handler]
/// impl ServerHandler for TimeServer {}
/// ```
///
/// ## Custom server info
///
/// ```rust,ignore
/// #[tool_handler(name = "my-server", version = "1.0.0", instructions = "A helpful server")]
/// impl ServerHandler for MyToolHandler {}
/// ```
///
/// ## Custom router expression
///
/// ```rust,ignore
/// #[tool_handler(router = self.tool_router)]
/// impl ServerHandler for MyToolHandler {
///    // ...implement other handler
/// }
/// ```
///
/// ## Manual `get_info()`
///
/// If you provide your own `get_info()`, the macro will not generate one:
///
/// ```rust,ignore
/// #[tool_handler]
/// impl ServerHandler for MyToolHandler {
///     fn get_info(&self) -> ServerInfo {
///         ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn tool_handler(attr: TokenStream, input: TokenStream) -> TokenStream {
    tool_handler::tool_handler(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// # prompt
///
/// This macro is used to mark a function as a prompt handler.
///
/// This will generate a function that returns the attribute of this prompt, with type `rmcp::model::Prompt`.
///
/// ## Usage
///
/// | field             | type     | usage |
/// | :-                | :-       | :-    |
/// | `name`            | `String` | The name of the prompt. If not provided, it defaults to the function name. |
/// | `description`     | `String` | A description of the prompt. The document of this function will be used if not provided. |
/// | `arguments`       | `Expr`   | An expression that evaluates to `Option<Vec<PromptArgument>>` defining the prompt's arguments. If not provided, it will automatically generate arguments from the `Parameters<T>` type found in the function signature. |
///
/// ## Example
///
/// ```rust,ignore
/// #[prompt(name = "code_review", description = "Reviews code for best practices")]
/// pub async fn code_review_prompt(&self, Parameters(args): Parameters<CodeReviewArgs>) -> Result<Vec<PromptMessage>> {
///     // Generate prompt messages based on arguments
/// }
/// ```
#[proc_macro_attribute]
pub fn prompt(attr: TokenStream, input: TokenStream) -> TokenStream {
    prompt::prompt(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// # prompt_router
///
/// This macro generates a prompt router based on functions marked with `#[rmcp::prompt]` in an implementation block.
///
/// It creates a function that returns a `PromptRouter` instance.
///
/// ## Usage
///
/// | field     | type          | usage |
/// | :-        | :-            | :-    |
/// | `router`  | `Ident`       | The name of the router function to be generated. Defaults to `prompt_router`. |
/// | `vis`     | `Visibility`  | The visibility of the generated router function. Defaults to empty. |
///
/// ## Example
///
/// ```rust,ignore
/// #[prompt_router]
/// impl MyPromptHandler {
///     #[prompt]
///     pub async fn greeting_prompt(&self, Parameters(args): Parameters<GreetingArgs>) -> Result<Vec<PromptMessage>, Error> {
///         // Generate greeting prompt using args
///     }
///
///     pub fn new() -> Self {
///         Self {
///             // the default name of prompt router will be `prompt_router`
///             prompt_router: Self::prompt_router(),
///         }
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn prompt_router(attr: TokenStream, input: TokenStream) -> TokenStream {
    prompt_router::prompt_router(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// # prompt_handler
///
/// This macro generates handler methods for `get_prompt` and `list_prompts` in the
/// implementation block, using a `PromptRouter`. It also auto-generates `get_info()`
/// with prompts capability enabled if not already provided.
///
/// ## Usage
///
/// | field     | type   | usage |
/// | :-        | :-     | :-    |
/// | `router`  | `Expr` | The expression to access the `PromptRouter` instance. Defaults to `Self::prompt_router()`. |
/// | `meta`    | `Expr` | Optional metadata for `ListPromptsResult`. |
///
/// ## Example
/// ```rust,ignore
/// #[prompt_handler]
/// impl ServerHandler for MyPromptHandler {
///     // ...implement other handler methods
/// }
/// ```
///
/// or using a custom router expression:
/// ```rust,ignore
/// #[prompt_handler(router = self.prompt_router)]
/// impl ServerHandler for MyPromptHandler {
///    // ...implement other handler methods
/// }
/// ```
#[proc_macro_attribute]
pub fn prompt_handler(attr: TokenStream, input: TokenStream) -> TokenStream {
    prompt_handler::prompt_handler(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// # task_handler
///
/// Generates basic task-handling methods (`enqueue_task` and `list_tasks`) for a server handler
/// using a shared \[`OperationProcessor`\]. The default processor expression assumes a
/// `self.processor` field holding an `Arc<Mutex<OperationProcessor>>`, but it can be customized
/// via `#[task_handler(processor = ...)]`. Because the macro captures `self` inside spawned
/// futures, the handler type must implement [`Clone`].
#[proc_macro_attribute]
pub fn task_handler(attr: TokenStream, input: TokenStream) -> TokenStream {
    task_handler::task_handler(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
