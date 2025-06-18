use darling::{FromMeta, ast::NestedMeta};
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Expr, Ident, ImplItemFn, ReturnType};
#[derive(FromMeta, Default, Debug)]
#[darling(default)]
pub struct ToolAttribute {
    /// The name of the tool
    pub name: Option<String>,
    pub description: Option<String>,
    /// A JSON Schema object defining the expected parameters for the tool
    pub input_schema: Option<Expr>,
    /// Optional additional tool information.
    pub annotations: Option<ToolAnnotationsAttribute>,
}

pub struct ResolvedToolAttribute {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Expr,
    pub annotations: Expr,
}

impl ResolvedToolAttribute {
    pub fn into_fn(self, fn_ident: Ident) -> syn::Result<ImplItemFn> {
        let Self {
            name,
            description,
            input_schema,
            annotations,
        } = self;
        let description = if let Some(description) = description {
            quote! { Some(#description.into()) }
        } else {
            quote! { None }
        };
        let tokens = quote! {
            pub fn #fn_ident() -> rmcp::model::Tool {
                rmcp::model::Tool {
                    name: #name.into(),
                    description: #description,
                    input_schema: #input_schema,
                    annotations: #annotations,
                }
            }
        };
        syn::parse2::<ImplItemFn>(tokens)
    }
}

#[derive(FromMeta, Debug, Default)]
#[darling(default)]
pub struct ToolAnnotationsAttribute {
    /// A human-readable title for the tool.
    pub title: Option<String>,

    /// If true, the tool does not modify its environment.
    ///
    /// Default: false
    pub read_only_hint: Option<bool>,

    /// If true, the tool may perform destructive updates to its environment.
    /// If false, the tool performs only additive updates.
    ///
    /// (This property is meaningful only when `readOnlyHint == false`)
    ///
    /// Default: true
    /// A human-readable description of the tool's purpose.
    pub destructive_hint: Option<bool>,

    /// If true, calling the tool repeatedly with the same arguments
    /// will have no additional effect on the its environment.
    ///
    /// (This property is meaningful only when `readOnlyHint == false`)
    ///
    /// Default: false.
    pub idempotent_hint: Option<bool>,

