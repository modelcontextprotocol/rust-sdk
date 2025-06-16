#[allow(unused_imports)]
use proc_macro::TokenStream;

// mod tool_inherite;
mod tool;
mod tool_router;
// #[proc_macro_attribute]
// pub fn tool(attr: TokenStream, input: TokenStream) -> TokenStream {
//     tool_inherite::tool(attr.into(), input.into())
//         .unwrap_or_else(|err| err.to_compile_error())
//         .into()
// }
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, input: TokenStream) -> TokenStream {
    tool::tool(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[proc_macro_attribute]
pub fn tool_router(attr: TokenStream, input: TokenStream) -> TokenStream {
    tool_router::tool_router(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
