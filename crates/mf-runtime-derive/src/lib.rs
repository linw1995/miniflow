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

/// Derives bidirectional string or internally tagged enum value codecs.
///
/// Unit variants encode as strings. `#[value(tag = "kind")]` encodes unit or
/// named-field variants as objects. Rename variants/fields with `#[value(rename = "wire-name")]`.
#[proc_macro_derive(NodeEnum, attributes(value))]
pub fn derive_node_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_enum(input) {
        Ok(output) => output.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

enum FieldDefault {
    Trait,
    Function(Path),
}

fn field_attributes(
    attrs: &[syn::Attribute],
    attribute: &str,
    allow_default: bool,
) -> syn::Result<(Option<LitStr>, Option<FieldDefault>)> {
    let mut renamed = None;
    let mut default = None;
    for attr in attrs {
        if attr.path().is_ident(attribute) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    if renamed.is_some() {
                        return Err(meta.error(format!("duplicate {attribute} rename")));
                    }
                    renamed = Some(meta.value()?.parse::<LitStr>()?);
                } else if allow_default && meta.path.is_ident("default") {
                    if default.is_some() {
                        return Err(meta.error("duplicate field default"));
                    }
                    default = Some(if meta.input.peek(syn::Token![=]) {
                        let path: LitStr = meta.value()?.parse()?;
                        FieldDefault::Function(path.parse()?)
                    } else {
                        FieldDefault::Trait
                    });
                } else {
                    return Err(meta.error(format!(
                        "expected `rename = \"port-name\"`{} on an {attribute} field",
                        if allow_default { " or `default`" } else { "" }
                    )));
                }
                Ok(())
            })?;
        }
    }
    Ok((renamed, default))
}

