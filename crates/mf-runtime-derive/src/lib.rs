use proc_macro::TokenStream;
use quote::quote;
use std::collections::BTreeSet;
use syn::{
    Data, DeriveInput, Fields, LitStr, Path, ext::IdentExt, parse_macro_input, parse_quote,
    spanned::Spanned, visit::Visit,
};

/// Derives port declarations and runtime-owned decoding for a named-field struct.
///
/// Use `#[input(rename = "port-name")]` on a field and
/// `#[input(runtime = "::runtime_alias")]` on the struct when needed.
#[proc_macro_derive(NodeInputs, attributes(input))]
pub fn derive_node_inputs(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(output) => output.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = input.ident;
    let Data::Struct(data) = input.data else {
        return Err(syn::Error::new(
            name.span(),
            "NodeInputs requires a named-field struct",
        ));
    };
    let Fields::Named(fields) = data.fields else {
        return Err(syn::Error::new(
            name.span(),
            "NodeInputs requires a named-field struct",
        ));
    };
    let mut runtime = None;
    for attr in &input.attrs {
        if attr.path().is_ident("input") {
            attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("runtime") {
                    return Err(meta.error("expected `runtime = \"path\"` on the input struct"));
                }
                if runtime.is_some() {
                    return Err(meta.error("duplicate runtime path"));
                }
                let value: LitStr = meta.value()?.parse()?;
                runtime = Some(value.parse::<Path>()?);
                Ok(())
            })?;
        }
    }
    let runtime: Path = runtime.unwrap_or_else(|| parse_quote!(::mf_runtime));
    let mut generics = input.generics;
    let mut ports = Vec::new();
    let mut decoded = Vec::new();
    let mut names = BTreeSet::new();
    for field in fields.named {
        let ident = field.ident.expect("named field");
        let mut renamed = None;
        for attr in &field.attrs {
            if attr.path().is_ident("input") {
                attr.parse_nested_meta(|meta| {
                    if !meta.path.is_ident("rename") {
                        return Err(
                            meta.error("expected `rename = \"port-name\"` on an input field")
                        );
                    }
                    if renamed.is_some() {
                        return Err(meta.error("duplicate input rename"));
                    }
                    renamed = Some(meta.value()?.parse::<LitStr>()?);
                    Ok(())
                })?;
            }
        }
        let port = renamed.unwrap_or_else(|| LitStr::new(&ident.unraw().to_string(), ident.span()));
        if port.value().is_empty() {
            return Err(syn::Error::new(
                port.span(),
                "input port name must not be empty",
            ));
        }
        if !names.insert(port.value()) {
            return Err(syn::Error::new(port.span(), "duplicate input port name"));
        }
        let ty = field.ty;
        let mut borrowed = BorrowedField(None);
        borrowed.visit_type(&ty);
        if let Some(error) = borrowed.0 {
            return Err(error);
        }
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: #runtime::InputField));
        ports.push(quote!(<#ty as #runtime::InputField>::port(#port)));
        decoded.push(quote!(#ident: __mf_decoder.take::<#ty>(#port)?));
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #runtime::NodeInputs for #name #type_generics #where_clause {
            fn ports() -> ::std::vec::Vec<#runtime::PortSpec> {
                ::std::vec![#(#ports),*]
            }

            fn from_inputs(__mf_inputs: #runtime::Inputs) -> ::std::result::Result<Self, #runtime::InputDecodeError> {
                let mut __mf_decoder = #runtime::InputDecoder::new(__mf_inputs);
                let __mf_result = Self { #(#decoded),* };
                __mf_decoder.finish()?;
                ::std::result::Result::Ok(__mf_result)
            }
        }
    })
}

struct BorrowedField(Option<syn::Error>);

impl<'ast> Visit<'ast> for BorrowedField {
    fn visit_type_reference(&mut self, reference: &'ast syn::TypeReference) {
        self.0 = Some(syn::Error::new(
            reference.span(),
            "NodeInputs requires owned input fields; references are unsupported",
        ));
    }
}
