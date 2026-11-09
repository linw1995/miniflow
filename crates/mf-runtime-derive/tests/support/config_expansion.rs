use crate::config::expand;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Path, parse_quote};

fn assert_expansion(input: DeriveInput, runtime: Path, parameters: &[&str]) {
    let name = &input.ident;
    let prefix = quote! { impl #runtime::NodeConfig for #name }.to_string();
    let extractions = parameters.iter().map(|name| {
        let pointer = format!("/{}", name.replace('~', "~0").replace('/', "~1"));
        quote! {
            if let ::core::option::Option::Some(value) = __mf_config.get(#name).filter(|value| !value.is_null()) {
                flows.insert(::std::string::String::from(#pointer), value.clone());
            }
        }
    });
    let declaration = quote! {
        fn flow_parameters(
            __mf_config: &::serde_json::Value,
        ) -> ::std::collections::BTreeMap<::std::string::String, ::serde_json::Value> {
            let mut flows = ::std::collections::BTreeMap::new();
            #(#extractions)*
            flows
        }
    }
    .to_string();
    let expanded = expand(input).unwrap().to_string();
    assert!(expanded.starts_with(&prefix), "{expanded}");
    for expected in [
        declaration.as_str(),
        "fn from_config",
        "deserialize_config :: < __MfConfigOrdinaryConfig >",
        "unexpected prepared Flow field",
    ] {
        assert!(
            expanded.contains(expected),
            "missing {expected:?}: {expanded}"
        );
    }
}

fn assert_error(input: DeriveInput, message: &str) {
    let error = expand(input).unwrap_err().to_string();
    assert!(
        error.contains(message),
        "expected {message:?}, got {error:?}"
    );
}

fn ordinary_config(attrs: TokenStream) -> DeriveInput {
    parse_quote! {
        struct Config {
            #attrs
            ordinary: String,
            body: Flow,
        }
    }
}

fn assert_field_order_errors(ordinary: TokenStream, flow: TokenStream, message: &str) {
    for fields in [quote!(#ordinary #flow), quote!(#flow #ordinary)] {
        assert_error(parse_quote!(struct Config { #fields }), message);
    }
}

#[test]
fn discovers_direct_and_optional_fields_in_declaration_order() {
    assert_expansion(
        parse_quote! {
            struct Config {
                body: Flow,
                count: usize,
                left: Option<Flow>,
                right: ::std::option::Option<::mf_runtime::Flow>,
                qualified: runtime_alias::Flow,
                ordinary: Vec<String>,
                r#type: Flow,
            }
        },
        parse_quote!(::mf_runtime),
        &["body", "left", "right", "qualified", "type"],
    );
}

#[test]
fn supports_empty_and_ordinary_configs() {
    assert_expansion(
        parse_quote!(
            struct Config {}
        ),
        parse_quote!(::mf_runtime),
        &[],
    );
    assert_expansion(
        parse_quote! {
            struct Config {
                #[serde(flatten)]
                ordinary: std::collections::BTreeMap<String, String>,
                #[serde(alias = "other", skip_deserializing)]
                value: String,
                body: Option<Settings>,
            }
        },
        parse_quote!(::mf_runtime),
        &[],
    );
}

#[test]
fn honors_runtime_alias_and_serde_names_and_defaults() {
    assert_expansion(
        parse_quote! {
            #[config(runtime = "::runtime_alias")]
            #[serde(rename_all = "camelCase", deny_unknown_fields, default)]
            struct Config {
                request_body: Flow,
                #[serde(rename = "left-hand", default)]
                left_side: Option<Flow>,
                right_side: Flow,
                #[serde(rename = "settings", alias = "legacy", default)]
                user_settings: Settings,
            }
        },
        parse_quote!(::runtime_alias),
        &["requestBody", "left-hand", "rightSide"],
    );
}

#[test]
fn executable_fields_reject_defaults_that_could_construct_a_flow() {
    for ty in [
        quote!(Flow),
        quote!(Option<Flow>),
        quote!(PreparedSubgraph),
        quote!(Option<PreparedSubgraph>),
    ] {
        for (default, message) in [
            (
                quote!(default = "make_flow"),
                "executable defaults cannot construct a Flow",
            ),
            (
                quote!(default = 42),
                "expected serde default attribute to be a string",
            ),
            (quote!(default()), "expected `,`"),
            (quote!(default(foo)), "expected `,`"),
            (
                quote!(default, default),
                "duplicate serde attribute `default`",
            ),
        ] {
            assert_error(
                parse_quote! { struct Config { #[serde(#default)] body: #ty } },
                message,
            );
        }
    }
    for ty in [quote!(Flow), quote!(PreparedSubgraph)] {
        assert_error(
            parse_quote! { struct Config { #[serde(default)] body: #ty } },
            "bare `default` on optional fields",
        );
    }
}

#[test]
fn helper_projection_preserves_the_original_config_scope() {
    let expanded = expand(parse_quote! {
        #[serde(default = "Self::defaults")]
        struct OrdinaryConfig {
            #[serde(default = "Self::default_count", deserialize_with = "Self::decode_count", serialize_with = "Self::encode_count", skip_serializing_if = "Self::skip_count")]
            count: usize,
            #[serde(with = "Self::codec")]
            settings: Settings,
            nested: OrdinaryConfig,
        }
    }).unwrap().to_string();
    for expected in [
        "struct __MfOrdinaryConfigOrdinaryConfig",
        "nested : OrdinaryConfig",
        "OrdinaryConfig :: default_count",
        "OrdinaryConfig :: decode_count",
        "OrdinaryConfig :: encode_count",
        "OrdinaryConfig :: skip_count",
        "OrdinaryConfig :: codec",
        "let _config = OrdinaryConfig :: defaults ()",
    ] {
        assert!(
            expanded.contains(expected),
            "missing {expected:?}: {expanded}"
        );
    }
    assert!(!expanded.contains("Self::"), "{expanded}");
}

#[test]
fn supports_all_serde_field_rename_rules() {
    for (rule, field, expected) in [
        ("lowercase", quote!(request_body), "request_body"),
        ("UPPERCASE", quote!(request_body), "REQUEST_BODY"),
        ("PascalCase", quote!(request_body), "RequestBody"),
        ("camelCase", quote!(request_body), "requestBody"),
        ("snake_case", quote!(request_body), "request_body"),
        ("SCREAMING_SNAKE_CASE", quote!(request_body), "REQUEST_BODY"),
        ("kebab-case", quote!(request_body), "request-body"),
        ("SCREAMING-KEBAB-CASE", quote!(request_body), "REQUEST-BODY"),
        // Serde does not lowercase a field identifier for its `lowercase` rule.
        ("lowercase", quote!(mixedCase), "mixedCase"),
    ] {
        assert_expansion(
            parse_quote! {
                #[serde(rename_all = #rule)]
                struct Config { #field: Flow }
            },
            parse_quote!(::mf_runtime),
            &[expected],
        );
    }
}

#[test]
fn rejects_ordinary_deserialize_names_and_aliases_that_shadow_flow_names() {
    for option in [
        quote!(alias = "body"),
        quote!(alias = "unused", alias = "body"),
        quote!(rename = "body"),
        quote!(rename(serialize = "output", deserialize = "body")),
        quote!(rename(deserialize = "body")),
        quote!(rename(deserialize = "body"), rename(serialize = "output")),
        quote!(rename(serialize = "output"), rename(deserialize = "body")),
    ] {
        assert_field_order_errors(
            quote! {
                #[serde(#option)]
                settings: Option<Value>,
            },
            quote!(body: Option<Flow>,),
            "collides with an ordinary field's deserialize name or alias",
        );
    }
    for (attrs, ordinary, wire_name) in [
        (quote!(), quote!(body: String,), "body"),
        (
            quote!(#[serde(rename_all = "camelCase")]),
            quote!(request_body: String,),
            "requestBody",
        ),
        (
            quote!(#[serde(rename_all = "camelCase")]),
            quote!(#[serde(rename(serialize = "output"))] request_body: String,),
            "requestBody",
        ),
    ] {
        assert_error(
            parse_quote! {
                #attrs
                struct Config { #ordinary #[serde(rename = #wire_name)] flow: Flow }
            },
            "collides with an ordinary field's deserialize name or alias",
        );
    }
}

#[test]
fn ordinary_deserialize_names_ignore_serialize_names_and_custom_codecs() {
    assert_expansion(
        parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct Config {
                #[serde(rename(serialize = "body", deserialize = "settings"))]
                body: String,
                #[serde(rename(serialize = "body"), deserialize_with = "custom")]
                other_settings: String,
                #[serde(alias = "moreSettings")]
                extra_settings: String,
                #[serde(rename = "body")]
                flow: Flow,
            }
        },
        parse_quote!(::mf_runtime),
        &["body"],
    );
}

#[test]
fn rejects_any_flattened_field_when_flow_fields_exist() {
    for ty in [
        quote!(Settings),
        quote!(std::collections::BTreeMap<String, Value>),
    ] {
        assert_field_order_errors(
            quote! {
                #[serde(flatten)]
                settings: #ty,
            },
            quote!(body: Flow,),
            "cannot combine Flow parameter fields with serde flatten",
        );
    }
}

#[test]
fn discovers_literal_type_spellings_and_ignores_differently_named_aliases() {
    assert_expansion(
        parse_quote! {
            struct Config {
                body: other::Flow,
                left: other::Option<other::Flow>,
                alias: RenamedFlow,
            }
        },
        parse_quote!(::mf_runtime),
        &["body", "left"],
    );
}

#[test]
fn rejects_non_named_structs_and_generics() {
    for input in [
        parse_quote!(
            struct Config(Flow);
        ),
        parse_quote!(
            struct Config;
        ),
        parse_quote!(
            enum Config {
                Body,
            }
        ),
        parse_quote!(union Config { body: usize }),
    ] {
        assert_error(input, "requires a named-field struct");
    }
    for (generics, ty) in [
        (quote!(<T>), quote!(T)),
        (quote!(<'a>), quote!(&'a str)),
        (quote!(<const N: usize>), quote!([u8; N])),
    ] {
        assert_error(
            parse_quote!(struct Config #generics { ordinary: #ty }),
            "does not support generic structs",
        );
    }
}

#[test]
fn rejects_nested_flow_parameters_instead_of_silently_ignoring_them() {
    for ty in [
        quote!(Vec<Flow>),
        quote!(std::collections::BTreeMap<String, runtime::Flow>),
        quote!(Option<Vec<Flow>>),
        quote!(Option<Option<Flow>>),
        quote!(Box<Flow>),
        quote!([Flow; 2]),
        quote!((String, Flow)),
        quote!(&Flow),
    ] {
        assert_error(
            parse_quote!(struct Config { body: #ty }),
            "only direct Flow or Option<Flow>",
        );
    }
}

#[test]
fn rejects_serde_options_that_change_flow_field_contracts() {
    for option in [
        quote!(flatten),
        quote!(alias = "other"),
        quote!(deserialize_with = "custom"),
        quote!(serialize_with = "custom"),
        quote!(with = "custom"),
        quote!(skip),
        quote!(skip_deserializing),
        quote!(skip_serializing),
        quote!(skip_serializing_if = "Option::is_none"),
    ] {
        assert_error(
            parse_quote! {
                struct Config {
                    #[serde(#option)]
                    body: Option<Flow>,
                }
            },
            "unsupported serde option on a Flow parameter field",
        );
    }
    for option in [
        quote!(from = "Other"),
        quote!(try_from = "Other"),
        quote!(transparent),
    ] {
        assert_error(
            parse_quote! {
                #[serde(#option)]
                struct Config { body: Flow }
            },
            "unsupported serde container option",
        );
    }
}

#[test]
fn rejects_directional_renames_and_invalid_rules() {
    for (container, field, message) in [
        (
            quote!(),
            quote!(#[serde(rename(deserialize = "body"))]),
            "directional Serde renames are unsupported",
        ),
        (
            quote!(#[serde(rename_all(deserialize = "camelCase"))]),
            quote!(),
            "directional Serde renames are unsupported",
        ),
        (
            quote!(#[serde(rename_all = "unknown")]),
            quote!(),
            "unknown rename rule",
        ),
    ] {
        assert_error(
            parse_quote!(#container struct Config { #field body: Flow }),
            message,
        );
    }
}

#[test]
fn rejects_duplicate_names_and_attributes() {
    assert_error(
        parse_quote! {
            struct Config {
                #[serde(rename = "body")]
                left: Flow,
                #[serde(rename = "body")]
                right: Flow,
            }
        },
        "duplicate Flow parameter wire name",
    );
    for input in [
        parse_quote! {
            struct Config {
                #[serde(rename = "settings")]
                left: String,
                #[serde(rename = "settings")]
                right: Settings,
            }
        },
        parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct Config {
                user_settings: String,
                userSettings: Settings,
                body: Flow,
            }
        },
    ] {
        assert_error(input, "duplicate configuration parameter wire name");
    }
    assert_error(
        parse_quote! {
            struct Config {
                #[serde(rename = "a", rename = "b")]
                body: Flow,
            }
        },
        "duplicate serde attribute `rename`",
    );
    for (attrs, message) in [
        (
            quote!(#[serde(rename_all = "camelCase", rename_all = "snake_case")]),
            "duplicate serde attribute `rename_all`",
        ),
        (
            quote!(#[config(runtime = "::a", runtime = "::b")]),
            "duplicate runtime path",
        ),
    ] {
        assert_error(parse_quote!(#attrs struct Config { body: Flow }), message);
    }
}

#[test]
fn rejects_unknown_config_keys_instead_of_ignoring_runtime_typos() {
    for option in [
        quote!(runtme = "::runtime_alias"),
        quote!(rename = "Config"),
    ] {
        assert_error(
            parse_quote! {
                #[config(#option)]
                struct Config { body: Flow }
            },
            "expected `runtime = \"path\"` on the config struct",
        );
    }
}

#[test]
fn rejects_malformed_runtime_paths_and_non_string_runtime_values() {
    for (option, message) in [
        (quote!(runtime = "not a path"), "unexpected token"),
        (quote!(runtime = 42), "expected string literal"),
        (quote!(runtime), "expected `=`"),
    ] {
        assert_error(
            parse_quote! {
                #[config(#option)]
                struct Config { body: Flow }
            },
            message,
        );
    }
    assert_error(
        parse_quote! {
            #[config(runtime = "::first")]
            #[config(runtime = "::second")]
            struct Config { body: Flow }
        },
        "duplicate runtime path",
    );
}

#[test]
fn rejects_ambiguous_ordinary_deserialize_renames() {
    for options in [
        quote!(#[serde(rename = "first", rename = "second")]),
        quote!(#[serde(rename(deserialize = "first"), rename(deserialize = "second"))]),
        quote!(#[serde(rename = "first", rename(deserialize = "second"))]),
        quote!(#[serde(rename(deserialize = "first"), rename = "second")]),
        quote!(#[serde(rename = "first")] #[serde(rename = "second")]),
    ] {
        assert_error(
            ordinary_config(options),
            "duplicate serde attribute `rename`",
        );
    }
}

#[test]
fn rejects_unknown_ordinary_rename_directions() {
    for direction in [quote!(deserialise), quote!(alias), quote!(unknown)] {
        assert_error(
            ordinary_config(quote!(#[serde(rename(#direction = "settings"))])),
            "malformed rename attribute",
        );
    }
}

#[test]
fn rejects_repeated_directions_inside_an_ordinary_rename() {
    for options in [
        quote!(serialize = "first", serialize = "second"),
        quote!(deserialize = "first", deserialize = "second"),
        quote!(
            serialize = "output",
            deserialize = "input",
            serialize = "other"
        ),
        quote!(
            deserialize = "input",
            serialize = "output",
            deserialize = "other"
        ),
    ] {
        assert_error(
            ordinary_config(quote!(#[serde(rename(#options))])),
            "duplicate serde attribute `rename`",
        );
    }
}

#[test]
fn rejects_non_string_serde_wire_names_for_ordinary_and_flow_fields() {
    for (options, message) in [
        (vec![quote!(rename)], "expected `=` or parentheses"),
        (
            vec![
                quote!(rename = 42),
                quote!(rename = wire_name()),
                quote!(rename(deserialize = 42)),
                quote!(rename(serialize = false)),
            ],
            "expected serde rename attribute to be a string",
        ),
        (
            vec![quote!(alias = false), quote!(alias = SOME_NAME)],
            "expected serde alias attribute to be a string",
        ),
    ] {
        for option in options {
            assert_error(ordinary_config(quote!(#[serde(#option)])), message);
        }
    }
    assert_error(
        parse_quote! {
            struct Config {
                #[serde(rename = 42)]
                body: Flow,
            }
        },
        "expected serde rename attribute to be a string",
    );
    assert_error(
        parse_quote! {
            #[serde(rename_all = false)]
            struct Config { body: Flow }
        },
        "NodeConfig requires a string value",
    );
}

#[test]
fn rejects_malformed_serde_option_lists() {
    for input in [
        parse_quote! {
            #[serde(rename_all = "camelCase" default)]
            struct Config { body: Flow }
        },
        parse_quote! {
            struct Config {
                #[serde(rename = "body" default)]
                body: Flow,
            }
        },
        ordinary_config(quote!(#[serde(rename(deserialize = "input" serialize = "output"))])),
    ] {
        assert_error(input, "expected `,`");
    }
}

#[test]
fn ignores_ordinary_associated_types_and_unparameterized_option_names() {
    assert_expansion(
        parse_quote! {
            struct Config {
                ordinary: <Settings as Trait>::Value,
                named_option: Option,
                qualified_option: other::Option,
                body: Flow,
            }
        },
        parse_quote!(::mf_runtime),
        &["body"],
    );
    assert_error(
        parse_quote! {
            struct Config {
                body: <Settings as Trait>::Flow,
            }
        },
        "only direct Flow or Option<Flow>",
    );
}

#[test]
fn injects_flow_and_existing_host_policy_handles_without_deserializing_them() {
    assert_expansion(
        parse_quote! {
            struct Config {
                compiled: Flow,
                optional: Option<Flow>,
                callback: PreparedSubgraph,
                optional_callback: Option<runtime_alias::PreparedSubgraph>,
            }
        },
        parse_quote!(::mf_runtime),
        &["compiled", "optional", "callback", "optional_callback"],
    );
    let expanded = expand(parse_quote! {
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        struct Config {
            executable_body: Flow,
            #[serde(alias = "legacy", default, deserialize_with = "decode_count")]
            count: usize,
        }
    })
    .unwrap()
    .to_string();
    for expected in [
        "struct __MfConfigOrdinaryConfig { # [serde (alias = \"legacy\" , default , deserialize_with = \"decode_count\")] count : usize }",
        "deny_unknown_fields , rename_all = \"camelCase\"",
        "fields . remove (\"executableBody\")",
        "prepared . into_flow () ?",
        "present != prepared . is_some ()",
        "missing prepared Flow field",
    ] {
        assert!(
            expanded.contains(expected),
            "missing {expected:?}: {expanded}"
        );
    }
}

#[test]
fn executable_field_names_cannot_shadow_decoder_working_values() {
    let expanded = expand(parse_quote! {
        struct Config { config: Flow, flows: Flow, __mf_config: Option<Flow> }
    })
    .unwrap()
    .to_string();
    for expected in [
        "mut __mf_config : :: serde_json :: Value",
        "mut __mf_flows : :: std :: collections :: BTreeMap",
        "let config =",
        "let flows =",
        "__mf_config . as_object_mut ()",
        "__mf_flows . remove (\"/flows\")",
    ] {
        assert!(
            expanded.contains(expected),
            "missing {expected:?}: {expanded}"
        );
    }
}

#[test]
fn projects_container_defaults_onto_only_ordinary_fields() {
    for default in [quote!(default), quote!(default = "default_config")] {
        let expanded = expand(parse_quote! {
            #[serde(#default)]
            struct Config { body: Flow, count: usize }
        })
        .unwrap()
        .to_string();
        assert!(
            expanded.contains("impl :: core :: default :: Default for __MfConfigOrdinaryConfig"),
            "{expanded}"
        );
        assert!(
            expanded.contains("Self { count : _config . count }"),
            "{expanded}"
        );
        assert!(!expanded.contains("body : _config . body"), "{expanded}");
    }
    for ty in [
        quote!(Vec<PreparedSubgraph>),
        quote!(Option<Option<PreparedSubgraph>>),
    ] {
        assert_error(
            parse_quote! { struct Config { body: #ty } },
            "nested containers",
        );
    }
}

#[test]
fn explicit_rename_gives_underscore_only_fields_a_valid_wire_name() {
    assert_expansion(
        parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct Config {
                #[serde(rename = "body/~")]
                __: Flow,
            }
        },
        parse_quote!(::mf_runtime),
        &["body/~"],
    );
}
