use crate::fn_handler;
use crate::impl_bloc_handler;
use proc_macro2::TokenStream;
use syn::{ItemFn, ItemImpl, Token, parse::Parse};

pub enum ToolItem {
    Fn(ItemFn),
    Impl(ItemImpl),
}

impl Parse for ToolItem {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let lookahead = input.lookahead1();
        if lookahead.peek(Token![impl]) {
            let item = input.parse::<ItemImpl>()?;
            Ok(ToolItem::Impl(item))
        } else {
            let item = input.parse::<ItemFn>()?;
            Ok(ToolItem::Fn(item))
        }
    }
}

// dispatch impl function item and impl block item
pub(crate) fn tool(attr: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let tool_item = syn::parse2::<ToolItem>(input)?;
    match tool_item {
        ToolItem::Fn(item) => fn_handler::tool_fn_item(attr, item),
        ToolItem::Impl(item) => impl_bloc_handler::tool_impl_item(attr, item),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use quote::quote;
    use syn::parse_quote;

    #[test]
    fn test_basic_tool_function_generation() {
        // Input: Create a basic tool function with two parameters
        let attr_input: TokenStream = quote! {
            name = "add_numbers",
            description = "Adds two numbers together"
        };

        let fn_input: TokenStream = parse_quote! {
            #[doc = "Adds two numbers together"]
            pub async fn add(
                #[tool(param)] a: i32,
                #[tool(param)] b: i32
            ) -> Result<i32, rmcp::Error> {
                Ok(a + b)
            }
        };

        // Arrange - Create expected output
        let expected_output = quote! {
            #[doc = "Adds two numbers together"]
            pub fn add_tool_attr() -> rmcp::model::Tool {
                rmcp::model::Tool {
                    name: "add_numbers".into(),
                    description: Some("Adds two numbers together".into()),
                    input_schema: {
                        use rmcp::{serde, schemars};
                        #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
                        pub struct __ADDToolCallParam {
                            pub a: i32,
                            pub b: i32,
                        }
                        rmcp::handler::server::tool::cached_schema_for_type::<__ADDToolCallParam>()
                    }
                        .into(),
                    annotations: None,
                }
            }

            #[doc = "Adds two numbers together"]
            pub async fn add_tool_call(
                context: rmcp::handler::server::tool::ToolCallContext<'_, Self>
            ) -> std::result::Result<rmcp::model::CallToolResult, rmcp::Error> {
                use rmcp::handler::server::tool::*;
                use rmcp::{serde, schemars};
                #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
                pub struct __ADDToolCallParam {
                    pub a: i32,
                    pub b: i32,
                }
                let (__rmcp_tool_req, context) = rmcp::model::JsonObject::from_tool_call_context_part(
                    context
                )?;
                let __ADDToolCallParam { a, b, } = parse_json_object(__rmcp_tool_req)?;
                Self::add(a, b).await.into_call_tool_result()
            }

            #[doc = "Adds two numbers together"]
            pub async fn add(a: i32, b: i32) -> Result<i32, rmcp::Error> {
                Ok(a + b)
            }
        };

        // Act - Generate the actual output
        let actual_output = tool(attr_input, fn_input).unwrap();

        // Assert - Compare the string representations
        let expected_str = expected_output.to_string();
        let actual_str = actual_output.to_string();

        // Compare the results
        assert_eq!(
            expected_str, actual_str,
            "Generated code doesn't match expected output.\nExpected:\n{}\n\nActual:\n{}",
            expected_str, actual_str
        );
    }

    #[test]
    fn test_basic_tool_impl_generation() {
        // Input: Create a basic tool impl block with the tool_box attribute
        let attr_input: TokenStream = quote! {
            tool_box = Calculator
        };

        let impl_input: TokenStream = quote! {
            impl Calculator {
                #[tool(name = "add_numbers", description = "Adds two numbers together")]
                pub async fn add(
                    &self,
                    #[tool(param)] a: i32,
                    #[tool(param)] b: i32
                ) -> Result<i32, rmcp::Error> {
                    Ok(a + b)
                }

                #[tool(name = "multiply_numbers", description = "Multiplies two numbers together")]
                pub async fn multiply(
                    &self,
                    #[tool(param)] x: f64,
                    #[tool(param)] y: f64
                ) -> Result<f64, rmcp::Error> {
                    Ok(x * y)
                }
            }
        };

        // Expected output: impl block with tool_box! macro call
        let expected_output = quote! {
            impl Calculator {
                #[tool(name = "add_numbers", description = "Adds two numbers together")]
                pub async fn add(
                    &self,
                    #[tool(param)] a: i32,
                    #[tool(param)] b: i32
                ) -> Result<i32, rmcp::Error> {
                    Ok(a + b)
                }

                #[tool(name = "multiply_numbers", description = "Multiplies two numbers together")]
                pub async fn multiply(
                    &self,
                    #[tool(param)] x: f64,
                    #[tool(param)] y: f64
                ) -> Result<f64, rmcp::Error> {
                    Ok(x * y)
                }

                rmcp::tool_box!(Calculator {
                    add,
                    multiply
                } Calculator);
            }
        };

        // Act
        let actual_output = tool(attr_input, impl_input).unwrap();

        // Assert
        let expected_str = expected_output.to_string();
        let actual_str = actual_output.to_string();

        assert_eq!(
            expected_str, actual_str,
            "Generated impl block doesn't match expected output.\nExpected:\n{}\n\nActual:\n{}",
            expected_str, actual_str
        );
    }

    #[test]
    fn test_tool_sync_macro() -> syn::Result<()> {
        let attr = quote! {
            name = "test_tool",
            description = "test tool",
            vis =
        };
        let input = quote! {
            fn sum(&self, #[tool(aggr)] req: StructRequest) -> Result<CallToolResult, McpError> {
                Ok(CallToolResult::success(vec![Content::text((req.a + req.b).to_string())]))
            }
        };
        let input = tool(attr, input)?;

        println!("input: {:#}", input);
        Ok(())
    }

    #[test]
    fn test_trait_tool_macro() -> syn::Result<()> {
        let attr = quote! {
            tool_box = Calculator
        };
        let input = quote! {
            impl ServerHandler for Calculator {
                #[tool]
                fn get_info(&self) -> ServerInfo {
                    ServerInfo {
                        instructions: Some("A simple calculator".into()),
                        ..Default::default()
                    }
                }
            }
        };
        let input = tool(attr, input)?;

        println!("input: {:#}", input);
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
