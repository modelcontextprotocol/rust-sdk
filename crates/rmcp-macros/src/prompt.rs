use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Expr, Ident, ImplItemFn, ReturnType};

use crate::common::{extract_doc_line, none_expr};

#[derive(FromMeta, Default, Debug)]
#[darling(default)]
pub struct PromptAttribute {
    /// The name of the prompt
    pub name: Option<String>,
    /// Human readable title of prompt
    pub title: Option<String>,
    /// Optional description of what the prompt does
    pub description: Option<String>,
    /// Arguments that can be passed to the prompt
    pub arguments: Option<Expr>,
    /// Optional icons for the prompt
    pub icons: Option<Expr>,
    /// Optional metadata for the prompt
    pub meta: Option<Expr>,
    /// When true, the generated future will not require `Send`. Useful for `!Send` handlers
    /// (e.g. single-threaded database connections). Also enabled globally by the `local` crate feature.
    pub local: bool,
}

pub struct ResolvedPromptAttribute {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<Expr>,
    pub arguments: Expr,
    pub icons: Option<Expr>,
    pub meta: Option<Expr>,
}

impl ResolvedPromptAttribute {
    pub fn into_fn(self, fn_ident: Ident) -> syn::Result<ImplItemFn> {
        let Self {
            name,
            description,
            arguments,
            title,
            icons,
            meta,
        } = self;
        let description = if let Some(description) = description {
            quote! { Some::<String>(#description.into()) }
        } else {
            quote! { None::<String> }
        };
        let title_call = title
            .map(|t| quote! { .with_title(#t) })
            .unwrap_or_default();
        let icons_call = icons
            .map(|i| quote! { .with_icons(#i) })
            .unwrap_or_default();
        let meta_call = meta.map(|m| quote! { .with_meta(#m) }).unwrap_or_default();
        let tokens = quote! {
            pub fn #fn_ident() -> rmcp::model::Prompt {
                rmcp::model::Prompt::from_raw(
                    #name,
                    #description,
                    #arguments,
                )
                #title_call
                #icons_call
                #meta_call
            }
        };
        syn::parse2::<ImplItemFn>(tokens)
    }
}

pub fn prompt(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attribute = if attr.is_empty() {
        Default::default()
    } else {
        let attr_args = NestedMeta::parse_meta_list(attr)?;
        PromptAttribute::from_list(&attr_args)?
    };
    let mut fn_item = syn::parse2::<ImplItemFn>(input.clone())?;
    let fn_ident = &fn_item.sig.ident;
    let omit_send = cfg!(feature = "local") || attribute.local;

    let prompt_attr_fn_ident = format_ident!("{}_prompt_attr", fn_ident);

    // Try to find prompt parameters from function parameters
    let arguments_expr = if let Some(arguments) = attribute.arguments {
        arguments
    } else {
        // Look for a type named Parameters in the function signature
        let params_ty = crate::common::find_parameters_type_impl(&fn_item);

        if let Some(params_ty) = params_ty {
            // Generate arguments from the type's schema with caching
            syn::parse2::<Expr>(quote! {
                rmcp::handler::server::prompt::cached_arguments_from_schema::<#params_ty>()
            })?
        } else {
            // No arguments
            none_expr()?
        }
    };

    let name = attribute.name.unwrap_or_else(|| fn_ident.to_string());
    let description = if let Some(s) = attribute.description {
        Some(Expr::Lit(syn::ExprLit {
            attrs: Vec::new(),
            lit: syn::Lit::Str(syn::LitStr::new(&s, Span::call_site())),
        }))
    } else {
        fn_item.attrs.iter().try_fold(None, extract_doc_line)?
    };
    let arguments = arguments_expr;

    let resolved_prompt_attr = ResolvedPromptAttribute {
        name: name.clone(),
        description: description.clone(),
        arguments: arguments.clone(),
        title: attribute.title,
        icons: attribute.icons,
        meta: attribute.meta,
    };
    let prompt_attr_fn = resolved_prompt_attr.into_fn(prompt_attr_fn_ident.clone())?;

    // Modify the input function for async support (same as tool macro)
    if fn_item.sig.asyncness.is_some() {
        // 1. remove asyncness from sig
        // 2. make return type: `std::pin::Pin<Box<dyn std::future::Future<Output = #ReturnType> + Send + '_>>`
        //    (omit `+ Send` when the `local` crate feature is active or `#[prompt(local)]` is used)
        // 3. make body: { Box::pin(async move { #body }) }
        let new_output = syn::parse2::<ReturnType>({
            let mut lt = quote! { 'static };
            if let Some(receiver) = fn_item.sig.receiver() {
                if let Some((_, receiver_lt)) = receiver.reference.as_ref() {
                    if let Some(receiver_lt) = receiver_lt {
                        lt = quote! { #receiver_lt };
                    } else {
                        lt = quote! { '_ };
                    }
                }
            }
            match &fn_item.sig.output {
                syn::ReturnType::Default => {
                    if omit_send {
                        quote! { -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ()> + #lt>> }
                    } else {
                        quote! { -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ()> + Send + #lt>> }
                    }
                }
                syn::ReturnType::Type(_, ty) => {
                    if omit_send {
                        quote! { -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = #ty> + #lt>> }
                    } else {
                        quote! { -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = #ty> + Send + #lt>> }
                    }
                }
            }
        })?;
        let prev_block = &fn_item.block;
        let new_block = syn::parse2::<syn::Block>(quote! {
           { Box::pin(async move #prev_block ) }
        })?;
        fn_item.sig.asyncness = None;
        fn_item.sig.output = new_output;
        fn_item.block = new_block;
    }

    Ok(quote! {
        #prompt_attr_fn
        #fn_item
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_prompt_macro() -> syn::Result<()> {
        let attr = quote! {
            name = "example-prompt",
            description = "An example prompt"
        };
        let input = quote! {
            async fn example_prompt(&self, Parameters(args): Parameters<ExampleArgs>) -> Result<String> {
                Ok("Example prompt response".to_string())
            }
        };
        let result = prompt(attr, input)?;

        // Verify the output contains both the attribute function and the modified function
        let result_str = result.to_string();
        assert!(result_str.contains("example_prompt_prompt_attr"));
        assert!(
            result_str.contains("rmcp")
                && result_str.contains("model")
                && result_str.contains("Prompt")
        );

        Ok(())
    }

    #[test]
    fn test_doc_comment_description() -> syn::Result<()> {
        let attr = quote! {}; // No explicit description
        let input = quote! {
            /// This is a test prompt description
            /// with multiple lines
            fn test_prompt(&self) -> Result<String> {
                Ok("Test".to_string())
            }
        };
        let result = prompt(attr, input)?;

        // The output should contain the description from doc comments
        let result_str = result.to_string();
        assert!(result_str.contains("This is a test prompt description"));
        assert!(result_str.contains("with multiple lines"));

        Ok(())
    }

    #[test]
    fn test_doc_include_description() -> syn::Result<()> {
        let attr = quote! {}; // No explicit description
        let input = quote! {
            #[doc = include_str!("some/test/data/doc.txt")]
            fn test_prompt_included(&self) -> Result<String> {
                Ok("Test".to_string())
            }
        };
        let result = prompt(attr, input)?;

        // The generated tokens should preserve the include_str! invocation
        let result_str = result.to_string();
        assert!(result_str.contains("include_str"));

        Ok(())
    }

    #[test]
    fn test_async_prompt_default_send_behavior() -> syn::Result<()> {
        let attr = quote! {};
        let input = quote! {
            async fn test_prompt_default_send(&self) -> String {
                "ok".to_string()
            }
        };
        let result = prompt(attr, input)?;

        let result_str = result.to_string();
        if cfg!(feature = "local") {
            assert!(!result_str.contains("+ Send +"));
        } else {
            assert!(result_str.contains("+ Send +"));
        }
        Ok(())
    }

    #[test]
    fn test_async_prompt_local_omits_send() -> syn::Result<()> {
        let attr = quote! { local };
        let input = quote! {
            async fn test_prompt_local_no_send(&self) -> String {
                "ok".to_string()
            }
        };
        let result = prompt(attr, input)?;

        let result_str = result.to_string();
        assert!(!result_str.contains("+ Send +"));
        Ok(())
    }
}
