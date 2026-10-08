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

/// Derives one bidirectional contract for an owned named-port struct.
#[proc_macro_derive(NodeValue, attributes(value))]
pub fn derive_node_value(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input, Direction::Value) {
        Ok(output) => output.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Input,
    Output,
    Value,
}

impl Direction {
    fn attribute(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Value => "value",
        }
    }

    fn derive_name(self) -> &'static str {
        match self {
            Self::Input => "NodeInputs",
            Self::Output => "NodeOutputs",
            Self::Value => "NodeValue",
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
    let mut typed = false;
    for attr in &input.attrs {
        if attr.path().is_ident(attribute) {
            attr.parse_nested_meta(|meta| {
                if matches!(direction, Direction::Value) && meta.path.is_ident("typed") {
                    if typed {
                        return Err(meta.error("duplicate typed generation marker"));
                    }
                    typed = true;
                    return Ok(());
                }
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
    let mut decoded = Vec::new();
    let mut encoded = Vec::new();
    let mut names = BTreeSet::new();
    let mut field_types = Vec::new();
    let mut field_names = Vec::new();
    let mut typed_ports = Vec::new();
    let mut typed_checks = Vec::new();
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
        field_types.push(ty.clone());
        field_names.push(ident.clone());
        if typed {
            typed_ports.push(quote!(#runtime::TypedPort {
                port: <#ty as #runtime::InputField>::port(#port),
                rust_type: <#ty as #runtime::TypedField>::rust_type(),
            }));
            typed_checks.push(
                quote!(if #runtime::validate_typed_output(&self.#ident, #port)? {
                    __mf_present.push(#port);
                }),
            );
        }
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
            Direction::Input | Direction::Value => quote!(#runtime::InputField),
            Direction::Output => quote!(#runtime::OutputField),
        };
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: #field_trait));
        if matches!(direction, Direction::Value) {
            generics
                .make_where_clause()
                .predicates
                .push(parse_quote!(#ty: #runtime::OutputField));
        }
        if typed {
            generics
                .make_where_clause()
                .predicates
                .push(parse_quote!(#ty: #runtime::TypedField));
        }
        ports.push(quote!(<#ty as #field_trait>::port(#port)));
        decoded.push(quote!(#ident: #runtime::decode_input::<#ty>(&mut __mf_inputs, #port)?));
        encoded
            .push(quote!(#runtime::encode_output::<#ty>(&mut __mf_outputs, #port, self.#ident)?;));
        converted.push(match direction {
            Direction::Input => {
                quote!(#ident: #runtime::decode_input::<#ty>(&mut __mf_inputs, #port)?)
            }
            Direction::Output => {
                quote!(#runtime::encode_output::<#ty>(&mut __mf_outputs, #port, self.#ident)?;)
            }
            Direction::Value => quote!(),
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
        Direction::Value => (
            quote!(#runtime::NodeValue),
            quote! {
                fn from_values(mut __mf_inputs: #runtime::NodeValues) -> ::std::result::Result<Self, #runtime::InputDecodeError> {
                    let __mf_result = Self { #(#decoded),* };
                    #runtime::reject_unknown_inputs(__mf_inputs)?;
                    Ok(__mf_result)
                }
                fn into_values(self) -> ::std::result::Result<#runtime::NodeValues, #runtime::OutputEncodeError> {
                    let mut __mf_outputs = #runtime::NodeValues::new();
                    #(#encoded)*
                    Ok(__mf_outputs)
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
    let bridges = if matches!(direction, Direction::Value) {
        quote! {
            impl #impl_generics #runtime::NodeInputs for #name #type_generics #where_clause {
                fn ports() -> ::std::vec::Vec<#runtime::PortSpec> { <Self as #runtime::NodeValue>::ports() }
                fn from_inputs(values: #runtime::Inputs) -> ::std::result::Result<Self, #runtime::InputDecodeError> {
                    <Self as #runtime::NodeValue>::from_values(values)
                }
            }
            impl #impl_generics #runtime::NodeOutputs for #name #type_generics #where_clause {
                fn ports() -> ::std::vec::Vec<#runtime::PortSpec> { <Self as #runtime::NodeValue>::ports() }
                fn into_outputs(self) -> ::std::result::Result<#runtime::Outputs, #runtime::OutputEncodeError> {
                    <Self as #runtime::NodeValue>::into_values(self)
                }
            }
        }
    } else {
        quote!()
    };
    let into_fields = if field_names.is_empty() {
        quote!()
    } else {
        quote!((#(self.#field_names,)*))
    };
    let typed_impl = if typed {
        quote! {
            impl #impl_generics #runtime::TypedNodeValue for #name #type_generics #where_clause {
                type Fields = (#(#field_types,)*);
                fn typed_ports() -> ::std::vec::Vec<#runtime::TypedPort> { vec![#(#typed_ports),*] }
                fn into_fields(self) -> Self::Fields { #into_fields }
                fn from_fields(fields: Self::Fields) -> Self {
                    let (#(#field_names,)*) = fields;
                    Self { #(#field_names),* }
                }
                fn validate_typed(&self) -> ::std::result::Result<::std::vec::Vec<&'static str>, #runtime::OutputEncodeError> {
                    let mut __mf_present = vec![];
                    #(#typed_checks)*
                    Ok(__mf_present)
                }
            }
        }
    } else {
        quote!()
    };
    Ok(quote! {
        #bridges
        #typed_impl
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
