use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Expr, ImplItem, ItemImpl};

use crate::common::{has_method, has_sibling_handler};

#[derive(FromMeta)]
#[darling(default)]
pub struct ToolHandlerAttribute {
    pub router: Expr,
    pub meta: Option<Expr>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub instructions: Option<String>,
}

impl Default for ToolHandlerAttribute {
    fn default() -> Self {
        Self {
            router: syn::parse2(quote! {
                Self::tool_router()
            })
            .unwrap(),
            meta: None,
            name: None,
            version: None,
            instructions: None,
        }
    }
}

pub fn tool_handler(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attr_args = NestedMeta::parse_meta_list(attr)?;
    let ToolHandlerAttribute {
        router,
        meta,
        name,
        version,
        instructions,
    } = ToolHandlerAttribute::from_list(&attr_args)?;
    let mut item_impl = syn::parse2::<ItemImpl>(input)?;

    if !has_method("call_tool", &item_impl) {
        let tool_call_fn = syn::parse2::<ImplItem>(quote! {
            async fn call_tool(
                &self,
                request: rmcp::model::CallToolRequestParams,
                context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
                let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
                #router.call(tcc).await
            }
        })?;
        item_impl.items.push(tool_call_fn);
    }

    let result_meta = if let Some(meta) = meta {
        quote! { Some(#meta) }
    } else {
        quote! { None }
    };

    if !has_method("list_tools", &item_impl) {
        let tool_list_fn = syn::parse2::<ImplItem>(quote! {
            async fn list_tools(
                &self,
                _request: Option<rmcp::model::PaginatedRequestParams>,
                _context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
                Ok(rmcp::model::ListToolsResult{
                    tools: #router.list_all(),
                    meta: #result_meta,
                    next_cursor: None,
                })
            }
        })?;
        item_impl.items.push(tool_list_fn);
    }

    if !has_method("get_tool", &item_impl) {
        let get_tool_fn = syn::parse2::<ImplItem>(quote! {
            fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
                #router.get(name).cloned()
            }
        })?;
        item_impl.items.push(get_tool_fn);
    }

    // Auto-generate get_info() if not already provided
    if !has_method("get_info", &item_impl) {
        let get_info_fn = build_get_info(
            &item_impl,
            name,
            version,
            instructions,
            CallerCapability::Tools,
        )?;
        item_impl.items.push(get_info_fn);
    }

    Ok(item_impl.into_token_stream())
}

/// Which handler macro is generating `get_info()`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallerCapability {
    Tools,
    Prompts,
    Tasks,
}

/// Build a `get_info()` method that returns `ServerInfo` with the appropriate capabilities.
///
/// The caller declares its own capability via `caller`. Sibling handler attributes
/// (`prompt_handler`, `task_handler`, `tool_handler`) are detected automatically
/// and their capabilities are included.
pub(crate) fn build_get_info(
    item_impl: &ItemImpl,
    name: Option<String>,
    version: Option<String>,
    instructions: Option<String>,
    caller: CallerCapability,
) -> syn::Result<ImplItem> {
    let has_tools =
        caller == CallerCapability::Tools || has_sibling_handler(item_impl, "tool_handler");
    let has_prompts =
        caller == CallerCapability::Prompts || has_sibling_handler(item_impl, "prompt_handler");
    let has_tasks =
        caller == CallerCapability::Tasks || has_sibling_handler(item_impl, "task_handler");

    let mut capability_calls = Vec::new();
    if has_tools {
        capability_calls.push(quote! { .enable_tools() });
    }
    if has_prompts {
        capability_calls.push(quote! { .enable_prompts() });
    }
    if has_tasks {
        capability_calls.push(quote! { .enable_tasks() });
    }

    let server_info_expr = match (name, version) {
        (Some(n), Some(v)) => quote! { rmcp::model::Implementation::new(#n, #v) },
        (Some(n), None) => {
            quote! { rmcp::model::Implementation::new(#n, env!("CARGO_PKG_VERSION")) }
        }
        (None, Some(v)) => {
            quote! { rmcp::model::Implementation::new(env!("CARGO_CRATE_NAME"), #v) }
        }
        (None, None) => quote! { rmcp::model::Implementation::from_build_env() },
    };

    let mut builder_calls = vec![quote! { .with_server_info(#server_info_expr) }];
    if let Some(i) = instructions {
        builder_calls.push(quote! { .with_instructions(#i.to_string()) });
    }

    syn::parse2::<ImplItem>(quote! {
        fn get_info(&self) -> rmcp::model::ServerInfo {
            rmcp::model::ServerInfo::new(
                rmcp::model::ServerCapabilities::builder()
                    #(#capability_calls)*
                    .build()
            )
            #(#builder_calls)*
        }
    })
}
