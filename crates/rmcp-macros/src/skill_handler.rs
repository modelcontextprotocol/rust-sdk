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
        capability_calls.push(quote! { .enable_skills_extension_without_directory_read() });
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

/// Generates a `ServerHandler` impl with SEP-2640 skills support.
///
/// **Limitation:** The `#[skill]` attribute has no file-list parameter, so
/// macro-generated skills cannot declare their files. Every generated entry
/// therefore carries `resources: "dynamic"` — the only value the spec permits
/// when stable digests cannot be published (`skills.mdx:225`; an entry with no
/// `resources` at all is invalid and hosts MUST NOT load it). Consequences:
/// - The host cannot content-bind or verify digests for these skills.
/// - `resources/directory/read` returns `{"resources": []}` for macro skills,
///   so the macro server does not advertise the `directoryRead` capability.
///
/// Use `#[skill_router]` with explicit `SkillEntry` construction if you need
/// to serve a populated directory listing with content integrity.
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
                // Spec: the final `<skill-path>` segment of `uri` MUST equal
                // `frontmatter.name`. Entries that violate it are invalid, so
                // they are not served.
                let skills = #router.list_all().into_iter()
                    .filter(|entry| {
                        rmcp::model::skills::validate_skill_name(&entry.uri, &entry.frontmatter)
                            .is_ok()
                    })
                    .collect();
                let mut result = rmcp::model::skills::SkillsListResult::new(skills);
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
                // Spec: params.uri MUST be the skill SKILL.md URI. Reject
                // any other URI with -32602. The equality check subsumes both
                // the directory-form and skill-md-form checks and also rejects
                // nested SKILL.md URIs that are not the canonical URI of any
                // registered skill.
                if request.uri != route.attr.uri {
                    return Err(rmcp::ErrorData::invalid_params(
                        format!("not a SKILL.md URI: {}", request.uri),
                        None,
                    ));
                }
                let skill_context = rmcp::handler::server::skill::SkillCallContext::new(
                    self,
                    request.uri.clone(),
                    context,
                );
                let skill_entry = (route.call)(skill_context).await?;
                // Spec: the final `<skill-path>` segment of `uri` MUST equal
                // `frontmatter.name`.
                rmcp::model::skills::validate_skill_name(&skill_entry.uri, &skill_entry.frontmatter)
                    .map_err(|e| rmcp::ErrorData::invalid_params(e, None))?;
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
                // The method applies only to directory resources. The skill root
                // (`skill://<skill-path>`) is a directory; a file URI such as
                // `skill://<skill-path>/SKILL.md` is not, and MUST return -32602.
                let router = #router;
                let resources: Vec<rmcp::model::Resource> = match router.get_by_uri(&request.uri) {
                    Some(route) => {
                        let directory_form = format!(
                            "skill://{}",
                            route.skill_path().unwrap_or_default()
                        );
                        if request.uri != directory_form {
                            return Err(rmcp::ErrorData::invalid_params(
                                format!("not a directory: {}", request.uri),
                                None,
                            ));
                        }
                        match &route.attr.resources {
                            Some(rmcp::model::skills::SkillResources::FileList(files)) => {
                                // Spec, Directory Listing: "The result contains
                                // every direct child of the directory." SKILL.md
                                // is a file directly inside the skill root, so it
                                // IS a direct child and MUST be listed.
                                let root = route.attr.uri
                                    .strip_suffix("/SKILL.md")
                                    .or_else(|| route.attr.uri.strip_suffix("SKILL.md"))
                                    .unwrap_or(&route.attr.uri);
                                let prefix = format!("{}/", root);
                                // Direct children only: a file maps to itself, a
                                // nested path maps to its first segment as a
                                // directory resource. The listing is not recursive.
                                let mut children: std::collections::BTreeMap<String, bool> =
                                    std::collections::BTreeMap::new();
                                for file in files {
                                    let Some(rest) = file.uri.strip_prefix(&prefix) else {
                                        continue;
                                    };
                                    if rest.is_empty() {
                                        continue;
                                    }
                                    match rest.split_once('/') {
                                        Some((head, _)) => {
                                            children.insert(format!("{}/{}", root, head), true);
                                        }
                                        None => {
                                            children.entry(file.uri.clone()).or_insert(false);
                                        }
                                    }
                                }
                                children.into_iter().map(|(uri, is_dir)| {
                                    let name = uri.rsplit('/').next().unwrap_or("").to_string();
                                    let resource = rmcp::model::Resource::new(uri, name);
                                    if is_dir {
                                        resource.with_mime_type("inode/directory")
                                    } else {
                                        resource
                                    }
                                }).collect()
                            }
                            _ => vec![],
                        }
                    }
                    None => {
                        // Spec line 453: a URI that does not identify a skill the
                        // server serves MUST return -32602, not an empty list.
                        return Err(rmcp::ErrorData::invalid_params(
                            format!("unknown skill URI: {}", request.uri),
                            None,
                        ));
                    }
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
