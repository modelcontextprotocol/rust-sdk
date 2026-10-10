//! Skill proc-macro — the sticker.
//!
//! Pedagogy: The `#[skill]` macro puts a sticker on an assignment that says
//! "this is a skill handler, here's its ID card (SkillEntry)." It generates
//! a companion `*_skill_attr()` function that returns the skill's metadata
//! (URI + frontmatter + resources), and wraps the async body in a
//! `Pin<Box<Future>>` so the gradebook can call it synchronously.

use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Expr, Ident, ImplItemFn, LitStr, parse_quote};

use crate::common::extract_doc_line;

#[allow(dead_code)]
#[derive(FromMeta, Default, Debug)]
#[darling(default)]
pub struct SkillAttribute {
    /// The `skill://` URI this handler serves, e.g. `skill://git-workflow/SKILL.md`.
    pub uri: Option<String>,
    /// Path to the frontmatter JSON (typically `include_str!("...frontmatter.json")`).
    pub frontmatter: Option<Expr>,
    /// Human-readable description. Falls back to doc-comments.
    pub description: Option<String>,
    /// Whether this skill generates its content dynamically (no fixed file list).
    pub dynamic: bool,
    /// When true, the generated future will not require `Send`.
    pub local: bool,
}

#[allow(dead_code)]
pub struct ResolvedSkillAttribute {
    pub uri: String,
    pub frontmatter: Expr,
    pub description: Option<Expr>,
    pub dynamic: bool,
}

impl ResolvedSkillAttribute {
    pub fn into_fn(self, fn_ident: Ident) -> syn::Result<ImplItemFn> {
        let Self {
            uri,
            frontmatter,
            description,
            dynamic,
        } = self;
        let _description = if let Some(description) = description {
            quote! { Some(#description.into()) }
        } else {
            quote! { None }
        };
        let resources = if dynamic {
            quote! { Some(rmcp::model::skills::SkillResources::Dynamic) }
        } else {
            quote! { None }
        };
        let doc_comment = format!("Generated skill metadata function for {uri}");
        let doc_attr: syn::Attribute = parse_quote!(#[doc = #doc_comment]);
        let tokens = quote! {
            #doc_attr
            pub fn #fn_ident() -> rmcp::model::skills::SkillEntry {
                let mut entry = rmcp::model::skills::SkillEntry::new(#uri, #frontmatter);
                entry.resources = #resources;
                entry
            }
        };
        syn::parse2::<ImplItemFn>(tokens)
    }
}

pub fn skill(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attribute = if attr.is_empty() {
        Default::default()
    } else {
        let attr_args = NestedMeta::parse_meta_list(attr)?;
        SkillAttribute::from_list(&attr_args)?
    };
    let fn_item = syn::parse2::<ImplItemFn>(input.clone())?;
    let fn_ident = &fn_item.sig.ident;

    let skill_attr_fn_ident = format_ident!("{}_skill_attr", fn_ident);

    // Validate URI is present
    let uri = attribute.uri.ok_or_else(|| {
        syn::Error::new_spanned(
            fn_ident,
            "`#[skill]` attribute requires a `uri` parameter, e.g. `#[skill(uri = \"skill://my-skill/SKILL.md\")]`",
        )
    })?;

    // Validate URI format at compile time (mirrors rmcp::model::skills::parse_skill_uri)
    if !uri.starts_with("skill://") || uri.len() <= 8 {
        return Err(syn::Error::new_spanned(
            fn_ident,
            format!(
                "`#[skill]` URI must start with `skill://` and contain a path + file (got `{uri}`)"
            ),
        ));
    }
    let stripped = &uri[8..];
    if stripped.is_empty() || stripped.ends_with('/') {
        return Err(syn::Error::new_spanned(
            fn_ident,
            format!(
                "`#[skill]` URI must not end with `/` and must contain a file path (got `{uri}`)"
            ),
        ));
    }
    if !stripped.contains('/') {
        return Err(syn::Error::new_spanned(
            fn_ident,
            format!(
                "`#[skill]` URI must contain at least one `/` separating skill path from file (got `{uri}`)"
            ),
        ));
    }

    // Validate frontmatter is present
    let frontmatter = attribute.frontmatter.ok_or_else(|| {
        syn::Error::new_spanned(
            fn_ident,
            "`#[skill]` attribute requires a `frontmatter` parameter, e.g. `#[skill(frontmatter = include_str!(\"frontmatter.json\"))]`",
        )
    })?;

    let description_expr = if let Some(s) = attribute.description {
        Some(Expr::Lit(syn::ExprLit {
            attrs: Vec::new(),
            lit: syn::Lit::Str(LitStr::new(&s, Span::call_site())),
        }))
    } else {
        fn_item.attrs.iter().try_fold(None, extract_doc_line)?
    };

    let resolved = ResolvedSkillAttribute {
        uri,
        frontmatter,
        description: description_expr,
        dynamic: attribute.dynamic,
    };
    let skill_attr_fn = resolved.into_fn(skill_attr_fn_ident.clone())?;

    // Generate the CallSkillHandler wrapper.
    // The user's function is the "body" — it takes &self and returns Result<(), ErrorData>.
    // The router needs a fn that takes &self + SkillCallContext<S> and returns Result<SkillEntry, ErrorData>.
    // We generate a wrapper that calls the user's fn, then returns the SkillEntry from *_skill_attr().
    // The original function is kept so the wrapper can call it.
    let handler_fn_ident = format_ident!("{}_handler", fn_ident);
    let skill_attr_fn_ident_clone = skill_attr_fn_ident.clone();

    let handler_fn = if fn_item.sig.asyncness.is_some() {
        quote! {
            #[allow(clippy::too_many_arguments)]
            fn #handler_fn_ident(
                &self,
                ctx: rmcp::handler::server::skill::SkillCallContext<'_, Self>,
            ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = Result<rmcp::model::skills::SkillEntry, rmcp::ErrorData>> + Send + 'static>> {
                let this = self.clone();
                Box::pin(async move {
                    let _ = ctx.service;
                    this.#fn_ident().await?;
                    Ok(Self::#skill_attr_fn_ident_clone())
                })
            }
        }
    } else {
        quote! {
            #[allow(clippy::too_many_arguments)]
            fn #handler_fn_ident(
                &self,
                _ctx: rmcp::handler::server::skill::SkillCallContext<'_, Self>,
            ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = Result<rmcp::model::skills::SkillEntry, rmcp::ErrorData>> + Send + 'static>> {
                let this = self.clone();
                Box::pin(async move {
                    this.#fn_ident()?;
                    Ok(Self::#skill_attr_fn_ident_clone())
                })
            }
        }
    };

    Ok(quote! {
        #skill_attr_fn
        #fn_item
        #handler_fn
    })
}
