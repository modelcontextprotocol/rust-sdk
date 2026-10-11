use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Expr, ImplItem, ItemImpl};

use crate::common::{has_method, has_sibling_handler};

// Consumed by `darling` macro expansion — not dead code.
#[allow(dead_code)]
#[derive(FromMeta, Debug)]
#[darling(default)]
pub struct SkillHandlerAttribute {
    pub router: Expr,
    pub meta: Option<Expr>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub instructions: Option<String>,
}

impl Default for SkillHandlerAttribute {
    fn default() -> Self {
        Self {
            router: syn::parse2(quote! { Self::skill_router() }).unwrap(),
            meta: None,
            name: None,
            version: None,
            instructions: None,
        }
    }
}

// Consumed by `darling` macro expansion — not dead code.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallerCapability {
    Skills,
}

pub(crate) fn build_get_info(
    item_impl: &ItemImpl,
    name: Option<String>,
    version: Option<String>,
    instructions: Option<String>,
    caller: CallerCapability,
) -> syn::Result<ImplItem> {
    let has_skills =
        caller == CallerCapability::Skills || has_sibling_handler(item_impl, "skill_handler");

    let mut capability_calls = Vec::new();
    if has_skills {
        capability_calls
            .push(quote! { .enable_skills_with(rmcp::model::SkillsCapability::new(true)) });
        capability_calls.push(quote! { .enable_skills_extension() });
        capability_calls.push(quote! { .enable_resources() });
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
        fn get_info(&self) -> rmcp::model::InitializeResult {
            rmcp::model::InitializeResult::new(
                rmcp::model::ServerCapabilities::builder()
                    #(#capability_calls)*
                    .build()
            )
            #(#builder_calls)*
        }
    })
}

pub fn skill_handler(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attr_args = NestedMeta::parse_meta_list(attr)?;
    let SkillHandlerAttribute {
        router,
        meta: _,
        name,
        version,
        instructions,
    } = SkillHandlerAttribute::from_list(&attr_args)?;
    let mut item_impl = syn::parse2::<ItemImpl>(input)?;

    if !has_method("skills_list", &item_impl) {
        let skill_list_fn = syn::parse2::<ImplItem>(quote! {
            async fn skills_list(
                &self,
                _request: Option<rmcp::model::PaginatedRequestParams>,
                context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::skills::SkillsListResult, rmcp::ErrorData> {
                let supports_cache_hints = context.protocol_version().is_some_and(|version| {
                    version >= rmcp::model::ProtocolVersion::V_2026_07_28
                });
                let mut result = rmcp::model::skills::SkillsListResult::new(#router.list_all());
                result.result_type = Some(rmcp::model::ResultType::COMPLETE);
                result.ttl_ms = supports_cache_hints.then_some(0);
                result.cache_scope = supports_cache_hints.then_some("public".to_string());
                Ok(result)
            }
        })?;
        item_impl.items.push(skill_list_fn);
    }

    if !has_method("skills_get", &item_impl) {
        let skill_get_fn = syn::parse2::<ImplItem>(quote! {
            async fn skills_get(
                &self,
                request: rmcp::model::skills::SkillsGetRequestParams,
                context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::skills::SkillsGetResult, rmcp::ErrorData> {
                let router = #router;
                let route = router.get_by_uri(&request.uri)
                    .ok_or_else(|| rmcp::ErrorData::invalid_params(
                        format!("skill not found: {}", request.uri),
                        None,
                    ))?;
                let skill_context = rmcp::handler::server::skill::SkillCallContext::new(
                    self,
                    request.uri.clone(),
                    context,
                );
                let skill_entry = (route.call)(skill_context).await?;
                Ok(rmcp::model::skills::SkillsGetResult::new(skill_entry))
            }
        })?;
        item_impl.items.push(skill_get_fn);
    }

    if !has_method("resources_directory_read", &item_impl) {
        let directory_read_fn = syn::parse2::<ImplItem>(quote! {
            async fn resources_directory_read(
                &self,
                request: rmcp::model::skills::ResourcesDirectoryReadRequestParams,
                context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::skills::ResourcesDirectoryReadResult, rmcp::ErrorData> {
                let supports_cache_hints = context.protocol_version().is_some_and(|version| {
                    version >= rmcp::model::ProtocolVersion::V_2026_07_28
                });
                // Enumerate files for multi-file skills
                let router = #router;
                let resources: Vec<rmcp::model::Resource> = if let Some(route) = router.get_by_uri(&request.uri) {
                    match &route.attr.resources {
                        Some(rmcp::model::skills::SkillResources::FileList(files)) => {
                            files.iter().map(|file| {
                                let name = file.uri.rsplit('/').next().unwrap_or("").to_string();
                                rmcp::model::Resource::new(file.uri.clone(), name)
                            }).collect()
                        }
                        Some(rmcp::model::skills::SkillResources::Dynamic) => vec![],
                        Some(_) => vec![],
                        None => vec![],
                    }
                } else {
                    vec![]
                };
                let mut result = rmcp::model::skills::ResourcesDirectoryReadResult::new(resources);
                result.result_type = Some(rmcp::model::ResultType::COMPLETE);
                result.ttl_ms = supports_cache_hints.then_some(0);
                result.cache_scope = supports_cache_hints.then_some("public".to_string());
                Ok(result)
            }
        })?;
        item_impl.items.push(directory_read_fn);
    }

    if !has_method("get_info", &item_impl) && !has_sibling_handler(&item_impl, "tool_handler") {
        let get_info_fn = build_get_info(
            &item_impl,
            name,
            version,
            instructions,
            CallerCapability::Skills,
        )?;
        item_impl.items.push(get_info_fn);
    }

    Ok(item_impl.into_token_stream())
}
