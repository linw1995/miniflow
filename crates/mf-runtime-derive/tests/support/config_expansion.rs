use crate::config::expand;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Path, parse_quote};

enum ParameterType {
    Value,
    Flow,
    OptionalFlow,
}

use ParameterType::{Flow, OptionalFlow, Value};

fn assert_expansion(input: DeriveInput, runtime: Path, parameters: &[(&str, ParameterType)]) {
    let name = &input.ident;
    let prefix = quote! { impl #runtime::NodeConfig for #name }.to_string();
    let entries = parameters.iter().map(|(name, ty)| {
        let ty = match ty {
            Value => quote!(#runtime::ConfigParameterType::Value),
            Flow => quote!(#runtime::ConfigParameterType::Flow),
            OptionalFlow => quote!(#runtime::ConfigParameterType::Optional(::std::boxed::Box::new(
                #runtime::ConfigParameterType::Flow
            ))),
        };
        quote!((#name.into(), #ty))
    });
    let declaration = quote! {
        fn parameter_types() -> #runtime::ConfigParameterTypes {
            #runtime::ConfigParameterTypes::from([#(#entries),*])
        }
    }
    .to_string();
    let expanded = expand(input).unwrap().to_string();
    assert!(expanded.starts_with(&prefix), "{expanded}");
    assert!(expanded.contains(&declaration), "{expanded}");
    assert!(expanded.contains("fn from_config"), "{expanded}");
    assert!(
        expanded.contains("deserialize_config :: < __MfConfigOrdinaryConfig >"),
        "{expanded}"
    );
    assert!(
        expanded.contains("unexpected prepared Flow field"),
        "{expanded}"
    );
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
        &[
            ("body", Flow),
            ("count", Value),
            ("left", OptionalFlow),
            ("right", OptionalFlow),
            ("qualified", Flow),
            ("ordinary", Value),
            ("type", Flow),
        ],
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
        &[("ordinary", Value), ("value", Value), ("body", Value)],
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
        &[
            ("requestBody", Flow),
            ("left-hand", OptionalFlow),
            ("rightSide", Flow),
            ("settings", Value),
        ],
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
        for default in [
            quote!(default = "make_flow"),
            quote!(default = 42),
            quote!(default()),
            quote!(default(foo)),
            quote!(default, default),
        ] {
            assert_error(
                parse_quote! { struct Config { #[serde(#default)] body: #ty } },
                "executable defaults cannot construct a Flow",
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
    assert!(
        expanded.contains("struct __MfOrdinaryConfigOrdinaryConfig"),
        "{expanded}"
    );
    assert!(expanded.contains("nested : OrdinaryConfig"), "{expanded}");
    for method in [
        "default_count",
        "decode_count",
        "encode_count",
        "skip_count",
        "codec",
    ] {
        assert!(
            expanded.contains(&format!("OrdinaryConfig :: {method}")),
            "{expanded}"
        );
    }
    assert!(
        expanded.contains("let _config = OrdinaryConfig :: defaults ()"),
        "{expanded}"
    );
    assert!(!expanded.contains("Self::"), "{expanded}");
}

#[test]
fn supports_all_serde_field_rename_rules() {
    for (rule, expected) in [
        ("lowercase", "request_body"),
        ("UPPERCASE", "REQUEST_BODY"),
        ("PascalCase", "RequestBody"),
        ("camelCase", "requestBody"),
        ("snake_case", "request_body"),
        ("SCREAMING_SNAKE_CASE", "REQUEST_BODY"),
        ("kebab-case", "request-body"),
        ("SCREAMING-KEBAB-CASE", "REQUEST-BODY"),
    ] {
        assert_expansion(
            parse_quote! {
                #[serde(rename_all = #rule)]
                struct Config { request_body: Flow }
            },
            parse_quote!(::mf_runtime),
            &[(expected, Flow)],
        );
    }
    // Serde does not lowercase a field identifier for its `lowercase` rule.
    assert_expansion(
        parse_quote! {
            #[serde(rename_all = "lowercase")]
            struct Config { mixedCase: Flow }
        },
        parse_quote!(::mf_runtime),
        &[("mixedCase", Flow)],
    );
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
    assert_error(
        parse_quote! {
            struct Config {
                body: String,
                #[serde(rename = "body")]
                flow: Flow,
            }
        },
        "collides with an ordinary field's deserialize name or alias",
    );
    assert_error(
        parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct Config {
                request_body: String,
                #[serde(rename = "requestBody")]
                flow: Flow,
            }
        },
        "collides with an ordinary field's deserialize name or alias",
    );
    assert_error(
        parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct Config {
                #[serde(rename(serialize = "output"))]
                request_body: String,
                #[serde(rename = "requestBody")]
                flow: Flow,
            }
        },
        "collides with an ordinary field's deserialize name or alias",
    );
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
        &[
            ("settings", Value),
            ("otherSettings", Value),
            ("extraSettings", Value),
            ("body", Flow),
        ],
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
        &[("body", Flow), ("left", OptionalFlow), ("alias", Value)],
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
    for input in [
        parse_quote!(
            struct Config<T> {
                ordinary: T,
            }
        ),
        parse_quote!(
            struct Config<'a> {
                ordinary: &'a str,
            }
        ),
        parse_quote!(
            struct Config<const N: usize> {
                ordinary: [u8; N],
            }
        ),
    ] {
        assert_error(input, "does not support generic structs");
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
    assert_error(
        parse_quote! {
            struct Config {
                #[serde(rename(deserialize = "body"))]
                body: Flow,
            }
        },
        "directional Serde renames are unsupported",
    );
    assert_error(
        parse_quote! {
            #[serde(rename_all(deserialize = "camelCase"))]
            struct Config { body: Flow }
        },
        "directional Serde renames are unsupported",
    );
    assert_error(
        parse_quote! {
            #[serde(rename_all = "unknown")]
            struct Config { body: Flow }
        },
        "unsupported serde rename_all rule",
    );
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
        "duplicate serde rename",
    );
    assert_error(
        parse_quote! {
            #[serde(rename_all = "camelCase", rename_all = "snake_case")]
            struct Config { body: Flow }
        },
        "duplicate serde rename_all",
    );
    assert_error(
        parse_quote! {
            #[config(runtime = "::a", runtime = "::b")]
            struct Config { body: Flow }
        },
        "duplicate runtime path",
    );
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
            "duplicate serde deserialize rename",
        );
    }
}

#[test]
fn rejects_unknown_ordinary_rename_directions() {
    for direction in [quote!(deserialise), quote!(alias), quote!(unknown)] {
        assert_error(
            ordinary_config(quote!(#[serde(rename(#direction = "settings"))])),
            "expected `serialize` or `deserialize` in serde rename",
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
            "duplicate serde rename direction",
        );
    }
}

#[test]
fn rejects_non_string_serde_wire_names_for_ordinary_and_flow_fields() {
    for option in [
        quote!(rename),
        quote!(rename = 42),
        quote!(rename = wire_name()),
        quote!(alias = false),
        quote!(alias = SOME_NAME),
        quote!(rename(deserialize = 42)),
        quote!(rename(serialize = false)),
    ] {
        assert_error(
            ordinary_config(quote!(#[serde(#option)])),
            "NodeConfig requires a string value",
        );
    }
    assert_error(
        parse_quote! {
            struct Config {
                #[serde(rename = 42)]
                body: Flow,
            }
        },
        "NodeConfig requires a string value",
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
        parse_quote! {
            struct Config {
                #[serde(rename(deserialize = "input" serialize = "output"))]
                ordinary: String,
                body: Flow,
            }
        },
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
        &[
            ("ordinary", Value),
            ("named_option", Value),
            ("qualified_option", Value),
            ("body", Flow),
        ],
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
        &[
            ("compiled", Flow),
            ("optional", OptionalFlow),
            ("callback", Flow),
            ("optional_callback", OptionalFlow),
        ],
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
    assert!(expanded.contains("struct __MfConfigOrdinaryConfig { # [serde (alias = \"legacy\" , default , deserialize_with = \"decode_count\")] count : usize }"), "{expanded}");
    assert!(
        expanded.contains("deny_unknown_fields , rename_all = \"camelCase\""),
        "{expanded}"
    );
    assert!(
        expanded.contains("fields . remove (\"executableBody\")"),
        "{expanded}"
    );
    assert!(expanded.contains("prepared . into_flow () ?"), "{expanded}");
    assert!(
        expanded.contains("present != prepared . is_some ()"),
        "{expanded}"
    );
    assert!(
        expanded.contains("missing prepared Flow field"),
        "{expanded}"
    );
}

#[test]
fn executable_field_names_cannot_shadow_decoder_working_values() {
    let expanded = expand(parse_quote! {
        struct Config { config: Flow, flows: Flow, __mf_config: Option<Flow> }
    })
    .unwrap()
    .to_string();
    assert!(
        expanded.contains("mut __mf_config : :: serde_json :: Value"),
        "{expanded}"
    );
    assert!(
        expanded.contains("mut __mf_flows : :: std :: collections :: BTreeMap"),
        "{expanded}"
    );
    assert!(expanded.contains("let config ="), "{expanded}");
    assert!(expanded.contains("let flows ="), "{expanded}");
    assert!(
        expanded.contains("__mf_config . as_object_mut ()"),
        "{expanded}"
    );
    assert!(
        expanded.contains("__mf_flows . remove (\"flows\")"),
        "{expanded}"
    );
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
fn camel_case_handles_underscore_only_names_without_panicking() {
    // Serde's field rule removes underscores, even when no letters remain.
    assert_expansion(
        parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct Config { __: Flow }
        },
        parse_quote!(::mf_runtime),
        &[("", Flow)],
    );
}