    /// If true, this tool may interact with an "open world" of external
    /// entities. If false, the tool's domain of interaction is closed.
    /// For example, the world of a web search tool is open, whereas that
    /// of a memory tool is not.
    ///
    /// Default: true
    pub open_world_hint: Option<bool>,
}

fn none_expr() -> Expr {
    syn::parse2::<Expr>(quote! { None }).unwrap()
}

// extract doc line from attribute
fn extract_doc_line(existing_docs: Option<String>, attr: &syn::Attribute) -> Option<String> {
    if !attr.path().is_ident("doc") {
        return None;
    }

    let syn::Meta::NameValue(name_value) = &attr.meta else {
        return None;
    };

    let syn::Expr::Lit(expr_lit) = &name_value.value else {
        return None;
    };

    let syn::Lit::Str(lit_str) = &expr_lit.lit else {
        return None;
    };

    let content = lit_str.value().trim().to_string();
    match (existing_docs, content) {
        (Some(mut existing_docs), content) if !content.is_empty() => {
            existing_docs.push('\n');
            existing_docs.push_str(&content);
            Some(existing_docs)
        }
        (Some(existing_docs), _) => Some(existing_docs),
        (None, content) if !content.is_empty() => Some(content),
        _ => None,
    }
}

pub fn tool(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let attribute = if attr.is_empty() {
        Default::default()
    } else {
        let attr_args = NestedMeta::parse_meta_list(attr)?;
        ToolAttribute::from_list(&attr_args)?
    };
    let mut fn_item = syn::parse2::<ImplItemFn>(input.clone())?;
    let fn_ident = &fn_item.sig.ident;

    let tool_attr_fn_ident = format_ident!("{}_tool_attr", fn_ident);
    let input_schema_expr = if let Some(input_schema) = attribute.input_schema {
        input_schema
    } else {
        // try to find some parameters wrapper in the function
        let params_ty = fn_item.sig.inputs.iter().find_map(|input| {
            if let syn::FnArg::Typed(pat_type) = input {
                if let syn::Type::Path(type_path) = &*pat_type.ty {
                    if type_path
                        .path
                        .segments
                        .last()
                        .is_some_and(|type_name| type_name.ident == "Parameters")
                    {
                        return Some(pat_type.ty.clone());
                    }
                }
            }
            None
        });
        if let Some(params_ty) = params_ty {
            // if found, use the Parameters schema
            syn::parse2::<Expr>(quote! {
                rmcp::handler::server::tool::cached_schema_for_type::<#params_ty>()
            })?
        } else {
            // if not found, use the default EmptyObject schema
            syn::parse2::<Expr>(quote! {
                rmcp::handler::server::tool::cached_schema_for_type::<rmcp::model::EmptyObject>()
            })?
        }
    };
    let annotations_expr = if let Some(annotations) = attribute.annotations {
        let ToolAnnotationsAttribute {
            title,
            read_only_hint,
            destructive_hint,
            idempotent_hint,
            open_world_hint,
        } = annotations;
        fn wrap_option<T: ToTokens>(x: Option<T>) -> TokenStream {
            x.map(|x| quote! {Some(#x.into())})
                .unwrap_or(quote! { None })
        }
        let title = wrap_option(title);
        let read_only_hint = wrap_option(read_only_hint);
        let destructive_hint = wrap_option(destructive_hint);
        let idempotent_hint = wrap_option(idempotent_hint);
        let open_world_hint = wrap_option(open_world_hint);
        let token_stream = quote! {
            Some(rmcp::model::ToolAnnotations {
                title: #title,
                read_only_hint: #read_only_hint,
                destructive_hint: #destructive_hint,
                idempotent_hint: #idempotent_hint,
                open_world_hint: #open_world_hint,
            })
        };
        syn::parse2::<Expr>(token_stream)?
    } else {
        none_expr()
    };
    let resolved_tool_attr = ResolvedToolAttribute {
        name: attribute.name.unwrap_or_else(|| fn_ident.to_string()),
        description: attribute
            .description
            .or_else(|| fn_item.attrs.iter().fold(None, extract_doc_line)),
        input_schema: input_schema_expr,
        annotations: annotations_expr,
    };
    let tool_attr_fn = resolved_tool_attr.into_fn(tool_attr_fn_ident)?;
    // modify the the input function
    if fn_item.sig.asyncness.is_some() {
        // 1. remove asyncness from sig
        // 2. make return type: `std::pin::Pin<Box<dyn Future<Output = #ReturnType> + Send + '_>>`
        // 3. make body: { Box::pin(async move { #body }) }
        let new_output = syn::parse2::<ReturnType>({
            match &fn_item.sig.output {
                syn::ReturnType::Default => {
                    quote! { -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> }
                }
                syn::ReturnType::Type(_, ty) => {
                    quote! { -> std::pin::Pin<Box<dyn Future<Output = #ty> + Send + '_>> }
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
        #tool_attr_fn
        #fn_item
    })
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test_trait_tool_macro() -> syn::Result<()> {
        let attr = quote! {
            name = "direct-annotated-tool",
            annotations(title = "Annotated Tool", read_only_hint = true)
        };
        let input = quote! {
            async fn async_method(&self, Parameters(Request { fields }): Parameters<Request>) {
                drop(fields)
            }
        };
        let _input = tool(attr, input)?;

        Ok(())
    }

    #[test]
    fn test_doc_comment_description() -> syn::Result<()> {
        let attr = quote! {}; // No explicit description
        let input = quote! {
            /// This is a test description from doc comments
            /// with multiple lines
            fn test_function(&self) -> Result<(), Error> {
                Ok(())
            }
        };
        let result = tool(attr, input)?;

        // The output should contain the description from doc comments
        let result_str = result.to_string();
        assert!(result_str.contains("This is a test description from doc comments"));
        assert!(result_str.contains("with multiple lines"));

        Ok(())
    }

    #[test]
    fn test_explicit_description_priority() -> syn::Result<()> {
        let attr = quote! {
            description = "Explicit description has priority"
        };
        let input = quote! {
            /// Doc comment description that should be ignored
            fn test_function(&self) -> Result<(), Error> {
                Ok(())
            }
        };
        let result = tool(attr, input)?;

        // The output should contain the explicit description
        let result_str = result.to_string();
        assert!(result_str.contains("Explicit description has priority"));
        Ok(())
    }
}
