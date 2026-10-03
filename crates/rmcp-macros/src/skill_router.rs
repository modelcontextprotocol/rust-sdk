//! Skill router proc-macro — the gradebook.
//!
//! Pedagogy: The `#[skill_router]` macro walks the class roster (the impl
//! block), finds every assignment with a `#[skill]` sticker, and builds a
//! gradebook (`SkillRouter`) that maps each skill's URI path to its handler.
//! Like `tool_router`, but the lookup key is a parsed `skill://` URI segment
//! instead of a flat tool name.

use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Ident, ImplItem, ItemImpl, Visibility};

// Consumed by `darling` macro expansion — not dead code.
#[allow(dead_code)]
#[derive(FromMeta)]
#[darling(default)]
pub struct SkillRouterAttribute {
    pub router: Ident,
    pub vis: Option<Visibility>,
    pub server_handler: bool,
    pub allow_empty: bool,
}

impl Default for SkillRouterAttribute {
    fn default() -> Self {
        Self {
            router: format_ident!("skill_router"),
            vis: None,
            server_handler: false,
            allow_empty: false,
        }
    }
}

pub fn skill_router(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attr_args = NestedMeta::parse_meta_list(attr)?;
    let SkillRouterAttribute {
        router,
        vis,
        server_handler,
        allow_empty,
    } = SkillRouterAttribute::from_list(&attr_args)?;
    let mut item_impl = syn::parse2::<ItemImpl>(input)?;

    let skill_attr_fns: Vec<_> = item_impl
        .items
        .iter()
        .filter_map(|item| {
            if let syn::ImplItem::Fn(fn_item) = item {
                fn_item
                    .attrs
                    .iter()
                    .any(|attr| {
                        attr.path()
                            .segments
                            .last()
                            .is_some_and(|seg| seg.ident == "skill")
                    })
                    .then_some(&fn_item.sig.ident)
            } else {
                None
            }
        })
        .collect();

    if skill_attr_fns.is_empty() && !allow_empty {
        return Err(syn::Error::new_spanned(
            &item_impl.self_ty,
            format!(
                "`#[skill_router]` found no `#[skill]` fn in this impl block, so `Self::{router}()` would serve no skills"
            ),
        ));
    }

    let mut routers = Vec::with_capacity(skill_attr_fns.len());
    for handler in skill_attr_fns {
        let skill_attr_fn_ident = format_ident!("{}_skill_attr", handler);
        routers.push(quote! {
            .with_route((Self::#skill_attr_fn_ident(), Self::#handler))
        });
    }

    let router_fn = syn::parse2::<ImplItem>(quote! {
        #vis fn #router() -> rmcp::handler::server::router::skill::SkillRouter<Self> {
            rmcp::handler::server::router::skill::SkillRouter::<Self>::new()
                #(#routers)*
        }
    })?;
    item_impl.items.push(router_fn);

    if !server_handler {
        return Ok(item_impl.into_token_stream());
    }

    if item_impl.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            item_impl,
            "`server_handler` is only supported on inherent impl blocks",
        ));
    }

    let self_ty = &item_impl.self_ty;
    let (impl_generics, ty_generics, where_clause) = item_impl.generics.split_for_impl();

    Ok(quote! {
        #item_impl

        #[::rmcp::skill_handler(router = Self::#router())]
        impl #impl_generics ::rmcp::ServerHandler for #self_ty #ty_generics #where_clause {}
    })
}
