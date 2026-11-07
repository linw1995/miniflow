use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Data, DeriveInput, Fields, GenericArgument, LitStr, Meta, Path, PathArguments, Type,
    parse_quote, punctuated::Punctuated, spanned::Spanned,
};

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            input.generics.span(),
            "NodeConfig does not support generic declarations",
        ));
    }
    let name = &input.ident;
    let mut runtime = None;
    for attr in &input.attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                if !meta.path.is_ident("runtime") || runtime.is_some() {
                    return Err(meta.error("expected one `runtime = \"path\"`"));
                }
                runtime = Some(meta.value()?.parse::<LitStr>()?.parse::<Path>()?);
                Ok(())
            })?;
        }
    }
    let runtime = runtime.unwrap_or_else(|| parse_quote!(::mf_runtime));
    let options = serde_options(&input.attrs)?;
    // Serde resolves wire names, aliases and rename_all rules for every supported shape.
    // Keep the same metadata authority as the ordinary Serde decoder.
    let context = serde_derive_internals::Ctxt::new();
    let metadata = serde_derive_internals::ast::Container::from_ast(
        &context,
        &input,
        serde_derive_internals::Derive::Deserialize,
        &syn::Ident::new("__private", proc_macro2::Span::mixed_site()),
    );
    context.check()?;
    let metadata = metadata.expect("validated config declaration");
    for option in &options {
        if ![
            "rename",
            "rename_all",
            "rename_all_fields",
            "default",
            "deny_unknown_fields",
            "tag",
            "content",
            "bound",
            "crate",
            "expecting",
        ]
        .iter()
        .any(|key| option.path().is_ident(key))
        {
            return Err(syn::Error::new_spanned(
                option,
                "unsupported serde container option for NodeConfig; ambiguous/custom wire layouts require a manual NodeConfig implementation",
            ));
        }
    }
    let shape = match &input.data {
        Data::Struct(data) => shape(
            &data.fields,
            quote!(Self),
            name,
            &runtime,
            &options,
            &match &metadata.data {
                serde_derive_internals::ast::Data::Struct(_, fields) => field_wires(fields),
                _ => unreachable!(),
            },
            "Struct",
            UnitWire::Null,
        )?,
        Data::Enum(data) => {
            if options.iter().any(|meta| meta.path().is_ident("default")) {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "serde container defaults are unsupported on NodeConfig enums",
                ));
            }
            let tag = option_string(&options, "tag")?;
            let content = option_string(&options, "content")?;
            if content.is_some() && tag.is_none() {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "serde content requires a tag",
                ));
            }
            let serde_derive_internals::ast::Data::Enum(variants) = &metadata.data else {
                unreachable!()
            };
            let mut definitions = Vec::new();
            let mut collectors = Vec::new();
            let mut decoders = Vec::new();
            let mut contains = Vec::new();
            let mut variant_names = BTreeSet::new();
            for (index, variant) in data.variants.iter().enumerate() {
                let ident = &variant.ident;
                if tag.is_some()
                    && content.is_none()
                    && matches!(&variant.fields, Fields::Unnamed(fields) if fields.unnamed.len() != 1)
                {
                    return Err(syn::Error::new_spanned(
                        variant,
                        "internally tagged tuple variants are unsupported by Serde",
                    ));
                }
                let attrs = serde_options(&variant.attrs)?;
                for meta in &attrs {
                    if !["rename", "alias", "rename_all"]
                        .iter()
                        .any(|key| meta.path().is_ident(key))
                    {
                        return Err(syn::Error::new_spanned(
                            meta,
                            "unsupported serde variant option for NodeConfig",
                        ));
                    }
                }
                let wire = variants[index].attrs.name().deserialize_name();
                let canonical = LitStr::new(&wire.value, wire.span);
                let mut names = vec![canonical.clone()];
                names.extend(
                    variants[index]
                        .attrs
                        .aliases()
                        .iter()
                        .filter(|alias| alias.value != canonical.value())
                        .map(|alias| LitStr::new(&alias.value, alias.span)),
                );
                for wire in &names {
                    if !variant_names.insert(wire.value()) {
                        return Err(syn::Error::new_spanned(
                            wire,
                            "duplicate NodeConfig enum variant wire name",
                        ));
                    }
                }
                let mut field_options: Vec<_> = options
                    .iter()
                    .filter(|meta| meta.path().is_ident("deny_unknown_fields"))
                    .cloned()
                    .collect();
                field_options.extend(
                    attrs
                        .iter()
                        .filter(|meta| meta.path().is_ident("rename_all"))
                        .cloned(),
                );
                let unit_wire = if tag.is_some() && content.is_none() {
                    UnitWire::TaggedObject
                } else {
                    UnitWire::Variant
                };
                let variant_shape = shape(
                    &variant.fields,
                    quote!(Self::#ident),
                    name,
                    &runtime,
                    &field_options,
                    &field_wires(&variants[index].fields),
                    &format!("Variant{index}"),
                    unit_wire,
                )?;
                definitions.push(variant_shape.definitions);
                let collect = variant_shape.collect;
                let decode = variant_shape.decode;
                collectors.push(quote!(#(#names)|* => { #collect }));
                decoders.push(quote!(#(#names)|* => { #decode }));
                contains.push(variant_shape.contains);
            }
            let deny_unknown = options
                .iter()
                .any(|meta| meta.path().is_ident("deny_unknown_fields"));
            let select = enum_selection(&tag, &content, &runtime, false, quote!(return Ok(())));
            let select_decode = enum_selection(
                &tag,
                &content,
                &runtime,
                deny_unknown,
                quote!(return Err(#runtime::node_config::invalid("missing configuration enum"))),
            );
            Shape {
                definitions: quote!(#(#definitions)*),
                collect: quote! { #select let __mf_config = __mf_payload.as_ref(); match __mf_variant { #(#collectors,)* _ => Err(#runtime::node_config::invalid("unknown configuration enum variant")) } },
                decode: quote! {
                    let __mf_value = __mf_config.take();
                    let __mf_config = __mf_value.as_ref();
                    #select_decode
                    let __mf_config = &mut { __mf_payload };
                    match __mf_variant { #(#decoders,)* _ => Err(#runtime::node_config::invalid("unknown configuration enum variant")) }
                },
                contains: quote!(false #(|| (#contains))*),
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                name,
                "NodeConfig does not support unions",
            ));
        }
    };
    let Shape {
        definitions,
        collect,
        decode,
        contains,
    } = shape;
    let leaf = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                traversal(&fields.unnamed[0].ty, Action::IsLeaf, &runtime)
            }
            _ => quote!(false),
        },
        _ => quote!(false),
    };
    Ok(quote! {
        const _: () = {
            #definitions
            impl #runtime::NodeConfig for #name {
                #[allow(unused_imports, unused_variables, unused_mut)]
                fn collect(__mf_config: ::core::option::Option<&::serde_json::Value>, __mf_prefix: &str, __mf_paths: &mut ::std::vec::Vec<::std::string::String>) -> ::core::result::Result<(), #runtime::NodeBuildError> {
                    use #runtime::node_config::{ConfigFieldDispatch as _, ConfigContainerDispatch as _};
                    #collect
                }
                #[allow(unused_imports, unused_variables, unused_mut)]
                fn decode(__mf_config: &mut ::core::option::Option<::serde_json::Value>, __mf_prefix: &str, __mf_flows: &mut ::std::collections::BTreeMap<::std::string::String, #runtime::PreparedSubgraph>) -> ::core::result::Result<Self, #runtime::NodeBuildError> {
                    use #runtime::node_config::{ConfigFieldDispatch as _, ConfigContainerDispatch as _};
                    #decode
                }
                #[allow(unused_imports, unused_variables)]
                fn contains_flows(__mf_visited: &mut ::std::collections::BTreeSet<&'static str>) -> bool {
                    use #runtime::node_config::{ConfigFieldDispatch as _, ConfigContainerDispatch as _};
                    if !__mf_visited.insert(::core::any::type_name::<Self>()) { return false; }
                    let __mf_result = #contains;
                    __mf_visited.remove(::core::any::type_name::<Self>());
                    __mf_result
                }
                #[allow(unused_imports)]
                fn is_flow_leaf() -> bool {
                    use #runtime::node_config::{ConfigFieldDispatch as _, ConfigContainerDispatch as _};
                    // A recursive newtype with no executable leaves must terminate
                    // before following its statically recursive transparent wrappers.
                    if !<Self as #runtime::NodeConfig>::contains_flows(&mut ::std::collections::BTreeSet::new()) {
                        return false;
                    }
                    #leaf
                }
            }
        };
    })
}

fn enum_selection(
    tag: &Option<LitStr>,
    content: &Option<LitStr>,
    runtime: &Path,
    deny_unknown: bool,
    missing: TokenStream,
) -> TokenStream {
    match (tag, content) {
        (Some(tag), Some(content)) => {
            let check = deny_unknown.then(|| quote! {
                if __mf_object.keys().any(|key| key != #tag && key != #content) {
                    return Err(#runtime::node_config::invalid("unknown field in adjacently tagged configuration enum"));
                }
            });
            quote! {
                let Some(__mf_object) = #runtime::node_config::object(__mf_config)? else { #missing; };
                #check
                let __mf_variant = __mf_object.get(#tag).and_then(::serde_json::Value::as_str)
                    .ok_or_else(|| #runtime::node_config::invalid("missing or invalid enum tag"))?;
                let __mf_payload = __mf_object.get(#content).cloned();
                let __mf_prefix_owned = #runtime::node_config::join_config_path(__mf_prefix, #content);
                let __mf_prefix = __mf_prefix_owned.as_str();
            }
        }
        (Some(tag), None) => quote! {
            let Some(__mf_object) = #runtime::node_config::object(__mf_config)? else { #missing; };
            let __mf_variant = __mf_object.get(#tag).and_then(::serde_json::Value::as_str)
                .ok_or_else(|| #runtime::node_config::invalid("missing or invalid enum tag"))?;
            let mut __mf_payload = __mf_object.clone();
            __mf_payload.remove(#tag);
            let __mf_payload = Some(::serde_json::Value::Object(__mf_payload));
        },
        (None, _) => quote! {
            let Some(__mf_value) = __mf_config else { #missing; };
            let (__mf_variant, __mf_payload, __mf_prefix_owned) = match __mf_value {
                ::serde_json::Value::String(variant) => (variant.as_str(), None, __mf_prefix.to_owned()),
                ::serde_json::Value::Object(object) if object.len() == 1 => {
                    let (variant, payload) = object.iter().next().unwrap();
                    (variant.as_str(), Some(payload.clone()), #runtime::node_config::join_config_path(__mf_prefix, variant))
                }
                _ => return Err(#runtime::node_config::invalid("expected an externally tagged configuration enum")),
            };
            let __mf_prefix = __mf_prefix_owned.as_str();
        },
    }
}

struct Shape {
    definitions: TokenStream,
    collect: TokenStream,
    decode: TokenStream,
    contains: TokenStream,
}
#[derive(Clone, Copy)]
enum UnitWire {
    Null,
    Variant,
    TaggedObject,
}

#[expect(
    clippy::too_many_arguments,
    reason = "The shape generator shares the original Serde context and constructor across structs and enum variants."
)]
fn shape(
    fields: &Fields,
    constructor: TokenStream,
    owner: &syn::Ident,
    runtime: &Path,
    options: &[Meta],
    wires: &[FieldWire],
    suffix: &str,
    unit: UnitWire,
) -> syn::Result<Shape> {
    let helper = format_ident!("__Mf{suffix}Raw", span = proc_macro2::Span::mixed_site());
    let container_default = options.iter().find(|meta| meta.path().is_ident("default"));
    let deny_unknown = options
        .iter()
        .any(|meta| meta.path().is_ident("deny_unknown_fields"));
    let mut definitions = Vec::new();
    let mut raw_fields = Vec::new();
    let mut collectors = Vec::new();
    let mut decoders = Vec::new();
    let mut contains = Vec::new();
    let mut members = Vec::new();
    let mut default_fields = Vec::new();
    let mut initializers = Vec::new();
    let mut default_safety = Vec::new();
    let mut default_checks = Vec::new();
    let mut names = BTreeSet::new();
    let is_newtype = matches!(fields, Fields::Unnamed(fields) if fields.unnamed.len() == 1);
    for (index, field) in fields.iter().enumerate() {
        let member = field.ident.as_ref().map_or_else(
            || syn::Member::Unnamed(index.into()),
            |ident| syn::Member::Named(ident.clone()),
        );
        let raw = format_ident!("__mf_field_{index}", span = proc_macro2::Span::mixed_site());
        let default_var = format_ident!(
            "__mf_default_{index}",
            span = proc_macro2::Span::mixed_site()
        );
        members.push(member.clone());
        default_fields.push(default_var.clone());
        let ty = &field.ty;
        let mut attrs = serde_options(&field.attrs)?;
        for meta in &mut attrs {
            qualify_serde_path(meta, owner)?;
        }
        let flatten = attrs.iter().any(|meta| meta.path().is_ident("flatten"));
        let skip = attrs
            .iter()
            .any(|meta| meta.path().is_ident("skip") || meta.path().is_ident("skip_deserializing"));
        let codec = attrs
            .iter()
            .any(|meta| meta.path().is_ident("with") || meta.path().is_ident("deserialize_with"));
        let default = attrs.iter().find(|meta| meta.path().is_ident("default"));
        for meta in &attrs {
            if ![
                "rename",
                "alias",
                "default",
                "flatten",
                "skip",
                "skip_deserializing",
                "skip_serializing",
                "serialize_with",
                "deserialize_with",
                "with",
                "skip_serializing_if",
                "bound",
                "borrow",
            ]
            .iter()
            .any(|key| meta.path().is_ident(key))
            {
                return Err(syn::Error::new_spanned(
                    meta,
                    "unsupported serde field option for NodeConfig",
                ));
            }
        }
        if flatten && (!matches!(fields, Fields::Named(_)) || codec || default.is_some() || skip) {
            return Err(syn::Error::new_spanned(
                field,
                "flatten with custom decoding/defaults is ambiguous; implement NodeConfig manually",
            ));
        }
        reject_repeated_deserialize_names(&attrs)?;
        let canonical = if field.ident.is_some() {
            wires[index].canonical.clone()
        } else {
            LitStr::new(&index.to_string(), field.span())
        };
        let aliases = &wires[index].aliases;
        if !flatten && !skip {
            for name in std::iter::once(&canonical).chain(aliases) {
                if !names.insert(name.value()) {
                    return Err(syn::Error::new_spanned(
                        name,
                        "duplicate configuration field deserialize name or alias",
                    ));
                }
            }
        }
        let field_contains = traversal(ty, Action::Contains, runtime);
        contains.push(field_contains.clone());
        let unsafe_default = default.is_some_and(|meta| !matches!(meta, Meta::Path(_)))
            || container_default.is_some_and(|meta| !matches!(meta, Meta::Path(_)));
        let has_default = default.is_some() || container_default.is_some();
        let (default_allowed, default_empty) = empty_default_policy(ty, runtime, &default_var);
        let unsafe_policy = if codec || skip || unsafe_default {
            quote!(true)
        } else if has_default {
            quote!(!(#default_allowed))
        } else {
            quote!(false)
        };
        let safety = if codec || skip || has_default {
            quote! {
                if { let __mf_visited = &mut ::std::collections::BTreeSet::new(); #field_contains } && #unsafe_policy {
                    return Err(#runtime::node_config::invalid("custom codecs, skipped fields, and executable defaults cannot supply Flow values"));
                }
            }
        } else {
            quote!()
        };
        default_safety.push(safety.clone());
        if container_default.is_some() {
            default_checks.push(quote! {
                if { let __mf_visited = &mut ::std::collections::BTreeSet::new(); #field_contains } && !(#default_empty) {
                    return Err(#runtime::node_config::invalid("container default supplied executable Flow configuration"));
                }
            });
        }
        let location = if flatten || is_newtype {
            quote!(let __mf_field_prefix = __mf_prefix.to_owned();)
        } else if matches!(fields, Fields::Named(_)) {
            quote! {
                let __mf_wire = if __mf_object.contains_key(#canonical) { #canonical }
                    #(else if __mf_object.contains_key(#aliases) { #aliases })*
                    else { #canonical };
                let __mf_field_prefix = #runtime::node_config::join_config_path(__mf_prefix, __mf_wire);
            }
        } else {
            quote!(let __mf_field_prefix = #runtime::node_config::join_config_path(__mf_prefix, #canonical);)
        };
        let source = if matches!(fields, Fields::Named(_)) {
            let raw_attrs = if flatten {
                quote!(#[serde(flatten)])
            } else {
                let skipped = skip.then(|| quote!(#[serde(skip_deserializing)]));
                quote!(#[serde(default, rename = #canonical #(, alias = #aliases)*)] #skipped)
            };
            let raw_type = if flatten {
                quote!(::serde_json::Value)
            } else {
                quote!(#runtime::node_config::Presence)
            };
            raw_fields.push(quote!(#raw_attrs #raw: #raw_type));
            if flatten {
                quote!(Some(__mf_raw.#raw))
            } else {
                quote!(__mf_raw.#raw.0)
            }
        } else if is_newtype {
            quote!(__mf_value)
        } else {
            quote!(__mf_values.next())
        };
        let collect = traversal(ty, Action::Collect, runtime);
        let blended = (flatten || (is_newtype && matches!(unit, UnitWire::TaggedObject))).then(|| quote! {
            if __mf_paths[__mf_start..].iter().any(|path| path == __mf_prefix) {
                return Err(#runtime::node_config::invalid("a Flow leaf cannot share its wire object with flattened siblings or an internal enum tag"));
            }
        });
        collectors.push(quote! {
            { #safety #location let __mf_value = #source;
              let __mf_config = __mf_value.as_ref(); let __mf_prefix = __mf_field_prefix.as_str();
              let __mf_start = __mf_paths.len(); #collect?; #blended }
        });
        let decode = traversal(ty, Action::Decode, runtime);
        let decode = if skip {
            // Serde ignores supplied data for skipped fields, but still applies
            // field defaults before container defaults before the type default.
            if let Some(default) = default {
                if matches!(default, Meta::Path(_)) {
                    quote!(::core::default::Default::default())
                } else {
                    let path: syn::ExprPath = string_value(default)?.parse()?;
                    quote!(#path())
                }
            } else if container_default.is_some() {
                quote!(#default_var)
            } else {
                quote!(::core::default::Default::default())
            }
        } else if codec || default.is_some_and(|meta| !matches!(meta, Meta::Path(_))) {
            let decoder = format_ident!(
                "__Mf{suffix}Field{index}",
                span = proc_macro2::Span::mixed_site()
            );
            let mut helper_ty = ty.clone();
            qualify_field_type(&mut helper_ty, owner);
            let attrs: Vec<_> = attrs
                .iter()
                .filter(|meta| {
                    ![
                        "rename",
                        "alias",
                        "flatten",
                        "skip_serializing",
                        "serialize_with",
                        "skip_serializing_if",
                        "bound",
                        "borrow",
                    ]
                    .iter()
                    .any(|name| meta.path().is_ident(name))
                })
                .collect();
            definitions.push(quote! {
                #[derive(::serde::Deserialize)]
                struct #decoder { #[serde(#(#attrs),*)] value: #helper_ty }
            });
            quote! {
                { let mut __mf_map = ::serde_json::Map::new();
                  if let Some(value) = __mf_config.take() { __mf_map.insert("value".into(), value); }
                  #runtime::deserialize_config::<#decoder>(::serde_json::Value::Object(__mf_map))?.value }
            }
        } else if default.is_some() {
            quote!(if __mf_config.is_none() { ::core::default::Default::default() } else { #decode? })
        } else if container_default.is_some() {
            quote!(if __mf_config.is_none() { #default_var } else { #decode? })
        } else {
            quote!(#decode?)
        };
        decoders.push(quote! {
            let #raw: #ty = { #safety #location let mut __mf_value = #source;
                let __mf_config = &mut __mf_value; let __mf_prefix = __mf_field_prefix.as_str(); #decode };
        });
        initializers.push(quote!(#member: #raw));
    }
    let default_init = if let Some(meta) = container_default {
        let expr = if matches!(meta, Meta::Path(_)) {
            quote!(<#owner as ::core::default::Default>::default())
        } else {
            let mut meta = meta.clone();
            qualify_serde_path(&mut meta, owner)?;
            let path: syn::ExprPath = string_value(&meta)?.parse()?;
            quote!(#path())
        };
        // Discovery never invokes the default. During decoding, only empty
        // optional/collection Flow defaults are accepted and none are discarded silently.
        quote! {
            #(#default_safety)*
            let #constructor { #(#members: #default_fields),* } = #expr;
            #(#default_checks)*
        }
    } else {
        quote!()
    };
    let (setup_collect, setup_decode) = match fields {
        Fields::Named(_) => {
            let deny = deny_unknown.then(|| quote!(#[serde(deny_unknown_fields)]));
            definitions.push(
                quote!(#[derive(::serde::Deserialize)] #deny struct #helper { #(#raw_fields),* }),
            );
            // Discovery owns only receiver roles. Preserve unknown ordinary data
            // for the decoder, where the original deny_unknown_fields applies.
            let metadata_helper = if deny_unknown {
                let metadata = format_ident!(
                    "__Mf{suffix}MetadataRaw",
                    span = proc_macro2::Span::mixed_site()
                );
                definitions.push(
                    quote!(#[derive(::serde::Deserialize)] struct #metadata { #(#raw_fields),* }),
                );
                metadata
            } else {
                helper.clone()
            };
            (
                quote! {
                    let Some(__mf_object) = #runtime::node_config::object(__mf_config)? else { return Ok(()); };
                    let __mf_raw: #metadata_helper = #runtime::deserialize_config(::serde_json::Value::Object(__mf_object.clone()))?;
                },
                quote! {
                    let __mf_value = __mf_config.take().ok_or_else(|| #runtime::node_config::invalid("missing configuration object"))?;
                    let __mf_object = __mf_value.as_object().ok_or_else(|| #runtime::node_config::invalid("expected a configuration object"))?;
                    let __mf_raw: #helper = #runtime::deserialize_config(__mf_value.clone())?;
                },
            )
        }
        Fields::Unnamed(_) if is_newtype => (
            quote!(let __mf_value = __mf_config.cloned();),
            quote!(let __mf_value = __mf_config.take();),
        ),
        Fields::Unnamed(fields) => {
            let len = fields.unnamed.len();
            (
                quote! {
                    let Some(__mf_values) = #runtime::node_config::array(__mf_config)? else { return Ok(()); };
                    if __mf_values.len() != #len { return Err(#runtime::node_config::invalid("incorrect configuration tuple length")); }
                    let mut __mf_values = __mf_values.iter().cloned();
                },
                quote! {
                    let __mf_values = #runtime::node_config::take_array(__mf_config)?;
                    if __mf_values.len() != #len { return Err(#runtime::node_config::invalid("incorrect configuration tuple length")); }
                    let mut __mf_values = __mf_values.into_iter();
                },
            )
        }
        Fields::Unit => {
            let check = match unit {
                UnitWire::Null | UnitWire::Variant => quote! {
                    if __mf_config.is_some_and(|value| !value.is_null()) {
                        return Err(#runtime::node_config::invalid("expected null for a unit configuration"));
                    }
                },
                UnitWire::TaggedObject => {
                    let unknown = deny_unknown.then(|| quote! {
                        if __mf_object.is_some_and(|object| !object.is_empty()) {
                            return Err(#runtime::node_config::invalid("unknown field in internally tagged unit variant"));
                        }
                    });
                    quote! { let __mf_object = #runtime::node_config::object(__mf_config)?; #unknown }
                }
            };
            let required = matches!(unit, UnitWire::Null).then(|| quote! {
                if __mf_value.is_none() { return Err(#runtime::node_config::invalid("missing unit configuration")); }
            });
            let collect_check = if matches!(unit, UnitWire::TaggedObject) {
                quote!(let _ = #runtime::node_config::object(__mf_config)?;)
            } else {
                check.clone()
            };
            (
                collect_check,
                quote! {
                    let __mf_value = __mf_config.take(); #required
                    let __mf_config = __mf_value.as_ref(); #check
                },
            )
        }
    };
    let construct = match fields {
        Fields::Named(_) => quote!(#constructor { #(#initializers),* }),
        Fields::Unnamed(_) => {
            let fields: Vec<_> = (0..fields.len())
                .map(|index| {
                    format_ident!("__mf_field_{index}", span = proc_macro2::Span::mixed_site())
                })
                .collect();
            quote!(#constructor(#(#fields),*))
        }
        Fields::Unit => constructor,
    };
    Ok(Shape {
        definitions: quote!(#(#definitions)*),
        collect: quote!(#setup_collect #(#collectors)* Ok(())),
        decode: quote!(#default_init #setup_decode #(#decoders)* Ok(#construct)),
        contains: quote!(false #(|| (#contains))*),
    })
}

#[derive(Clone, Copy)]
enum Action {
    Collect,
    Decode,
    Contains,
    IsLeaf,
}

// Expand standard containers at the concrete field site so mixed tuples and
// collections of ordinary custom Deserialize types do not acquire NodeConfig bounds.
fn traversal(ty: &Type, action: Action, runtime: &Path) -> TokenStream {
    let body = traversal_body(ty, action, runtime);
    if matches!(action, Action::Collect) {
        let contains = traversal_body(ty, Action::Contains, runtime);
        quote!({
            if { let __mf_visited = &mut ::std::collections::BTreeSet::new(); #contains } {
                #body
            } else {
                Ok::<(), #runtime::NodeBuildError>(())
            }
        })
    } else {
        body
    }
}

fn traversal_body(ty: &Type, action: Action, runtime: &Path) -> TokenStream {
    let container = if let Type::Path(path) = ty {
        path.path.segments.last().and_then(|segment| {
            let PathArguments::AngleBracketed(args) = &segment.arguments else {
                return None;
            };
            let args: Vec<_> = args
                .args
                .iter()
                .filter_map(|arg| {
                    if let GenericArgument::Type(ty) = arg {
                        Some(ty)
                    } else {
                        None
                    }
                })
                .collect();
            Some((segment.ident.to_string(), args))
        })
    } else {
        None
    };
    if let Some((kind, args)) = container {
        if matches!(kind.as_str(), "Option" | "Box" | "Vec") && args.len() == 1 {
            let inner = traversal(args[0], action, runtime);
            let leaf = traversal(args[0], Action::IsLeaf, runtime);
            let element = args[0];
            let canonical: Type = match kind.as_str() {
                "Option" => parse_quote!(::std::option::Option<#element>),
                "Box" => parse_quote!(::std::boxed::Box<#element>),
                "Vec" => parse_quote!(::std::vec::Vec<#element>),
                _ => unreachable!(),
            };
            let body = match (kind.as_str(), action) {
                (_, Action::Contains) => inner,
                ("Option" | "Box", Action::IsLeaf) => inner,
                (_, Action::IsLeaf) => quote!(false),
                ("Box", Action::Collect) => inner,
                ("Box", Action::Decode) => quote!((#inner).map(::std::boxed::Box::new)),
                ("Option", Action::Collect) => quote!({
                    if #leaf || __mf_config.is_some_and(|value| !value.is_null()) { #inner?; }
                    Ok::<(), #runtime::NodeBuildError>(())
                }),
                ("Option", Action::Decode) => quote!({
                    if __mf_config.as_ref().is_none_or(::serde_json::Value::is_null) && !__mf_flows.contains_key(__mf_prefix) {
                        __mf_config.take(); Ok::<_, #runtime::NodeBuildError>(None)
                    } else { (#inner).map(Some) }
                }),
                ("Vec", Action::Collect) => quote!({
                    if let Some(values) = #runtime::node_config::array(__mf_config)? {
                        for (index, value) in values.iter().enumerate() {
                            let prefix = #runtime::node_config::join_config_path(__mf_prefix, &index.to_string());
                            let __mf_prefix = prefix.as_str(); let __mf_config = Some(value); #inner?;
                        }
                    }
                    Ok::<(), #runtime::NodeBuildError>(())
                }),
                ("Vec", Action::Decode) => quote!({
                    let mut result = ::std::vec::Vec::new();
                    for (index, value) in #runtime::node_config::take_array(__mf_config)?.into_iter().enumerate() {
                        let prefix = #runtime::node_config::join_config_path(__mf_prefix, &index.to_string());
                        let __mf_prefix = prefix.as_str(); let __mf_config = &mut Some(value); result.push(#inner?);
                    }
                    Ok::<_, #runtime::NodeBuildError>(result)
                }),
                _ => unreachable!(),
            };
            return container_traversal(ty, &canonical, action, runtime, body);
        }
        if matches!(kind.as_str(), "BTreeMap" | "HashMap") && args.len() == 2 && is_string(args[0])
        {
            let inner = traversal(args[1], action, runtime);
            let element = args[1];
            let canonical: Type = if kind == "BTreeMap" {
                parse_quote!(::std::collections::BTreeMap<::std::string::String, #element>)
            } else {
                parse_quote!(::std::collections::HashMap<::std::string::String, #element>)
            };
            let body = match action {
                Action::Contains => inner,
                Action::IsLeaf => quote!(false),
                Action::Collect => quote!({
                    if let Some(values) = #runtime::node_config::object(__mf_config)? {
                        for (key, value) in values {
                            let prefix = #runtime::node_config::join_config_path(__mf_prefix, key);
                            let __mf_prefix = prefix.as_str(); let __mf_config = Some(value); #inner?;
                        }
                    }
                    Ok::<(), #runtime::NodeBuildError>(())
                }),
                Action::Decode => quote!({
                    let mut result: #canonical = ::core::default::Default::default();
                    for (key, value) in #runtime::node_config::take_object(__mf_config)? {
                        let prefix = #runtime::node_config::join_config_path(__mf_prefix, &key);
                        let __mf_prefix = prefix.as_str(); let __mf_config = &mut Some(value); result.insert(key, #inner?);
                    }
                    Ok::<_, #runtime::NodeBuildError>(result)
                }),
            };
            return container_traversal(ty, &canonical, action, runtime, body);
        }
    }
    if let Type::Array(array) = ty {
        let inner = &array.elem;
        let len = &array.len;
        let vector: Type = parse_quote!(::std::vec::Vec<#inner>);
        let nested = traversal(&vector, action, runtime);
        return match action {
            Action::Decode => quote!({
                let values: ::std::vec::Vec<#inner> = #nested?;
                match values.try_into() {
                    Ok(values) => Ok(values),
                    Err(_) => #runtime::NodeInvalidSubgraphSnafu {
                        message: ::std::format!("expected an array of length {}", #len),
                    }.fail(),
                }
            }),
            Action::Collect => quote!({
                if let Some(values) = #runtime::node_config::array(__mf_config)? {
                    if values.len() != #len { return Err(#runtime::node_config::invalid(::std::format!("expected an array of length {}", #len))); }
                }
                #nested
            }),
            _ => nested,
        };
    }
    if let Type::Tuple(tuple) = ty
        && !tuple.elems.is_empty()
    {
        let inner: Vec<_> = tuple
            .elems
            .iter()
            .map(|ty| traversal(ty, action, runtime))
            .collect();
        let indices: Vec<_> = (0..tuple.elems.len())
            .map(|index| LitStr::new(&index.to_string(), ty.span()))
            .collect();
        let len = inner.len();
        return match action {
            Action::Contains => quote!(false #(|| (#inner))*),
            Action::IsLeaf => quote!(false),
            Action::Collect => quote!({
                if let Some(values) = #runtime::node_config::array(__mf_config)? {
                    if values.len() != #len { return Err(#runtime::node_config::invalid("incorrect tuple length")); }
                    let mut values = values.iter();
                    #({ let prefix = #runtime::node_config::join_config_path(__mf_prefix, #indices);
                        let __mf_prefix = prefix.as_str(); let __mf_config = values.next(); #inner?; })*
                }
                Ok::<(), #runtime::NodeBuildError>(())
            }),
            Action::Decode => quote!({
                let values = #runtime::node_config::take_array(__mf_config)?;
                if values.len() != #len { return Err(#runtime::node_config::invalid("incorrect tuple length")); }
                let mut values = values.into_iter();
                Ok::<_, #runtime::NodeBuildError>((#({ let prefix = #runtime::node_config::join_config_path(__mf_prefix, #indices);
                    let __mf_prefix = prefix.as_str(); let __mf_config = &mut values.next(); #inner? },)*))
            }),
        };
    }
    let receiver = quote!((&&#runtime::node_config::ConfigField::<#ty>::default()));
    match action {
        Action::Collect => quote!(#receiver.field_collect(__mf_config, __mf_prefix, __mf_paths)),
        Action::Decode => quote!(#receiver.field_decode(__mf_config, __mf_prefix, __mf_flows)),
        Action::Contains => quote!(#receiver.field_contains(__mf_visited)),
        Action::IsLeaf => quote!(#receiver.field_is_leaf()),
    }
}

// The extra autoref level gives a manual NodeConfig implementation priority over
// syntax. The generated callback is usable only when Actual == Canonical; a
// foreign Deserialize wrapper otherwise takes the opaque fallback.
fn container_traversal(
    actual: &Type,
    canonical: &Type,
    action: Action,
    runtime: &Path,
    body: TokenStream,
) -> TokenStream {
    let receiver =
        quote!((&&&#runtime::node_config::ConfigField::<#actual, #canonical>::default()));
    match action {
        Action::Collect => quote!(#receiver.container_collect(__mf_config, __mf_prefix, __mf_paths,
            |__mf_config, __mf_prefix, __mf_paths| { #body })),
        Action::Decode => quote!(#receiver.container_decode(__mf_config, __mf_prefix, __mf_flows,
            |__mf_config, __mf_prefix, __mf_flows| -> ::core::result::Result<#canonical, #runtime::NodeBuildError> { #body })),
        Action::Contains => {
            quote!(#receiver.container_contains(__mf_visited, |__mf_visited| { #body }))
        }
        Action::IsLeaf => quote!(#receiver.container_is_leaf(|| { #body })),
    }
}

fn is_string(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "String"))
}
fn empty_default_policy(
    ty: &Type,
    runtime: &Path,
    value: &syn::Ident,
) -> (TokenStream, TokenStream) {
    let canonical = (|| {
        let Type::Path(path) = ty else {
            return None;
        };
        let segment = path.path.segments.last()?;
        let PathArguments::AngleBracketed(args) = &segment.arguments else {
            return None;
        };
        let args: Vec<_> = args
            .args
            .iter()
            .filter_map(|arg| {
                if let GenericArgument::Type(ty) = arg {
                    Some(ty)
                } else {
                    None
                }
            })
            .collect();
        match (segment.ident.to_string().as_str(), args.as_slice()) {
            ("Option", [element]) => Some((parse_quote!(::std::option::Option<#element>), true)),
            ("Vec", [element]) => Some((parse_quote!(::std::vec::Vec<#element>), false)),
            ("BTreeMap", [key, element]) if is_string(key) => Some((
                parse_quote!(::std::collections::BTreeMap<::std::string::String, #element>),
                false,
            )),
            ("HashMap", [key, element]) if is_string(key) => Some((
                parse_quote!(::std::collections::HashMap<::std::string::String, #element>),
                false,
            )),
            _ => None,
        }
    })();
    if let Some((canonical, optional)) = canonical {
        let canonical: Type = canonical;
        let receiver =
            quote!((&&&#runtime::node_config::ConfigField::<#ty, #canonical>::default()));
        let empty = if optional {
            quote!(|value| value.is_none())
        } else {
            quote!(|value| value.is_empty())
        };
        (
            quote!(#receiver.container_default_allowed()),
            quote!(#receiver.container_default_empty(&#value, #empty)),
        )
    } else {
        let receiver = quote!((&&#runtime::node_config::ConfigField::<#ty>::default()));
        (
            quote!(#receiver.field_default_allowed()),
            quote!(#receiver.field_default_empty(&#value)),
        )
    }
}

// Field helper items have their own `Self`; preserve the original receiver in
// concrete types as well as in Serde's string-valued function paths.
fn qualify_field_type(ty: &mut Type, owner: &syn::Ident) {
    match ty {
        Type::Path(path) => {
            if let Some(qself) = &mut path.qself {
                qualify_field_type(&mut qself.ty, owner);
            }
            if let Some(first) = path.path.segments.first_mut()
                && first.ident == "Self"
            {
                first.ident = owner.clone();
            }
            for segment in &mut path.path.segments {
                if let PathArguments::AngleBracketed(args) = &mut segment.arguments {
                    for arg in &mut args.args {
                        if let GenericArgument::Type(ty) = arg {
                            qualify_field_type(ty, owner);
                        }
                    }
                }
            }
        }
        Type::Array(array) => qualify_field_type(&mut array.elem, owner),
        Type::Slice(slice) => qualify_field_type(&mut slice.elem, owner),
        Type::Reference(reference) => qualify_field_type(&mut reference.elem, owner),
        Type::Ptr(pointer) => qualify_field_type(&mut pointer.elem, owner),
        Type::Paren(paren) => qualify_field_type(&mut paren.elem, owner),
        Type::Group(group) => qualify_field_type(&mut group.elem, owner),
        Type::Tuple(tuple) => {
            for ty in &mut tuple.elems {
                qualify_field_type(ty, owner);
            }
        }
        _ => {}
    }
}

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
        "NodeConfig requires a string value",
    ))
}
fn option_string(options: &[Meta], key: &str) -> syn::Result<Option<LitStr>> {
    let mut result = None;
    for option in options.iter().filter(|meta| meta.path().is_ident(key)) {
        if result.is_some() {
            return Err(syn::Error::new_spanned(option, "duplicate serde option"));
        }
        result = Some(string_value(option)?);
    }
    Ok(result)
}
struct FieldWire {
    canonical: LitStr,
    aliases: Vec<LitStr>,
}

fn field_wires(fields: &[serde_derive_internals::ast::Field<'_>]) -> Vec<FieldWire> {
    fields
        .iter()
        .map(|field| {
            let wire = field.attrs.name().deserialize_name();
            FieldWire {
                canonical: LitStr::new(&wire.value, wire.span),
                aliases: field
                    .attrs
                    .aliases()
                    .iter()
                    .filter(|alias| alias.value != wire.value)
                    .map(|alias| LitStr::new(&alias.value, alias.span))
                    .collect(),
            }
        })
        .collect()
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
