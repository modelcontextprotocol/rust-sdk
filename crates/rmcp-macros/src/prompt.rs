use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Expr, Ident, ImplItemFn, ReturnType};

use crate::common::{extract_doc_line, none_expr};

#[derive(FromMeta, Default, Debug)]
#[darling(default)]
pub struct PromptAttribute {
    /// The name of the prompt
    pub name: Option<String>,
    /// Optional description of what the prompt does
    pub description: Option<String>,
    /// Arguments that can be passed to customize the prompt
    pub arguments: Option<Expr>,
}

pub struct ResolvedPromptAttribute {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Expr,
}

impl ResolvedPromptAttribute {
    pub fn into_fn(self, fn_ident: Ident) -> syn::Result<ImplItemFn> {
        let Self {
            name,
            description,
            arguments,
        } = self;
        let description = if let Some(description) = description {
            quote! { Some(#description.into()) }
        } else {
            quote! { None }
        };
        let tokens = quote! {
            pub fn #fn_ident() -> rmcp::model::Prompt {
                rmcp::model::Prompt {
                    name: #name.into(),
                    description: #description,
                    arguments: #arguments,
                }
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

    let prompt_attr_fn_ident = format_ident!("{}_prompt_attr", fn_ident);

    // Try to find prompt arguments from function parameters
    let arguments_expr = if let Some(arguments) = attribute.arguments {
        arguments
    } else {
        // Look for a type named Arguments or PromptArguments in the function signature
        let args_ty = fn_item.sig.inputs.iter().find_map(|input| {
            if let syn::FnArg::Typed(pat_type) = input {
                if let syn::Type::Path(type_path) = &*pat_type.ty {
                    if let Some(last_segment) = type_path.path.segments.last() {
                        if last_segment.ident == "Arguments"
                            || last_segment.ident == "PromptArguments"
                        {
                            // Extract the inner type from Arguments<T>
                            if let syn::PathArguments::AngleBracketed(args) =
                                &last_segment.arguments
                            {
                                if let Some(syn::GenericArgument::Type(inner_ty)) =
                                    args.args.first()
                                {
                                    return Some(inner_ty.clone());
                                }
                            }
                        }
                    }
                }
            }
            None
        });

        if let Some(args_ty) = args_ty {
            // Generate arguments from the type's schema with caching
            syn::parse2::<Expr>(quote! {
                rmcp::handler::server::prompt::cached_arguments_from_schema::<#args_ty>()
            })?
        } else {
            // No arguments
            none_expr()?
        }
    };

    let name = attribute.name.unwrap_or_else(|| fn_ident.to_string());
    let description = attribute
        .description
        .or_else(|| fn_item.attrs.iter().fold(None, extract_doc_line));
    let arguments = arguments_expr;

    let resolved_prompt_attr = ResolvedPromptAttribute {
        name: name.clone(),
        description: description.clone(),
        arguments: arguments.clone(),
    };
    let prompt_attr_fn = resolved_prompt_attr.into_fn(prompt_attr_fn_ident.clone())?;

    // Modify the input function for async support
    if fn_item.sig.asyncness.is_some() {
        // 1. remove asyncness from sig
        // 2. make return type: `std::pin::Pin<Box<dyn Future<Output = #ReturnType> + Send + '_>>`
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
                    quote! { -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + #lt>> }
                }
                syn::ReturnType::Type(_, ty) => {
                    quote! { -> std::pin::Pin<Box<dyn Future<Output = #ty> + Send + #lt>> }
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
            async fn example_prompt(&self, Arguments(args): Arguments<ExampleArgs>) -> Result<String> {
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
}

