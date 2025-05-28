use crate::common::TOOL_IDENT;
use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::parse::Parse;
use syn::{ItemImpl, Token, parse_quote};

#[derive(Default)]
struct ToolImplItemAttrs {
    tool_box: Option<Option<Ident>>,
}

impl Parse for ToolImplItemAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut tool_box = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "tool_box" => {
                    tool_box = Some(None);
                    if input.lookahead1().peek(Token![=]) {
                        input.parse::<Token![=]>()?;
                        let value: Ident = input.parse()?;
                        tool_box = Some(Some(value));
                    }
                }
                _ => {
                    return Err(syn::Error::new(key.span(), "unknown attribute"));
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        Ok(ToolImplItemAttrs { tool_box })
    }
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
enum ImplType {
    TraitWithGenerics,
    TraitWithoutGenerics,
    RegularWithGenerics,
    RegularWithoutGenerics,
}

pub(crate) fn tool_impl_item(attr: TokenStream, mut input: ItemImpl) -> syn::Result<TokenStream> {
    let tool_impl_attr: ToolImplItemAttrs = syn::parse2(attr)?;
    let tool_box_ident = tool_impl_attr.tool_box.flatten();

    let tool_fn_idents = extract_tool_function_names(&input);
    let impl_type = determine_impl_type(&input);

    match impl_type {
        ImplType::TraitWithGenerics => {
            handle_trait_with_generics(&mut input, tool_box_ident)?;
        }
        ImplType::TraitWithoutGenerics => {
            handle_trait_without_generics(&mut input, tool_box_ident)?;
        }
        ImplType::RegularWithGenerics => {
            handle_regular_with_generics(&mut input, tool_fn_idents);
        }
        ImplType::RegularWithoutGenerics => {
            // Only process if tool_box_ident exists
            if let Some(ident) = tool_box_ident {
                handle_regular_without_generics(&mut input, tool_fn_idents, ident);
            }
        }
    }

    Ok(quote! {
        #input
    })
}

fn extract_tool_function_names(input: &ItemImpl) -> Vec<Ident> {
    let mut tool_fn_idents = Vec::new();
    for item in &input.items {
        if let syn::ImplItem::Fn(method) = item {
            for attr in &method.attrs {
                if attr.path().is_ident(TOOL_IDENT) {
                    tool_fn_idents.push(method.sig.ident.clone());
                }
            }
        }
    }
    tool_fn_idents
}

fn determine_impl_type(input: &ItemImpl) -> ImplType {
    let is_trait = input.trait_.is_some();
    let has_generics = !input.generics.params.is_empty();
    match (is_trait, has_generics) {
        (true, true) => ImplType::TraitWithGenerics,
        (true, false) => ImplType::TraitWithoutGenerics,
        (false, true) => ImplType::RegularWithGenerics,
        (false, false) => ImplType::RegularWithoutGenerics,
    }
}

fn handle_trait_with_generics(
    input: &mut ItemImpl,
    tool_box_ident: Option<Ident>,
) -> syn::Result<()> {
    if tool_box_ident.is_none() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "tool_box attribute is required for trait implementation",
        ));
    }

    // for trait implementation with generic parameters, directly use the already generated *_inner method
    input.items.extend([
        parse_quote! {
            async fn call_tool(
                &self,
                request: rmcp::model::CallToolRequestParam,
                context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::CallToolResult, rmcp::Error> {
                self.call_tool_inner(request, context).await
            }
        },
        parse_quote! {
            async fn list_tools(
                &self,
                request: Option<rmcp::model::PaginatedRequestParam>,
                context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::ListToolsResult, rmcp::Error> {
                self.list_tools_inner(request, context).await
            }
        },
    ]);
    Ok(())
}

fn handle_trait_without_generics(
    input: &mut ItemImpl,
    tool_box_ident: Option<Ident>,
) -> syn::Result<()> {
    let ident = tool_box_ident.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "tool_box attribute is required for trait implementation",
        )
    })?;
    // if there are no generic parameters, add tool box derive
    input.items.push(parse_quote!(
        rmcp::tool_box!(@derive #ident);
    ));
    Ok(())
}

fn handle_regular_with_generics(input: &mut ItemImpl, tool_fn_idents: Vec<Ident>) {
    // if there are generic parameters, not use tool_box! macro, but generate code directly

    // create call code for each tool function
    let match_arms = tool_fn_idents.iter().map(|ident| {
        let attr_fn = Ident::new(&format!("{}_tool_attr", ident), ident.span());
        let call_fn = Ident::new(&format!("{}_tool_call", ident), ident.span());
        quote! {
            name if name == Self::#attr_fn().name => {
                Self::#call_fn(tcc).await
            }
        }
    });

    let tool_attrs = tool_fn_idents.iter().map(|ident| {
        let attr_fn = Ident::new(&format!("{}_tool_attr", ident), ident.span());
        quote! { Self::#attr_fn() }
    });

    input.items.extend([
        parse_quote! {
            async fn call_tool_inner(
                &self,
                request: rmcp::model::CallToolRequestParam,
                context: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::CallToolResult, rmcp::Error> {
                let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
                match tcc.name() {
                    #(#match_arms,)*
                    _ => Err(rmcp::Error::invalid_params("tool not found", None)),
                }
            }
        },
        parse_quote! {
            async fn list_tools_inner(
                &self,
                _: Option<rmcp::model::PaginatedRequestParam>,
                _: rmcp::service::RequestContext<rmcp::RoleServer>,
            ) -> Result<rmcp::model::ListToolsResult, rmcp::Error> {
                Ok(rmcp::model::ListToolsResult {
                    next_cursor: None,
                    tools: vec![#(#tool_attrs),*],
                })
            }
        },
    ])
}

fn handle_regular_without_generics(
    input: &mut ItemImpl,
    tool_fn_idents: Vec<Ident>,
    tool_box_ident: Ident,
) {
    // if there are no generic parameters, use the original tool_box! macro
    let self_ty = &input.self_ty;
    input.items.push(parse_quote!(
        rmcp::tool_box!(#self_ty {
            #(#tool_fn_idents),*
        } #tool_box_ident);
    ))
}
