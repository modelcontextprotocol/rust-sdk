use crate::common::{
    AGGREGATED_IDENT, PARAM_IDENT, REQ_IDENT, SCHEMARS_IDENT, SERDE_IDENT, TOOL_IDENT,
};
use proc_macro2::{Ident, TokenStream};
use quote::{ToTokens, quote};
use serde_json::json;
use std::collections::HashSet;
use syn::parse::Parse;
use syn::spanned::Spanned;
use syn::{Expr, FnArg, ItemFn, Lit, MetaList, PatType, Token, Type, Visibility, parse_quote};

/// Stores tool annotation attributes
#[derive(Default, Clone)]
struct ToolAnnotationAttrs(pub serde_json::Map<String, serde_json::Value>);

impl Parse for ToolAnnotationAttrs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut attrs = serde_json::Map::new();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let value: Lit = input.parse()?;
            let value = match value {
                Lit::Str(s) => json!(s.value()),
                Lit::Bool(b) => json!(b.value),
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "annotations must be string or boolean literals",
                    ));
                }
            };
            attrs.insert(key.to_string(), value);
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        Ok(ToolAnnotationAttrs(attrs))
    }
}

#[derive(Default)]
struct ToolFnMetadata {
    name: Option<Expr>,
    description: Option<Expr>,
    vis: Option<Visibility>,
    annotations: Option<ToolAnnotationAttrs>,
}

impl Parse for ToolFnMetadata {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut description = None;
        let mut vis = None;
        let mut annotations = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "name" => {
                    let value: Expr = input.parse()?;
                    name = Some(value);
                }
                "description" => {
                    let value: Expr = input.parse()?;
                    description = Some(value);
                }
                "vis" => {
                    let value: Visibility = input.parse()?;
                    vis = Some(value);
                }
                "annotations" => {
                    // Parse the annotations as a nested structure
                    let content;
                    syn::braced!(content in input);
                    let value = content.parse()?;
                    annotations = Some(value);
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

        Ok(ToolFnMetadata {
            name,
            description,
            vis,
            annotations,
        })
    }
}

struct ToolFnParamAttrs {
    serde_meta: Vec<MetaList>,
    schemars_meta: Vec<MetaList>,
    ident: Ident,
    rust_type: Box<Type>,
}

impl ToTokens for ToolFnParamAttrs {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = &self.ident;
        let rust_type = &self.rust_type;
        let serde_meta = &self.serde_meta;
        let schemars_meta = &self.schemars_meta;
        tokens.extend(quote! {
            #(#[#serde_meta])*
            #(#[#schemars_meta])*
            pub #ident: #rust_type,
        });
    }
}

#[derive(Default)]
enum ToolParams {
    Aggregated {
        rust_type: PatType,
    },
    Params {
        attrs: Vec<ToolFnParamAttrs>,
    },
    #[default]
    NoParam,
}

#[derive(Default)]
struct ToolAttrs {
    fn_metadata: ToolFnMetadata,
    params: ToolParams,
}

pub enum ParamMarker {
    Param,
    Aggregated,
}

impl Parse for ParamMarker {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        match ident.to_string().as_str() {
            PARAM_IDENT => Ok(ParamMarker::Param),
            AGGREGATED_IDENT | REQ_IDENT => Ok(ParamMarker::Aggregated),
            _ => Err(syn::Error::new(ident.span(), "unknown attribute")),
        }
    }
}

pub(crate) fn tool_fn_item(attr: TokenStream, mut input_fn: ItemFn) -> syn::Result<TokenStream> {
    let mut tool_macro_attrs = ToolAttrs::default();
    let tool_metadata: ToolFnMetadata = parse_tool_metadata(attr)?;
    tool_macro_attrs.fn_metadata = tool_metadata;

    let (params, unextractable_args_indexes) = process_function_parameters(&mut input_fn)?;
    tool_macro_attrs.params = params;

    let tool_attr_fn = generate_tool_attr_function(&tool_macro_attrs, &input_fn);

    let tool_call_fn = generate_tool_call_function(
        &mut tool_macro_attrs,
        &input_fn,
        &unextractable_args_indexes,
    );

    Ok(quote! {
        #tool_attr_fn
        #tool_call_fn
        #input_fn
    })
}

