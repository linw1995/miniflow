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
    let Data::Struct(data) = input.data else {
        return Err(syn::Error::new(
            input.ident.span(),
            "NodeConfig requires a named-field struct",
        ));
    };
    let Fields::Named(fields) = data.fields else {
        return Err(syn::Error::new(
            input.ident.span(),
            "NodeConfig requires a named-field struct",
        ));
    };

    let name = input.ident;
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
    let mut rename_all = None;
    for meta in serde_options(&input.attrs)? {
        if meta.path().is_ident("rename_all") {
            if rename_all.is_some() {
                return Err(syn::Error::new_spanned(meta, "duplicate serde rename_all"));
            }
            rename_all = Some(RenameRule::parse(&string_value(&meta)?)?);
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
    let mut parameters = Vec::new();
    let mut flow_names = BTreeMap::new();
    let mut ordinary_names = BTreeSet::new();
    let mut ordinary_wire_names = BTreeSet::new();
    let mut flatten = None;
    for field in fields.named {
        let mut options = serde_options(&field.attrs)?;
        let Some((flow_ident, optional)) = flow_field_type(&field.ty) else {
            let mut nested = ContainsFlow(false);
            nested.visit_type(&field.ty);
            if nested.0 {
                return Err(syn::Error::new_spanned(
                    field.ty,
                    "NodeConfig supports only direct Flow or Option<Flow> fields (or PreparedSubgraph equivalents); nested containers and wrappers are unsupported",
                ));
            }
            let ident = field.ident.clone().expect("named field");
            let mut ordinary = field.clone();
            for meta in &mut options {
                qualify_serde_path(meta, &name)?;
            }
            ordinary.attrs = if options.is_empty() {
                Vec::new()
            } else {
                vec![parse_quote!(#[serde(#(#options),*)])]
            };
            ordinary_fields.push(ordinary);
            ordinary_idents.push(ident.clone());
            let mut rename = None;
            for meta in options {
                if meta.path().is_ident("rename") {
                    if let Some(value) = deserialize_rename(&meta)? {
                        if rename.is_some() {
                            return Err(syn::Error::new_spanned(
                                meta,
                                "duplicate serde deserialize rename",
                            ));
                        }
                        rename = Some(value);
                    }
                } else if meta.path().is_ident("alias") {
                    ordinary_names.insert(string_value(&meta)?.value());
                } else if meta.path().is_ident("flatten") {
                    flatten = Some(meta.span());
                }
            }
            let wire = wire_name(&ident, rename, rename_all.as_ref());
            if !ordinary_wire_names.insert(wire.value()) {
                return Err(syn::Error::new(
                    wire.span(),
                    "duplicate configuration parameter wire name",
                ));
            }
            ordinary_names.insert(wire.value());
            parameters.push((wire, quote!(#runtime::ConfigParameterType::Value)));
            continue;
        };
        let ident = field.ident.expect("named field");
        let mut rename = None;
        let mut default = false;
        for meta in options {
            if meta.path().is_ident("rename") {
                if rename.is_some() {
                    return Err(syn::Error::new_spanned(meta, "duplicate serde rename"));
                }
                rename = Some(string_value(&meta)?);
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
        let wire = wire_name(&ident, rename, rename_all.as_ref());
        if flow_names.insert(wire.value(), wire.span()).is_some() {
            return Err(syn::Error::new(
                wire.span(),
                "duplicate Flow parameter wire name",
            ));
        }
        let prepared = quote! {
            {
                let source = #config_value.as_object_mut().and_then(|fields| fields.remove(#wire));
                let present = source.as_ref().is_some_and(|value| !value.is_null());
                let prepared = #prepared_flows.remove(#wire);
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
        let parameter_type = if optional {
            quote!(#runtime::ConfigParameterType::Optional(::std::boxed::Box::new(
                #runtime::ConfigParameterType::Flow
            )))
        } else {
            quote!(#runtime::ConfigParameterType::Flow)
        };
        parameters.push((wire, parameter_type));
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
        qualify_serde_path(&mut meta, &name)?;
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
    let (parameter_names, parameter_types): (Vec<_>, Vec<_>) = parameters.into_iter().unzip();
    Ok(quote! {
        impl #runtime::NodeConfig for #name {
            fn parameter_types() -> #runtime::ConfigParameterTypes {
                #runtime::ConfigParameterTypes::from([
                    #((#parameter_names.into(), #parameter_types)),*
                ])
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

fn wire_name(ident: &syn::Ident, rename: Option<LitStr>, rule: Option<&RenameRule>) -> LitStr {
    rename.unwrap_or_else(|| {
        let name = ident.unraw().to_string();
        let name = rule.map_or_else(|| name.clone(), |rule| rule.apply(&name));
        LitStr::new(&name, ident.span())
    })
}

// Ordinary fields may use directional renames even though Flow fields cannot:
// only the deserialize name participates in routing incoming configuration.
fn deserialize_rename(meta: &Meta) -> syn::Result<Option<LitStr>> {
    let Meta::List(list) = meta else {
        return string_value(meta).map(Some);
    };
    let mut rename = None;
    let mut seen = BTreeSet::new();
    for direction in list.parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)? {
        let name = if direction.path().is_ident("deserialize") {
            "deserialize"
        } else if direction.path().is_ident("serialize") {
            "serialize"
        } else {
            return Err(syn::Error::new_spanned(
                direction,
                "expected `serialize` or `deserialize` in serde rename",
            ));
        };
        if !seen.insert(name) {
            return Err(syn::Error::new_spanned(
                direction,
                "duplicate serde rename direction",
            ));
        }
        let value = string_value(&direction)?;
        if name == "deserialize" {
            rename = Some(value);
        }
    }
    Ok(rename)
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

// These are Serde's *field* rules, not its enum-variant rules. In particular,
// lowercase and snake_case preserve the field identifier rather than splitting it.
enum RenameRule {
    Unchanged,
    Uppercase,
    PascalCase,
    CamelCase,
    KebabCase,
    ScreamingKebabCase,
}

impl RenameRule {
    fn parse(value: &LitStr) -> syn::Result<Self> {
        match value.value().as_str() {
            "lowercase" | "snake_case" => Ok(Self::Unchanged),
            "UPPERCASE" | "SCREAMING_SNAKE_CASE" => Ok(Self::Uppercase),
            "PascalCase" => Ok(Self::PascalCase),
            "camelCase" => Ok(Self::CamelCase),
            "kebab-case" => Ok(Self::KebabCase),
            "SCREAMING-KEBAB-CASE" => Ok(Self::ScreamingKebabCase),
            _ => Err(syn::Error::new(
                value.span(),
                "unsupported serde rename_all rule",
            )),
        }
    }

    fn apply(&self, field: &str) -> String {
        match self {
            Self::Unchanged => field.to_owned(),
            Self::Uppercase => field.to_ascii_uppercase(),
            Self::PascalCase | Self::CamelCase => {
                let mut capitalize = true;
                let mut name = String::new();
                for ch in field.chars() {
                    if ch == '_' {
                        capitalize = true;
                    } else if capitalize {
                        name.push(ch.to_ascii_uppercase());
                        capitalize = false;
                    } else {
                        name.push(ch);
                    }
                }
                if matches!(self, Self::CamelCase) {
                    let mut chars = name.chars();
                    if let Some(first) = chars.next() {
                        return first.to_ascii_lowercase().to_string() + chars.as_str();
                    }
                }
                name
            }
            Self::KebabCase => field.replace('_', "-"),
            Self::ScreamingKebabCase => field.to_ascii_uppercase().replace('_', "-"),
        }
    }
}