fn default_tokens(default: &FieldDefault, ty: &syn::Type) -> proc_macro2::TokenStream {
    match default {
        FieldDefault::Trait => quote!(<#ty as ::std::default::Default>::default),
        FieldDefault::Function(path) => quote!(#path),
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
    let mut decoded = Vec::new();
    let mut encoded = Vec::new();
    let mut names = BTreeSet::new();
    let mut field_types = Vec::new();
    let mut field_names = Vec::new();
    let mut typed_ports = Vec::new();
    let mut field_ports = Vec::new();
    let mut typed_checks = Vec::new();
    for field in fields.named {
        let ident = field.ident.expect("named field");
        let (renamed, default) = field_attributes(
            &field.attrs,
            attribute,
            !matches!(direction, Direction::Output),
        )?;
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
        field_ports.push(port.clone());
        let ty = field.ty;
        field_types.push(ty.clone());
        field_names.push(ident.clone());
        if typed {
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
        let port_spec = if default.is_some() {
            quote!(#runtime::PortSpec::new(#port, <#ty as #field_trait>::value_type(), false))
        } else {
            quote!(<#ty as #field_trait>::port(#port))
        };
        ports.push(port_spec.clone());
        if typed {
            let rust_type = quote!(<#ty as #runtime::TypedField>::rust_type());
            let rust_type = if default.is_some() {
                quote!(#runtime::RustValueType::Defaulted(::std::boxed::Box::new(#rust_type)))
            } else {
                rust_type
            };
            typed_ports
                .push(quote!(#runtime::TypedPort { port: #port_spec, rust_type: #rust_type }));
        }
        if let Some(default) = default {
            if matches!(default, FieldDefault::Trait) {
                generics
                    .make_where_clause()
                    .predicates
                    .push(parse_quote!(#ty: ::std::default::Default));
            }
            let default = default_tokens(&default, &ty);
            decoded.push(quote!(#ident: #runtime::decode_default_input::<#ty>(&mut __mf_inputs, #port, #default)?));
        } else {
            decoded.push(quote!(#ident: #runtime::decode_input::<#ty>(&mut __mf_inputs, #port)?));
        }
        encoded
            .push(quote!(#runtime::encode_output::<#ty>(&mut __mf_outputs, #port, self.#ident)?;));
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let (contract, conversion) = match direction {
        Direction::Input => (
            quote!(#runtime::NodeInputs),
            quote! {
                fn from_inputs(mut __mf_inputs: #runtime::Inputs) -> ::std::result::Result<Self, #runtime::InputDecodeError> {
                    let __mf_result = Self { #(#decoded),* };
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
                    #(#encoded)*
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
            impl #impl_generics #runtime::InputValue for #name #type_generics #where_clause {
                fn value_type() -> #runtime::ValueType { #runtime::ValueType::Object }
                fn decode(value: #runtime::ValueRef) -> ::std::result::Result<Self, #runtime::TypeMismatch> {
                    #runtime::decode_node_value(value)
                }
            }
            impl #impl_generics #runtime::InputField for #name #type_generics #where_clause {
                const REQUIRED: bool = true;
                fn value_type() -> #runtime::ValueType { #runtime::ValueType::Object }
                fn decode_field(port: &str, value: ::std::option::Option<#runtime::ValueRef>) -> ::std::result::Result<Self, #runtime::InputDecodeError> {
                    #runtime::decode_required_input(port, value)
                }
            }
            impl #impl_generics #runtime::OutputValue for #name #type_generics #where_clause {
                fn value_type() -> #runtime::ValueType { #runtime::ValueType::Object }
                fn encode(self) -> ::std::result::Result<#runtime::ValueRef, #runtime::TypeMismatch> {
                    #runtime::encode_node_value(self)
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
                const PORT_NAMES: &'static [&'static str] = &[#(#field_ports),*];
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

fn expand_enum(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = input.ident;
    let Data::Enum(data) = input.data else {
        return Err(syn::Error::new(name.span(), "NodeEnum requires an enum"));
    };
    if data.variants.is_empty() {
        return Err(syn::Error::new(
            name.span(),
            "NodeEnum requires at least one variant",
        ));
    }
    let mut runtime = None;
    let mut tag = None;
    for attr in &input.attrs {
        if attr.path().is_ident("value") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("runtime") {
                    if runtime.is_some() {
                        return Err(meta.error("duplicate runtime path"));
                    }
                    let path: LitStr = meta.value()?.parse()?;
                    runtime = Some(path.parse::<Path>()?);
                } else if meta.path.is_ident("tag") {
                    if tag.is_some() {
                        return Err(meta.error("duplicate enum tag"));
                    }
                    let value: LitStr = meta.value()?.parse()?;
                    if value.value().is_empty() {
                        return Err(meta.error("enum tag must not be empty"));
                    }
                    tag = Some(value);
                } else {
                    return Err(meta.error("expected `runtime` or `tag` on a NodeEnum enum"));
                }
                Ok(())
            })?;
        }
    }
    let runtime: Path = runtime.unwrap_or_else(|| parse_quote!(::mf_runtime));
    let mut generics = input.generics;
    let mut wire_names = BTreeSet::new();
    let mut decoded = Vec::new();
    let mut encoded = Vec::new();
    for variant in data.variants {
        let ident = variant.ident;
        let (rename, _) = field_attributes(&variant.attrs, "value", false)?;
        let wire = rename.unwrap_or_else(|| LitStr::new(&ident.unraw().to_string(), ident.span()));
        if wire.value().is_empty() {
            return Err(syn::Error::new(
                wire.span(),
                "enum variant name must not be empty",
            ));
        }
        if !wire_names.insert(wire.value()) {
            return Err(syn::Error::new(wire.span(), "duplicate enum variant name"));
        }
        if let Some(tag) = &tag {
            let mut names = BTreeSet::from([tag.value()]);
            let mut field_names = Vec::new();
            let mut field_decoders = Vec::new();
            let mut field_encoders = Vec::new();
            let unit = matches!(variant.fields, Fields::Unit);
            match variant.fields {
                Fields::Unit => {}
                Fields::Named(fields) => {
                    for field in fields.named {
                        let ident = field.ident.expect("named field");
                        let (rename, default) = field_attributes(&field.attrs, "value", true)?;
                        let port = rename.unwrap_or_else(|| {
                            LitStr::new(&ident.unraw().to_string(), ident.span())
                        });
                        if port.value().is_empty() {
                            return Err(syn::Error::new(
                                port.span(),
                                "value port name must not be empty",
                            ));
                        }
                        if !names.insert(port.value()) {
                            return Err(syn::Error::new(
                                port.span(),
                                "duplicate field name or conflict with enum tag",
                            ));
                        }
                        let ty = field.ty;
                        let mut borrowed = BorrowedField {
                            error: None,
                            derive_name: "NodeEnum",
                            attribute: "value",
                        };
                        borrowed.visit_type(&ty);
                        if let Some(error) = borrowed.error {
                            return Err(error);
                        }
                        generics
                            .make_where_clause()
                            .predicates
                            .push(parse_quote!(#ty: #runtime::InputField + #runtime::OutputField));
                        let decode = if let Some(default) = default {
                            if matches!(default, FieldDefault::Trait) {
                                generics
                                    .make_where_clause()
                                    .predicates
                                    .push(parse_quote!(#ty: ::std::default::Default));
                            }
                            let default = default_tokens(&default, &ty);
                            quote!(#runtime::decode_default_input::<#ty>(&mut __mf_inputs, #port, #default)?)
                        } else {
                            quote!(#runtime::decode_input::<#ty>(&mut __mf_inputs, #port)?)
                        };
                        field_decoders.push(quote!(#ident: #decode));
                        field_encoders.push(quote!(#runtime::encode_output::<#ty>(&mut __mf_outputs, #port, #ident)?;));
                        field_names.push(ident);
                    }
                }
                Fields::Unnamed(fields) => {
                    return Err(syn::Error::new(
                        fields.span(),
                        "tagged NodeEnum requires unit or named-field variants",
                    ));
                }
            }
            let construction = if unit {
                quote!(Self::#ident)
            } else {
                quote!(Self::#ident { #(#field_decoders),* })
            };
            let pattern = if unit {
                quote!(Self::#ident)
            } else {
                quote!(Self::#ident { #(#field_names),* })
            };
            decoded.push(quote!(#wire => {
                let __mf_result = #construction;
                #runtime::reject_unknown_inputs(__mf_inputs)?;
                Ok(__mf_result)
            }));
            encoded.push(quote!(#pattern => {
                let mut __mf_outputs = #runtime::Outputs::new();
                __mf_outputs.insert(#tag.into(), #runtime::ValueRef::from(#wire));
                #(#field_encoders)*
                Ok(__mf_outputs)
            }));
        } else {
            if !matches!(variant.fields, Fields::Unit) {
                return Err(syn::Error::new(
                    variant.fields.span(),
                    "string NodeEnum requires unit variants; use `#[value(tag = \"kind\")]` for payloads",
                ));
            }
            decoded.push(quote!(#wire => Ok(Self::#ident)));
            encoded.push(quote!(Self::#ident => Ok(#runtime::ValueRef::from(#wire))));
        }
    }
    let descriptor = if tag.is_some() {
        quote!(#runtime::ValueType::Object)
    } else {
        quote!(#runtime::ValueType::String)
    };
    let decode = if let Some(tag) = &tag {
        quote!(#runtime::decode_object_value(value, |mut __mf_inputs| {
            let __mf_tag: ::std::string::String = #runtime::decode_input(&mut __mf_inputs, #tag)?;
            match __mf_tag.as_str() { #(#decoded),*, _ => #runtime::unknown_enum_tag(#tag) }
        }))
    } else {
        quote! {
            let __mf_variant = <::std::string::String as #runtime::InputValue>::decode(value)?;
            match __mf_variant.as_str() { #(#decoded),*, _ => #runtime::unknown_enum_variant() }
        }
    };
    let encode = if tag.is_some() {
        quote!(#runtime::encode_object_value(|| match self { #(#encoded),* }))
    } else {
        quote!(match self { #(#encoded),* })
    };
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #runtime::InputValue for #name #type_generics #where_clause {
            fn value_type() -> #runtime::ValueType { #descriptor }
            fn decode(value: #runtime::ValueRef) -> ::std::result::Result<Self, #runtime::TypeMismatch> { #decode }
        }
        impl #impl_generics #runtime::InputField for #name #type_generics #where_clause {
            const REQUIRED: bool = true;
            fn value_type() -> #runtime::ValueType { #descriptor }
            fn decode_field(port: &str, value: ::std::option::Option<#runtime::ValueRef>) -> ::std::result::Result<Self, #runtime::InputDecodeError> {
                #runtime::decode_required_input(port, value)
            }
        }
        impl #impl_generics #runtime::OutputValue for #name #type_generics #where_clause {
            fn value_type() -> #runtime::ValueType { #descriptor }
            fn encode(self) -> ::std::result::Result<#runtime::ValueRef, #runtime::TypeMismatch> { #encode }
        }
    })
}