fn parse_tool_metadata(attr: TokenStream) -> syn::Result<ToolFnMetadata> {
    syn::parse2(attr)
}

fn process_function_parameters(input_fn: &mut ItemFn) -> syn::Result<(ToolParams, HashSet<usize>)> {
    let mut params = ToolParams::default();
    let mut unextractable_args_indexes = HashSet::new();

    for (index, mut fn_arg) in input_fn.sig.inputs.iter_mut().enumerate() {
        enum Caught {
            Param(ToolFnParamAttrs),
            Aggregated(PatType),
        }
        let mut caught = None;
        match &mut fn_arg {
            FnArg::Receiver(_) => {
                continue;
            }
            FnArg::Typed(pat_type) => {
                let mut serde_metas = Vec::new();
                let mut schemars_metas = Vec::new();
                let mut arg_ident = match pat_type.pat.as_ref() {
                    syn::Pat::Ident(pat_ident) => Some(pat_ident.ident.clone()),
                    _ => None,
                };
                let raw_attrs: Vec<_> = pat_type.attrs.drain(..).collect();
                for attr in raw_attrs {
                    match &attr.meta {
                        syn::Meta::List(meta_list) => {
                            if meta_list.path.is_ident(TOOL_IDENT) {
                                let pat_type = pat_type.clone();
                                let marker = meta_list.parse_args::<ParamMarker>()?;
                                match marker {
                                    ParamMarker::Param => {
                                        let Some(arg_ident) = arg_ident.take() else {
                                            return Err(syn::Error::new(
                                                proc_macro2::Span::call_site(),
                                                "input param must have an ident as name",
                                            ));
                                        };
                                        caught.replace(Caught::Param(ToolFnParamAttrs {
                                            serde_meta: Vec::new(),
                                            schemars_meta: Vec::new(),
                                            ident: arg_ident,
                                            rust_type: pat_type.ty.clone(),
                                        }));
                                    }
                                    ParamMarker::Aggregated => {
                                        caught.replace(Caught::Aggregated(pat_type.clone()));
                                    }
                                }
                            } else if meta_list.path.is_ident(SERDE_IDENT) {
                                serde_metas.push(meta_list.clone());
                            } else if meta_list.path.is_ident(SCHEMARS_IDENT) {
                                schemars_metas.push(meta_list.clone());
                            } else {
                                pat_type.attrs.push(attr);
                            }
                        }
                        _ => {
                            pat_type.attrs.push(attr);
                        }
                    }
                }
                match caught {
                    Some(Caught::Param(mut param)) => {
                        param.serde_meta = serde_metas;
                        param.schemars_meta = schemars_metas;
                        match &mut params {
                            ToolParams::Params { attrs } => {
                                attrs.push(param);
                            }
                            _ => {
                                params = ToolParams::Params { attrs: vec![param] };
                            }
                        }
                        unextractable_args_indexes.insert(index);
                    }
                    Some(Caught::Aggregated(rust_type)) => {
                        if let ToolParams::Params { .. } = params {
                            return Err(syn::Error::new(
                                rust_type.span(),
                                "cannot mix aggregated and individual parameters",
                            ));
                        }
                        params = ToolParams::Aggregated { rust_type };
                        unextractable_args_indexes.insert(index);
                    }
                    None => {}
                }
            }
        }
    }

    Ok((params, unextractable_args_indexes))
}

fn generate_tool_attr_function(tool_macro_attrs: &ToolAttrs, input_fn: &ItemFn) -> TokenStream {
    let name = get_tool_name(&tool_macro_attrs.fn_metadata, &input_fn.sig.ident);
    let description = get_tool_description(&tool_macro_attrs.fn_metadata, &input_fn.attrs);
    let schema = generate_schema(&tool_macro_attrs.params, &input_fn.sig.ident);
    let annotations_code = get_tool_annotations(&tool_macro_attrs.fn_metadata);
    let tool_attr_fn_ident = Ident::new(
        &format!("{}_tool_attr", input_fn.sig.ident),
        proc_macro2::Span::call_site(),
    );

    let input_fn_attrs = &input_fn.attrs;
    let input_fn_vis = &input_fn.vis;

    quote! {
        #(#input_fn_attrs)*
        #input_fn_vis fn #tool_attr_fn_ident() -> rmcp::model::Tool {
            rmcp::model::Tool {
                name: #name.into(),
                description: Some(#description.into()),
                input_schema: #schema.into(),
                annotations: #annotations_code,
            }
        }
    }
}

