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
    match expand(input, Direction::Input) {
        Ok(output) => output.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

/// Derives port declarations and runtime-owned encoding for a named-field struct.
///
/// Use `#[output(rename = "port-name")]` on a field and
/// `#[output(runtime = "::runtime_alias")]` on the struct when needed.
#[proc_macro_derive(NodeOutputs, attributes(output))]
pub fn derive_node_outputs(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input, Direction::Output) {
        Ok(output) => output.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Input,
    Output,
}

impl Direction {
    fn attribute(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }

    fn derive_name(self) -> &'static str {
        match self {
            Self::Input => "NodeInputs",
            Self::Output => "NodeOutputs",
        }
    }
}

fn expand(input: DeriveInput, direction: Direction) -> syn::Result<proc_macro2::TokenStream> {
    let attribute = direction.attribute();
    let derive_name = direction.derive_name();
    let name = input.ident;
    let Data::Struct(data) = input.data else {
        return Err(syn::Error::new(
            name.span(),
            format!("{derive_name} requires a named-field struct"),
        ));
    };
    let Fields::Named(fields) = data.fields else {
        return Err(syn::Error::new(
            name.span(),
            format!("{derive_name} requires a named-field struct"),
        ));
    };
    let mut runtime = None;
    for attr in &input.attrs {
        if attr.path().is_ident(attribute) {
            attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("runtime") {
                    return Err(meta.error(format!(
                        "expected `runtime = \"path\"` on the {attribute} struct"
                    )));
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
    let mut converted = Vec::new();
    let mut names = BTreeSet::new();
    for field in fields.named {
        let ident = field.ident.expect("named field");
        let mut renamed = None;
        for attr in &field.attrs {
            if attr.path().is_ident(attribute) {
                attr.parse_nested_meta(|meta| {
                    if !meta.path.is_ident("rename") {
                        return Err(meta.error(format!(
                            "expected `rename = \"port-name\"` on an {attribute} field"
                        )));
                    }
                    if renamed.is_some() {
                        return Err(meta.error(format!("duplicate {attribute} rename")));
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
                format!("{attribute} port name must not be empty"),
            ));
        }
        if !names.insert(port.value()) {
            return Err(syn::Error::new(
                port.span(),
                format!("duplicate {attribute} port name"),
            ));
        }
        let ty = field.ty;
        let mut borrowed = BorrowedField {
            error: None,
            derive_name,
            attribute,
        };
        borrowed.visit_type(&ty);
        if let Some(error) = borrowed.error {
            return Err(error);
        }
        let field_trait = match direction {
            Direction::Input => quote!(#runtime::InputField),
            Direction::Output => quote!(#runtime::OutputField),
        };
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: #field_trait));
        ports.push(quote!(<#ty as #field_trait>::port(#port)));
        converted.push(match direction {
            Direction::Input => {
                quote!(#ident: #runtime::decode_input::<#ty>(&mut __mf_inputs, #port)?)
            }
            Direction::Output => {
                quote!(#runtime::encode_output::<#ty>(&mut __mf_outputs, #port, self.#ident)?;)
            }
        });
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let (contract, conversion) = match direction {
        Direction::Input => (
            quote!(#runtime::NodeInputs),
            quote! {
                fn from_inputs(mut __mf_inputs: #runtime::Inputs) -> ::std::result::Result<Self, #runtime::InputDecodeError> {
                    let __mf_result = Self { #(#converted),* };
                    #runtime::reject_unknown_inputs(__mf_inputs)?;
                    ::std::result::Result::Ok(__mf_result)
                }
            },
        ),
        Direction::Output => (
            quote!(#runtime::NodeOutputs),
            quote! {
                fn into_outputs(self) -> ::std::result::Result<#runtime::Outputs, #runtime::OutputEncodeError> {
                    let mut __mf_outputs = #runtime::Outputs::new();
                    #(#converted)*
                    ::std::result::Result::Ok(__mf_outputs)
                }
            },
        ),
    };
    Ok(quote! {
        impl #impl_generics #contract for #name #type_generics #where_clause {
            fn ports() -> ::std::vec::Vec<#runtime::PortSpec> {
                ::std::vec![#(#ports),*]
            }

            #conversion
        }
    })
}

struct BorrowedField {
    error: Option<syn::Error>,
    derive_name: &'static str,
    attribute: &'static str,
}

impl<'ast> Visit<'ast> for BorrowedField {
    fn visit_type_reference(&mut self, reference: &'ast syn::TypeReference) {
        self.error = Some(syn::Error::new(
            reference.span(),
            format!(
                "{} requires owned {} fields; references are unsupported",
                self.derive_name, self.attribute
            ),
        ));
    }
}
