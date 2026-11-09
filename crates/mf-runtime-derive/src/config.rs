use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Fields, GenericArgument, LitStr, Meta, Path, PathArguments, Type,
    ext::IdentExt, parse_quote, punctuated::Punctuated, spanned::Spanned, visit::Visit,
};

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            input.generics.span(),
            "NodeConfig does not support generic structs",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "NodeConfig requires a named-field struct",
        ));
    };
    let Fields::Named(_) = &data.fields else {
        return Err(syn::Error::new(
            input.ident.span(),
            "NodeConfig requires a named-field struct",
        ));
    };

    let name = &input.ident;
    let ordinary_config = format_ident!(
        "__Mf{}OrdinaryConfig",
        name.unraw(),
        span = proc_macro2::Span::mixed_site()
    );
    let mut runtime = None;
    for attr in &input.attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("runtime") {
                    return Err(meta.error("expected `runtime = \"path\"` on the config struct"));
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
    for meta in serde_options(&input.attrs)? {
        if meta.path().is_ident("rename_all") {
            string_value(&meta)?;
        } else if ![
            "rename",
            "default",
            "deny_unknown_fields",
            "bound",
            "crate",
            "expecting",
        ]
        .iter()
        .any(|name| meta.path().is_ident(name))
        {
            return Err(syn::Error::new_spanned(
                meta,
                "unsupported serde container option for NodeConfig; it may change Flow parameter wire names or decoding",
            ));
        }
    }

    // Keep caller-chosen field names from shadowing the decoder's working values.
    let config_value = syn::Ident::new("__mf_config", proc_macro2::Span::mixed_site());
    let prepared_flows = syn::Ident::new("__mf_flows", proc_macro2::Span::mixed_site());
    let mut ordinary_fields = Vec::new();
    let mut ordinary_idents = Vec::new();
    let mut flow_initializers = Vec::new();
    let mut flow_idents = Vec::new();
    let mut flow_extractions = Vec::new();
    let mut flow_names = BTreeMap::new();
    let mut ordinary_names = BTreeSet::new();
    let mut ordinary_wire_names = BTreeSet::new();
    let mut flatten = None;
    // Serde owns wire names and aliases. Consume its resolved fields rather than
    // reimplementing rename/rename_all rules alongside the ordinary decoder.
    let context = serde_derive_internals::Ctxt::new();
    let container = serde_derive_internals::ast::Container::from_ast(
        &context,
        &input,
        serde_derive_internals::Derive::Deserialize,
        &syn::Ident::new("__private", proc_macro2::Span::mixed_site()),
    );
    context.check()?;
    let container = container.expect("validated named-field config");
    let serde_derive_internals::ast::Data::Struct(_, fields) = container.data else {
        unreachable!("validated named-field config")
    };
    for metadata in fields {
        let wire = metadata.attrs.name().deserialize_name();
        let wire = LitStr::new(&wire.value, wire.span);
        let field = metadata.original;
        let mut options = serde_options(&field.attrs)?;
        let Some((flow_ident, optional)) = flow_field_type(&field.ty) else {
            let mut nested = ContainsFlow(false);
            nested.visit_type(&field.ty);
            if nested.0 {
                return Err(syn::Error::new_spanned(
                    &field.ty,
                    "NodeConfig supports only direct Flow or Option<Flow> fields (or PreparedSubgraph equivalents); nested containers and wrappers are unsupported",
                ));
            }
            reject_repeated_deserialize_names(&options)?;
            let ident = field.ident.clone().expect("named field");
            let mut ordinary = field.clone();
            for meta in &mut options {
                qualify_serde_path(meta, name)?;
            }
            ordinary.attrs = if options.is_empty() {
                Vec::new()
            } else {
                vec![parse_quote!(#[serde(#(#options),*)])]
            };
            ordinary_fields.push(ordinary);
            ordinary_idents.push(ident.clone());
            for meta in options {
                if meta.path().is_ident("flatten") {
                    flatten = Some(meta.span());
                }
            }
            ordinary_names.extend(
                metadata
                    .attrs
                    .aliases()
                    .iter()
                    .map(|alias| alias.value.clone()),
            );
            if !ordinary_wire_names.insert(wire.value()) {
                return Err(syn::Error::new(
                    wire.span(),
                    "duplicate configuration parameter wire name",
                ));
            }
            ordinary_names.insert(wire.value());
            continue;
        };
        let ident = field.ident.clone().expect("named field");
        let mut default = false;
        for meta in options {
            if meta.path().is_ident("rename") {
                string_value(&meta)?;
            } else if !(meta.path().is_ident("default")
                && matches!(meta, Meta::Path(_))
                && optional
                && !default)
            {
                return Err(syn::Error::new_spanned(
                    meta,
                    "unsupported serde option on a Flow parameter field; only `rename = \"name\"` and bare `default` on optional fields are supported; executable defaults cannot construct a Flow",
                ));
            } else {
                default = true;
            }
        }
        if flow_names.insert(wire.value(), wire.span()).is_some() {
            return Err(syn::Error::new(
                wire.span(),
                "duplicate Flow parameter wire name",
            ));
        }
        let pointer = LitStr::new(
            &format!("/{}", wire.value().replace('~', "~0").replace('/', "~1")),
            wire.span(),
        );
        let prepared = quote! {
            {
                let source = #config_value.as_object_mut().and_then(|fields| fields.remove(#wire));
                let present = source.as_ref().is_some_and(|value| !value.is_null());
                let prepared = #prepared_flows.remove(#pointer);
                if present != prepared.is_some() {
                    return #runtime::NodeInvalidSubgraphSnafu {
                        message: ::std::format!("prepared Flow field `{}` does not match its configuration", #wire),
                    }.fail();
                }
                prepared
            }
        };
        let convert = if flow_ident == "Flow" {
            quote!(prepared.into_flow()?)
        } else {
            quote!(prepared)
        };
        let value = if !optional {
            quote! {
                {
                    let prepared = #prepared.ok_or_else(|| #runtime::NodeInvalidSubgraphSnafu {
                        message: ::std::format!("missing prepared Flow field `{}`", #wire),
                    }.build())?;
                    #convert
                }
            }
        } else {
            quote! {
                match #prepared {
                    ::core::option::Option::Some(prepared) => ::core::option::Option::Some(#convert),
                    ::core::option::Option::None => ::core::option::Option::None,
                }
            }
        };
        flow_initializers.push(quote!(let #ident = #value;));
        flow_idents.push(ident);
        flow_extractions.push(quote! {
            if let ::core::option::Option::Some(value) = #config_value.get(#wire).filter(|value| !value.is_null()) {
                flows.insert(::std::string::String::from(#pointer), value.clone());
            }
        });
    }
    if !flow_names.is_empty()
        && let Some(span) = flatten
    {
        return Err(syn::Error::new(
            span,
            "NodeConfig cannot combine Flow parameter fields with serde flatten; flattened wire names are unknown, so implement NodeConfig manually",
        ));
    }
    for (parameter, span) in &flow_names {
        if ordinary_names.contains(parameter) {
            return Err(syn::Error::new(
                *span,
                "Flow parameter wire name collides with an ordinary field's deserialize name or alias",
            ));
        }
    }
    let mut container_options = Vec::new();
    let mut helper_default = None;
    for mut meta in serde_options(&input.attrs)? {
        qualify_serde_path(&mut meta, name)?;
        if meta.path().is_ident("default") {
            let default = if matches!(meta, Meta::Path(_)) {
                quote!(<#name as ::core::default::Default>::default())
            } else {
                let path: syn::ExprPath = string_value(&meta)?.parse()?;
                quote!(#path())
            };
            helper_default = Some(quote! {
                impl ::core::default::Default for #ordinary_config {
                    fn default() -> Self {
                        let _config = #default;
                        Self { #(#ordinary_idents: _config.#ordinary_idents),* }
                    }
                }
            });
            container_options.push(parse_quote!(default));
        } else {
            container_options.push(meta);
        }
    }
    let container_attrs =
        (!container_options.is_empty()).then(|| quote!(#[serde(#(#container_options),*)]));
    Ok(quote! {
        impl #runtime::NodeConfig for #name {
            #[allow(unused_mut)]
            fn flow_parameters(
                #config_value: &::serde_json::Value,
            ) -> ::std::collections::BTreeMap<::std::string::String, ::serde_json::Value> {
                let mut flows = ::std::collections::BTreeMap::new();
                #(#flow_extractions)*
                flows
            }

            #[allow(unused_mut)]
            fn from_config(
                mut #config_value: ::serde_json::Value,
                mut #prepared_flows: ::std::collections::BTreeMap<::std::string::String, #runtime::PreparedSubgraph>,
            ) -> ::core::result::Result<Self, #runtime::NodeBuildError> {
                #[derive(::serde::Deserialize)]
                #container_attrs
                struct #ordinary_config { #(#ordinary_fields),* }
                #helper_default
                #(#flow_initializers)*
                if let ::core::option::Option::Some(name) = #prepared_flows.keys().next() {
                    return #runtime::NodeInvalidSubgraphSnafu {
                        message: ::std::format!("unexpected prepared Flow field `{}`", name),
                    }.fail();
                }
                let #ordinary_config { #(#ordinary_idents),* } =
                    #runtime::deserialize_config::<#ordinary_config>(#config_value)?;
                Ok(Self { #(#ordinary_idents,)* #(#flow_idents),* })
            }
        }
    })
}

// These Serde paths are evaluated inside the local helper after projection.
// Preserve an explicit `Self::...` receiver from the original config's scope.
fn qualify_serde_path(meta: &mut Meta, config: &syn::Ident) -> syn::Result<()> {
    if matches!(meta, Meta::NameValue(_))
        && [
            "default",
            "deserialize_with",
            "serialize_with",
            "with",
            "skip_serializing_if",
            "getter",
        ]
        .iter()
        .any(|name| meta.path().is_ident(name))
    {
        let value = string_value(meta)?;
        let mut path: syn::ExprPath = value.parse()?;
        let receiver = match &mut path.qself {
            Some(qualified) => match qualified.ty.as_mut() {
                Type::Path(ty) => &mut ty.path,
                _ => return Ok(()),
            },
            None => &mut path.path,
        };
        if let Some(first) = receiver.segments.first_mut()
            && first.ident == "Self"
        {
            first.ident = config.clone();
            let value = LitStr::new(&quote!(#path).to_string(), value.span());
            let Meta::NameValue(meta) = meta else {
                unreachable!()
            };
            meta.value = parse_quote!(#value);
        }
    }
    Ok(())
}

fn serde_options(attrs: &[syn::Attribute]) -> syn::Result<Vec<Meta>> {
    let mut options = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("serde") {
            options.extend(
                attr.parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)?,
            );
        }
    }
    Ok(options)
}

fn string_value(meta: &Meta) -> syn::Result<LitStr> {
    if let Meta::NameValue(value) = meta
        && let syn::Expr::Lit(value) = &value.value
        && let syn::Lit::Str(value) = &value.lit
    {
        return Ok(value.clone());
    }
    Err(syn::Error::new_spanned(
        meta,
        "NodeConfig requires a string value; directional Serde renames are unsupported",
    ))
}

// Serde accepts repeated deserialize renames as aliases. NodeConfig keeps its
// single-canonical-name contract; values and name transformations stay Serde-owned.
fn reject_repeated_deserialize_names(options: &[Meta]) -> syn::Result<()> {
    let mut count = 0;
    for meta in options.iter().filter(|meta| meta.path().is_ident("rename")) {
        count += match meta {
            Meta::NameValue(_) => 1,
            Meta::List(list) => list
                .parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)?
                .iter()
                .filter(|direction| direction.path().is_ident("deserialize"))
                .count(),
            Meta::Path(_) => 0,
        };
        if count > 1 {
            return Err(syn::Error::new_spanned(
                meta,
                "duplicate serde attribute `rename`",
            ));
        }
    }
    Ok(())
}

fn flow_field_type(ty: &Type) -> Option<(&syn::Ident, bool)> {
    let Type::Path(path) = ty else { return None };
    if path.qself.is_some() {
        return None;
    }
    let segment = path.path.segments.last()?;
    match &segment.arguments {
        PathArguments::None if segment.ident == "Flow" || segment.ident == "PreparedSubgraph" => {
            Some((&segment.ident, false))
        }
        PathArguments::AngleBracketed(arguments)
            if segment.ident == "Option" && arguments.args.len() == 1 =>
        {
            let GenericArgument::Type(inner) = arguments.args.first()? else {
                return None;
            };
            let (ident, optional) = flow_field_type(inner)?;
            (!optional).then_some((ident, true))
        }
        _ => None,
    }
}

struct ContainsFlow(bool);

impl<'ast> Visit<'ast> for ContainsFlow {
    fn visit_type_path(&mut self, path: &'ast syn::TypePath) {
        if path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Flow" || segment.ident == "PreparedSubgraph")
        {
            self.0 = true;
        }
        syn::visit::visit_type_path(self, path);
    }
}
