//! Procedural macro implementation for `#[tool_router]` (see `lib.rs`).
//!
//! When `server_handler` is set, we emit a second `impl ServerHandler` item decorated with
//! `#[::rmcp::tool_handler]` so `tool_handler` expands in a later proc-macro pass—keeping all
//! tool dispatch and `get_info` logic in `tool_handler.rs` without duplicating it here.

use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Ident, ImplItem, ItemImpl, Visibility};

#[derive(FromMeta)]
#[darling(default)]
pub struct ToolRouterAttribute {
    pub router: Ident,
    pub vis: Option<Visibility>,
    /// When set, also emit `#[::rmcp::tool_handler]` on `impl ServerHandler for Self` so callers
    /// can skip a separate `#[tool_handler]` block (expanded in a later macro pass).
    pub server_handler: bool,
}

impl Default for ToolRouterAttribute {
    fn default() -> Self {
        Self {
            router: format_ident!("tool_router"),
            vis: None,
            server_handler: false,
        }
    }
}

pub fn tool_router(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attr_args = NestedMeta::parse_meta_list(attr)?;
    let ToolRouterAttribute {
        router,
        vis,
        server_handler,
    } = ToolRouterAttribute::from_list(&attr_args)?;
    let mut item_impl = syn::parse2::<ItemImpl>(input)?;
    // find all function marked with `#[rmcp::tool]`
    let tool_attr_fns: Vec<_> = item_impl
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
                            .is_some_and(|seg| seg.ident == "tool")
                    })
                    .then_some(&fn_item.sig.ident)
            } else {
                None
            }
        })
        .collect();
    let mut routers = Vec::with_capacity(tool_attr_fns.len());
    for handler in tool_attr_fns {
        let tool_attr_fn_ident = format_ident!("{handler}_tool_attr");
        routers.push(quote! {
            .with_route((Self::#tool_attr_fn_ident(), Self::#handler))
        })
    }
    let router_fn = syn::parse2::<ImplItem>(quote! {
        #vis fn #router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
            rmcp::handler::server::router::tool::ToolRouter::<Self>::new()
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
            "`server_handler` is only supported on inherent impl blocks (e.g. `impl MyType { ... }`)",
        ));
    }

    let self_ty = &item_impl.self_ty;
    let (impl_generics, ty_generics, where_clause) = item_impl.generics.split_for_impl();

    Ok(quote! {
        #item_impl

        #[::rmcp::tool_handler(router = Self::#router())]
        impl #impl_generics ::rmcp::ServerHandler for #self_ty #ty_generics #where_clause {}
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn tool_router_attribute_parses_router_visibility_and_defaults_server_handler_off()
    -> syn::Result<()> {
        let attr = quote! {
            router = test_router,
            vis = "pub(crate)"
        };
        let attr_args = NestedMeta::parse_meta_list(attr)?;
        let ToolRouterAttribute {
            router,
            vis,
            server_handler,
        } = ToolRouterAttribute::from_list(&attr_args)?;
        assert_eq!(router.to_string(), "test_router");
        assert!(vis.is_some(), "vis = \"pub(crate)\" should parse");
        assert!(
            !server_handler,
            "server_handler should default to false when omitted"
        );
        Ok(())
    }

    #[test]
    fn tool_router_attribute_parses_server_handler_flag() -> syn::Result<()> {
        let attr = quote! {
            router = custom_router,
            server_handler
        };
        let attr_args = NestedMeta::parse_meta_list(attr)?;
        let ToolRouterAttribute {
            router,
            server_handler,
            ..
        } = ToolRouterAttribute::from_list(&attr_args)?;
        assert_eq!(router.to_string(), "custom_router");
        assert!(server_handler);
        Ok(())
    }
}