fn get_tool_name(metadata: &ToolFnMetadata, fn_ident: &Ident) -> Expr {
    match &metadata.name {
        Some(name) => name.clone(),
        None => parse_quote! {
            stringify!(#fn_ident)
        },
    }
}

fn get_tool_description(metadata: &ToolFnMetadata, fn_attrs: &[syn::Attribute]) -> Expr {
    match &metadata.description {
        Some(expr) =>
        // Use explicitly provided description if available
        {
            expr.clone()
        }
        None => {
            // Try to extract documentation comments
            let doc_content = extract_documentation(fn_attrs);
            parse_quote! {
                    #doc_content.trim().to_string()
            }
        }
    }
}

fn extract_documentation(fn_attrs: &[syn::Attribute]) -> String {
    fn_attrs
        .iter()
        .filter_map(extract_doc_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn generate_schema(params: &ToolParams, fn_ident: &Ident) -> TokenStream {
    match params {
        ToolParams::Aggregated { rust_type } => {
            let ty = &rust_type.ty;
            let schema = quote! {
                rmcp::handler::server::tool::cached_schema_for_type::<#ty>()
            };
            schema
        }
        ToolParams::Params { attrs, .. } => {
            let (param_type, temp_param_type_name) =
                create_request_type(attrs, fn_ident.to_string());
            let schema = quote! {
                {
                    #param_type
                    rmcp::handler::server::tool::cached_schema_for_type::<#temp_param_type_name>()
                }
            };
            schema
        }
        ToolParams::NoParam => {
            quote! {
                rmcp::handler::server::tool::cached_schema_for_type::<rmcp::model::EmptyObject>()
            }
        }
    }
}

// todo! - add tests
fn get_tool_annotations(metadata: &ToolFnMetadata) -> TokenStream {
    match &metadata.annotations {
        Some(annotations) => {
            let annotations =
                serde_json::to_string(&annotations.0).expect("failed to serialize annotations");

            quote! {
                Some(serde_json::from_str::<rmcp::model::ToolAnnotations>(&#annotations)
                    .expect("Could not parse tool annotations"))
            }
        }
        // why return None?
        None => quote! { None },
    }
}

fn generate_tool_call_function(
    tool_macro_attrs: &mut ToolAttrs,
    input_fn: &ItemFn,
    unextractable_args_indexes: &HashSet<usize>,
) -> TokenStream {
    let trivial_arg_extraction_part =
        generate_trivial_arg_extraction(input_fn, unextractable_args_indexes);
    let processed_arg_extraction_part =
        generate_parameter_processing(&mut tool_macro_attrs.params, &input_fn.sig.ident);
    let function_call = generate_function_invocation(input_fn);
    let tool_call_fn_ident = create_tool_call_ident(&input_fn.sig.ident);

    let visibility = tool_macro_attrs
        .fn_metadata
        .vis
        .as_ref()
        .unwrap_or(&input_fn.vis);
    let preserved_attrs = &input_fn
        .attrs
        .iter()
        .filter(|attr| !attr.path().is_ident(TOOL_IDENT))
        .collect::<Vec<_>>();

    // Assemble the final wrapper function
    quote! {
        #(#preserved_attrs)*
        #visibility async fn #tool_call_fn_ident(context: rmcp::handler::server::tool::ToolCallContext<'_, Self>)
            -> std::result::Result<rmcp::model::CallToolResult, rmcp::Error> {
            use rmcp::handler::server::tool::*;
            #trivial_arg_extraction_part
            #processed_arg_extraction_part
            #function_call
        }
    }
}

fn generate_trivial_arg_extraction(
    input_fn: &ItemFn,
    unextractable_args_indexes: &HashSet<usize>,
) -> TokenStream {
    let trivial_args = input_fn
        .sig
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            if unextractable_args_indexes.contains(&index) {
                None
            } else {
                // get ident/type pair
                let line = match arg {
                    FnArg::Typed(pat_type) => {
                        let pat = &pat_type.pat;
                        let ty = &pat_type.ty;
                        quote! {
                            let (#pat, context) = <#ty>::from_tool_call_context_part(context)?;
                        }
                    }
                    FnArg::Receiver(r) => {
                        let ty = r.ty.clone();
                        let pat = create_receiver_ident();
                        quote! {
                            let  (#pat, context) = <#ty>::from_tool_call_context_part(context)?;
                        }
                    }
                };
                Some(line)
            }
        });

    quote! {
        #(#trivial_args)*
    }
}

fn generate_parameter_processing(params: &mut ToolParams, fn_ident: &Ident) -> TokenStream {
    match params {
        ToolParams::Aggregated { rust_type } => {
            let PatType { pat, ty, .. } = rust_type;
            quote! {
                let (Parameters(#pat), context) = <Parameters<#ty>>::from_tool_call_context_part(context)?;
            }
        }
        ToolParams::Params { attrs } => {
            let (param_type, temp_param_type_name) =
                create_request_type(attrs, fn_ident.to_string());

            let params_ident = attrs.iter().map(|attr| &attr.ident).collect::<Vec<_>>();
            quote! {
                #param_type
                let (__rmcp_tool_req, context) = rmcp::model::JsonObject::from_tool_call_context_part(context)?;
                let #temp_param_type_name {
                    #(#params_ident,)*
                } = parse_json_object(__rmcp_tool_req)?;
            }
        }
        ToolParams::NoParam => {
            quote! {}
        }
    }
}

fn generate_function_invocation(input_fn: &ItemFn) -> TokenStream {
    let is_async = input_fn.sig.asyncness.is_some();
    let params = &input_fn
        .sig
        .inputs
        .iter()
        .map(|fn_arg| match fn_arg {
            FnArg::Receiver(_) => {
                let pat = create_receiver_ident();
                quote! { #pat }
            }
            FnArg::Typed(pat_type) => {
                let pat = &pat_type.pat.clone();
                quote! { #pat }
            }
        })
        .collect::<Vec<_>>();
    let raw_fn_ident = &input_fn.sig.ident;

    if is_async {
        quote! {
            Self::#raw_fn_ident(#(#params),*).await.into_call_tool_result()
        }
    } else {
        quote! {
            Self::#raw_fn_ident(#(#params),*).into_call_tool_result()
        }
    }
}

// for receiver type, name it as __rmcp_tool_receiver
fn create_receiver_ident() -> Ident {
    Ident::new("__rmcp_tool_receiver", proc_macro2::Span::call_site())
}

/// Creates the tool call function identifier from the original function name
fn create_tool_call_ident(original_ident: &Ident) -> Ident {
    Ident::new(
        &format!("{}_tool_call", original_ident),
        proc_macro2::Span::call_site(),
    )
}

fn create_request_type(attrs: &[ToolFnParamAttrs], tool_name: String) -> (TokenStream, Ident) {
    let pascal_case_tool_name = tool_name.to_ascii_uppercase();
    let temp_param_type_name = Ident::new(
        &format!("__{pascal_case_tool_name}ToolCallParam",),
        proc_macro2::Span::call_site(),
    );
    (
        quote! {
            use rmcp::{serde, schemars};
            #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
            pub struct #temp_param_type_name {
                #(#attrs)*
            }
        },
        temp_param_type_name,
    )
}

// extract doc line from attribute
fn extract_doc_line(attr: &syn::Attribute) -> Option<String> {
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

    (!content.is_empty()).then_some(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn test_parse_tool_metadata() {
        // Arrange - Prepare input
        let input = quote! {
            name = "calculator",
            description = "A simple calculator tool",
            annotations = {
                category: "math",
                required: true
            }
        };

        // Act - Parse the metadata
        let result = parse_tool_metadata(input).unwrap();

        // Assert
        assert_eq!(
            result.name.unwrap().to_token_stream().to_string(),
            "\"calculator\"".to_string()
        );
        assert_eq!(
            result.description.unwrap().to_token_stream().to_string(),
            "\"A simple calculator tool\"".to_string()
        );
        let annotations = result.annotations.unwrap().0;
        assert_eq!(annotations.get("category").unwrap(), &json!("math"));
        assert_eq!(annotations.get("required").unwrap(), &json!(true));
        assert!(result.vis.is_none());
    }

    #[test]
    fn test_generate_tool_attr_function() {
        // Arrange - Use the actual parsing functions
        let attr_input: TokenStream = quote! {
            name = "multiply_numbers",
            description = "Multiplies two floating point numbers"
        };

        let mut input_fn: ItemFn = parse_quote! {
            #[doc = "Multiplies two numbers together"]
            pub fn multiply(
                #[tool(param)] x: f64,
                #[tool(param)] y: f64
            ) -> f64 {
                x * y
            }
        };

        // Use the actual parsing functions like the real macro does
        let fn_metadata = parse_tool_metadata(attr_input).unwrap();
        let (params, _) = process_function_parameters(&mut input_fn).unwrap();

        let tool_attrs = ToolAttrs {
            fn_metadata,
            params,
        };

        // Expected output
        let expected_output = quote! {
            #[doc = "Multiplies two numbers together"]
            pub fn multiply_tool_attr() -> rmcp::model::Tool {
                rmcp::model::Tool {
                    name: "multiply_numbers".into(),
                    description: Some("Multiplies two floating point numbers".into()),
                    input_schema: {
                        use rmcp::{serde, schemars};
                        #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
                        pub struct __MULTIPLYToolCallParam {
                            pub x: f64,
                            pub y: f64,
                        }
                        rmcp::handler::server::tool::cached_schema_for_type::<__MULTIPLYToolCallParam>()
                    }.into(),
                    annotations: None,
                }
            }
        };

        // Act - Generate the tool attr function
        let actual_output = generate_tool_attr_function(&tool_attrs, &input_fn);

        // Assert - Compare the string representations
        let expected_str = expected_output.to_string();
        let actual_str = actual_output.to_string();

        assert_eq!(
            expected_str, actual_str,
            "Generated tool attr function doesn't match expected output.\nExpected:\n{}\n\nActual:\n{}",
            expected_str, actual_str
        );
    }

    #[test]
    fn test_get_tool_name() {
        // Test case 1: When metadata has an explicit name
        let explicit_name_metadata = ToolFnMetadata {
            name: Some(parse_quote! { "file_search" }),
            description: None,
            vis: None,
            annotations: None,
        };
        let fn_ident = Ident::new("search_files_in_directory", proc_macro2::Span::call_site());

        let result_with_explicit_name = get_tool_name(&explicit_name_metadata, &fn_ident);
        let expected_explicit: Expr = parse_quote! { "file_search" };

        assert_eq!(
            result_with_explicit_name.to_token_stream().to_string(),
            expected_explicit.to_token_stream().to_string(),
            "Should use explicit name when provided"
        );

        // Test case 2: When metadata has no name (fallback to function name)
        let no_name_metadata = ToolFnMetadata {
            name: None,
            description: None,
            vis: None,
            annotations: None,
        };
        let fn_ident = Ident::new("calculate_monthly_budget", proc_macro2::Span::call_site());

        let result_with_fallback = get_tool_name(&no_name_metadata, &fn_ident);
        let expected_fallback: Expr = parse_quote! { stringify!(calculate_monthly_budget) };

        assert_eq!(
            result_with_fallback.to_token_stream().to_string(),
            expected_fallback.to_token_stream().to_string(),
            "Should fallback to stringify function name when no explicit name provided"
        );
    }

    #[test]
    fn test_get_tool_description() {
        // Test case 1: When metadata has an explicit description
        let explicit_description_metadata = ToolFnMetadata {
            name: None,
            description: Some(
                parse_quote! { "Searches for files matching the specified pattern in a directory tree" },
            ),
            vis: None,
            annotations: None,
        };
        let empty_attrs = vec![];

        let result_with_explicit_description =
            get_tool_description(&explicit_description_metadata, &empty_attrs);
        let expected_explicit: Expr = parse_quote! { "Searches for files matching the specified pattern in a directory tree" };

        assert_eq!(
            result_with_explicit_description
                .to_token_stream()
                .to_string(),
            expected_explicit.to_token_stream().to_string(),
            "Should use explicit description when provided"
        );

        // Test case 2: When metadata has no description (fallback to doc comments)
        let no_description_metadata = ToolFnMetadata {
            name: None,
            description: None,
            vis: None,
            annotations: None,
        };

        // Create function attributes with doc comments (simulating /// comments)
        let doc_attrs: Vec<syn::Attribute> = vec![
            parse_quote! { #[doc = " Calculates the total monthly budget based on income and expenses."] },
            parse_quote! { #[doc = " Returns the remaining budget after all deductions."] },
        ];

        let result_with_fallback = get_tool_description(&no_description_metadata, &doc_attrs);
        let expected_fallback: Expr = parse_quote! {
            "Calculates the total monthly budget based on income and expenses.\nReturns the remaining budget after all deductions.".trim().to_string()
        };

        assert_eq!(
            result_with_fallback.to_token_stream().to_string(),
            expected_fallback.to_token_stream().to_string(),
            "Should fallback to extracted doc comments when no explicit description provided"
        );
    }

    #[test]
    fn test_generate_schema() {
        let fn_ident = Ident::new("process_data", proc_macro2::Span::call_site());

        // Test case 1: Aggregated params (uses a single aggregated type)
        let aggregated_params = ToolParams::Aggregated {
            rust_type: parse_quote! { request: DatabaseQuery },
        };

        let result_aggregated = generate_schema(&aggregated_params, &fn_ident);
        let expected_aggregated = quote! {
            rmcp::handler::server::tool::cached_schema_for_type::<DatabaseQuery>()
        };

        assert_eq!(
            result_aggregated.to_string(),
            expected_aggregated.to_string(),
            "Should generate schema for aggregated type directly"
        );

        // Test case 2: Individual params (creates a struct from parameters)
        let individual_params = ToolParams::Params {
            attrs: vec![
                ToolFnParamAttrs {
                    serde_meta: vec![],
                    schemars_meta: vec![],
                    ident: Ident::new("user_id", proc_macro2::Span::call_site()),
                    rust_type: Box::new(parse_quote! { u64 }),
                },
                ToolFnParamAttrs {
                    serde_meta: vec![],
                    schemars_meta: vec![],
                    ident: Ident::new("file_path", proc_macro2::Span::call_site()),
                    rust_type: Box::new(parse_quote! { String }),
                },
            ],
        };

        let result_params = generate_schema(&individual_params, &fn_ident);
        let expected_params = quote! {
            {
                use rmcp::{serde, schemars};
                #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
                pub struct __PROCESS_DATAToolCallParam {
                    pub user_id: u64,
                    pub file_path: String,
                }
                rmcp::handler::server::tool::cached_schema_for_type::<__PROCESS_DATAToolCallParam>()
            }
        };

        assert_eq!(
            result_params.to_string(),
            expected_params.to_string(),
            "Should generate schema for individual parameters by creating a struct"
        );

        // Test case 3: No params (uses EmptyObject)
        let no_params = ToolParams::NoParam;

        let result_no_params = generate_schema(&no_params, &fn_ident);
        let expected_no_params = quote! {
            rmcp::handler::server::tool::cached_schema_for_type::<rmcp::model::EmptyObject>()
        };

        assert_eq!(
            result_no_params.to_string(),
            expected_no_params.to_string(),
            "Should generate schema for EmptyObject when no parameters"
        );
    }

    #[test]
    fn test_generate_tool_call_function() {
        // Arrange - An async function with parameters
        let mut input_fn: ItemFn = parse_quote! {
            #[doc = "Processes user data and generates a report"]
            pub async fn process_user_data(
                #[tool(param)] user_id: u64,
                #[tool(param)] report_type: String
            ) -> Result<String, ProcessingError> {
                // Implementation would go here
                Ok(format!("Report for user {}: {}", user_id, report_type))
            }
        };

        // Parse the function parameters using the actual parsing logic
        let (params, unextractable_args_indexes) =
            process_function_parameters(&mut input_fn).unwrap();

        let mut tool_attrs = ToolAttrs {
            fn_metadata: ToolFnMetadata {
                name: Some(parse_quote! { "user_report_generator" }),
                description: Some(
                    parse_quote! { "Generates reports for users based on their data" },
                ),
                vis: Some(parse_quote! { pub }),
                annotations: None,
            },
            params,
        };

        // Expected output - what the generated tool call function should look like
        let expected_output = quote! {
            #[doc = "Processes user data and generates a report"]
            pub async fn process_user_data_tool_call(context: rmcp::handler::server::tool::ToolCallContext<'_, Self>)
                -> std::result::Result<rmcp::model::CallToolResult, rmcp::Error> {
                use rmcp::handler::server::tool::*;

                use rmcp::{serde, schemars};
                #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
                pub struct __PROCESS_USER_DATAToolCallParam {
                    pub user_id: u64,
                    pub report_type: String,
                }
                let (__rmcp_tool_req, context) = rmcp::model::JsonObject::from_tool_call_context_part(context)?;
                let __PROCESS_USER_DATAToolCallParam {
                    user_id,
                    report_type,
                } = parse_json_object(__rmcp_tool_req)?;

                Self::process_user_data(user_id, report_type).await.into_call_tool_result()
            }
        };

        // Act - Generate the tool call function
        let actual_output =
            generate_tool_call_function(&mut tool_attrs, &input_fn, &unextractable_args_indexes);

        // Assert
        let expected_str = expected_output.to_string();
        let actual_str = actual_output.to_string();

        assert_eq!(
            expected_str, actual_str,
            "Generated tool call function doesn't match expected output.\nExpected:\n{}\n\nActual:\n{}",
            expected_str, actual_str
        );
    }

    #[test]
    fn test_generate_tool_call_function_no_params() {
        // Arrange - Create a function with no tool parameters
        let mut input_fn: ItemFn = parse_quote! {
            pub async fn get_system_status() -> SystemStatus {
                SystemStatus::new()
            }
        };

        let (params, unextractable_args_indexes) =
            process_function_parameters(&mut input_fn).unwrap();

        let mut tool_attrs = ToolAttrs {
            fn_metadata: ToolFnMetadata::default(),
            params,
        };

        // Expected output for no parameters
        let expected_output = quote! {
            pub async fn get_system_status_tool_call(context: rmcp::handler::server::tool::ToolCallContext<'_, Self>)
                -> std::result::Result<rmcp::model::CallToolResult, rmcp::Error> {
                use rmcp::handler::server::tool::*;

                Self::get_system_status().await.into_call_tool_result()
            }
        };

        // Act
        let actual_output =
            generate_tool_call_function(&mut tool_attrs, &input_fn, &unextractable_args_indexes);

        // Assert
        assert_eq!(
            expected_output.to_string(),
            actual_output.to_string(),
            "Generated tool call function for no params doesn't match expected output"
        );
    }

    #[test]
    fn test_generate_tool_call_function_sync() {
        // Arrange - A synchronous function
        let mut input_fn: ItemFn = parse_quote! {
            pub fn calculate_tax(
                #[tool(param)] income: f64,
                #[tool(param)] rate: f64
            ) -> f64 {
                income * rate
            }
        };

        let (params, unextractable_args_indexes) =
            process_function_parameters(&mut input_fn).unwrap();

        let mut tool_attrs = ToolAttrs {
            fn_metadata: ToolFnMetadata::default(),
            params,
        };

        // Expected output for sync function (note: no .await)
        let expected_output = quote! {
            pub async fn calculate_tax_tool_call(context: rmcp::handler::server::tool::ToolCallContext<'_, Self>)
                -> std::result::Result<rmcp::model::CallToolResult, rmcp::Error> {
                use rmcp::handler::server::tool::*;

                use rmcp::{serde, schemars};
                #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
                pub struct __CALCULATE_TAXToolCallParam {
                    pub income: f64,
                    pub rate: f64,
                }
                let (__rmcp_tool_req, context) = rmcp::model::JsonObject::from_tool_call_context_part(context)?;
                let __CALCULATE_TAXToolCallParam {
                    income,
                    rate,
                } = parse_json_object(__rmcp_tool_req)?;

                Self::calculate_tax(income, rate).into_call_tool_result()
            }
        };

        // Act
        let actual_output =
            generate_tool_call_function(&mut tool_attrs, &input_fn, &unextractable_args_indexes);

        // Assert
        assert_eq!(
            expected_output.to_string(),
            actual_output.to_string(),
            "Generated tool call function for sync function doesn't match expected output"
        );
    }
}
