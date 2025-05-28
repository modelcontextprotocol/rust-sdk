#[allow(unused_imports)]
use proc_macro::TokenStream;

mod common;
mod fn_handler;
mod impl_bloc_handler;
mod tool;

#[proc_macro_attribute]
pub fn tool(attr: TokenStream, input: TokenStream) -> TokenStream {
    tool::tool(attr.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
