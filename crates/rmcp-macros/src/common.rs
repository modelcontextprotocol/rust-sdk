//! Common utilities shared between different macro implementations

use quote::quote;
use syn::{Attribute, Expr};

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

