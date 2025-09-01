//! Common utilities shared between different macro implementations

use quote::quote;
use syn::{Attribute, Expr, FnArg, ImplItemFn, Signature, Type};

/// Parse a None expression
pub fn none_expr() -> syn::Result<Expr> {
    syn::parse2::<Expr>(quote! { None })
}

/// Extract documentation from doc attributes
pub fn extract_doc_line(existing_docs: Option<String>, attr: &Attribute) -> Option<String> {
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

/// Find Parameters<T> type in function signature
/// Returns the full Parameters<T> type if found
pub fn find_parameters_type_in_sig(sig: &Signature) -> Option<Box<Type>> {
    sig.inputs.iter().find_map(|input| {
        if let FnArg::Typed(pat_type) = input {
            if let Type::Path(type_path) = &*pat_type.ty {
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
    })
}

/// Find Parameters<T> type in ImplItemFn
pub fn find_parameters_type_impl(fn_item: &ImplItemFn) -> Option<Box<Type>> {
    find_parameters_type_in_sig(&fn_item.sig)
}
