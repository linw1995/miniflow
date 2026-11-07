use crate::config::expand;
use quote::quote;
use syn::{DeriveInput, parse_quote};

fn expansion(input: DeriveInput) -> String {
    expand(input).unwrap().to_string()
}
fn assert_error(input: DeriveInput, expected: &str) {
    let error = expand(input).unwrap_err().to_string();
    assert!(
        error.contains(expected),
        "expected {expected:?}, got {error:?}"
    );
}

#[test]
fn discovers_actual_field_types_instead_of_literal_flow_spellings() {
    let result = expansion(parse_quote! {
        struct Config { body: ExecutableAlias, ordinary: UserDefinedDeserialize, arbitrary: serde_json::Value }
    });
    assert!(
        result.contains("ConfigField :: < ExecutableAlias >"),
        "{result}"
    );
    assert!(
        result.contains("ConfigField :: < UserDefinedDeserialize >"),
        "{result}"
    );
    assert!(
        result.contains("ConfigField :: < serde_json :: Value >"),
        "{result}"
    );
    assert!(result.contains("field_collect"));
    assert!(result.contains("field_decode"));
    assert!(!result.contains("FLOW_PARAMETERS"));
    assert!(!result.contains("serde_json :: from_value :: < ExecutableAlias"));
}

#[test]
fn nested_standard_containers_dispatch_at_concrete_leaf_sites() {
    let result = expansion(parse_quote! {
        struct Config {
            body: Option<Box<Tree>>,
            mixed: (Flow, UserDefinedDeserialize),
            list: Vec<UserDefinedDeserialize>,
            map: std::collections::BTreeMap<String, Vec<FlowAlias>>,
            array: [Flow; 3],
        }
    });
    for expected in [
        "ConfigField :: < Tree >",
        "ConfigField :: < UserDefinedDeserialize >",
        "ConfigField :: < FlowAlias >",
        "take_array",
        "take_object",
        "join_config_path",
        "values . len () != 3",
        "try_into",
    ] {
        assert!(result.contains(expected), "missing {expected:?}: {result}");
    }
}

#[test]
fn container_syntax_requires_canonical_type_identity_before_generated_traversal() {
    let result = expansion(parse_quote! {
        struct Config { body: custom::Vec<Flow>, map: custom::BTreeMap<String, Flow> }
    });
    assert!(
        result
            .contains("ConfigField :: < custom :: Vec < Flow > , :: std :: vec :: Vec < Flow > >"),
        "{result}"
    );
    assert!(result.contains("ConfigField :: < custom :: BTreeMap < String , Flow > , :: std :: collections :: BTreeMap < :: std :: string :: String , Flow > >"), "{result}");
    assert!(result.contains("container_collect"));
    assert!(result.contains("container_decode"));
    assert!(result.contains("let mut result = :: std :: vec :: Vec :: new"));
    assert!(
        result.contains("__mf_paths : & mut :: std :: vec :: Vec < :: std :: string :: String >")
    );
}

#[test]
fn recursion_classification_is_cycle_guarded_but_traversal_is_value_driven() {
    let result =
        expansion(parse_quote! { struct Tree { next: Option<Box<Tree>>, body: Option<Flow> } });
    assert!(result.contains("__mf_visited . insert"));
    assert!(result.contains("__mf_visited . remove"));
    assert!(result.contains("__mf_config . is_some_and"));
    assert!(result.contains("field_is_leaf"));
}

#[test]
fn metadata_uses_presence_only_and_preserves_serde_alias_duplicate_checks() {
    let result = expansion(parse_quote! {
        #[serde(rename_all(deserialize = "camelCase", serialize = "snake_case"), deny_unknown_fields)]
        struct Config {
            #[serde(rename(deserialize = "request/body~", serialize = "ignored"), alias = "legacy")]
            request_body: Flow,
            #[serde(default = "Self::default_count", deserialize_with = "Self::decode_count")]
            count: usize,
        }
    });
    assert!(result.contains("Presence"));
    assert!(result.contains("deny_unknown_fields"));
    assert!(result.contains("rename = \"request/body~\" , alias = \"legacy\""));
    assert!(result.contains("contains_key (\"legacy\")"));
    assert!(result.contains("Config :: default_count"));
    assert!(result.contains("Config :: decode_count"));
    let collect = result
        .split("fn collect")
        .nth(1)
        .unwrap()
        .split("fn decode")
        .next()
        .unwrap();
    assert!(!collect.contains("default_count"));
    assert!(!collect.contains("decode_count"));
    assert!(!collect.contains("as :: core :: default :: Default"));
}

#[test]
fn raw_metadata_helpers_never_use_original_defaults_or_custom_codecs() {
    let result = expansion(parse_quote! {
        #[serde(default)]
        struct Config { count: usize, body: Option<Flow> }
    });
    let raw = result
        .split("impl :: mf_runtime :: NodeConfig")
        .next()
        .unwrap();
    assert!(raw.contains("Presence"));
    assert!(!raw.contains("Default for"));
    assert!(!raw.contains("< Config as"));
    assert!(result.contains("container default supplied executable Flow configuration"));
}

#[test]
fn flatten_traverses_the_shared_object_not_a_synthetic_field_pointer() {
    let result =
        expansion(parse_quote! { struct Config { #[serde(flatten)] nested: NestedConfig } });
    assert!(result.contains("serde (flatten)"));
    assert!(result.contains("let __mf_field_prefix = __mf_prefix . to_owned ()"));
    assert!(!result.contains("join_config_path"));
    assert_error(
        parse_quote! { struct Config { #[serde(flatten, default)] nested: NestedConfig } },
        "flatten with custom decoding/defaults",
    );
}

#[test]
fn derives_newtypes_tuple_structs_and_unit_structs() {
    for input in [
        parse_quote!(
            struct Newtype(FlowAlias);
        ),
        parse_quote!(
            struct Tuple(Flow, Vec<Ordinary>);
        ),
        parse_quote!(
            struct Unit;
        ),
    ] {
        let result = expansion(input);
        assert!(result.contains("fn collect"));
        assert!(result.contains("fn decode"));
    }
    let unit = expansion(parse_quote!(
        struct Unit;
    ));
    assert!(unit.contains("expected null for a unit configuration"));
    assert!(unit.contains("missing unit configuration"));
}

#[test]
fn externally_tagged_enums_select_only_the_present_variant() {
    let result = expansion(parse_quote! {
        #[serde(rename_all = "snake_case")]
        enum Config { UnitVariant, #[serde(alias = "legacy")] WithBody { body: Flow }, Tuple(Flow, usize), Newtype(Nested) }
    });
    assert!(result.contains("\"unit_variant\""));
    assert!(result.contains("\"with_body\" | \"legacy\""));
    assert!(result.contains("match __mf_variant"));
    assert!(result.contains("object . len () == 1"));
    assert!(result.contains("expected null for a unit configuration"));
}

#[test]
fn internally_tagged_enums_remove_the_tag_before_decoding_fields() {
    let result = expansion(parse_quote! {
        #[serde(tag = "kind", deny_unknown_fields, rename_all_fields = "camelCase")]
        enum Config { Unit, WithBody { request_body: Flow }, Wrapped(Nested) }
    });
    assert!(result.contains("__mf_payload . remove (\"kind\")"));
    assert!(result.contains("rename = \"requestBody\""));
    assert!(result.contains("unknown field in internally tagged unit variant"));
}

#[test]
fn adjacently_tagged_enums_check_outer_keys_and_descend_into_content() {
    let result = expansion(parse_quote! {
        #[serde(tag = "kind", content = "data", deny_unknown_fields)]
        enum Config { Unit, Body(Flow) }
    });
    assert!(result.contains("unknown field in adjacently tagged configuration enum"));
    assert!(result.contains("join_config_path (__mf_prefix , \"data\")"));
    assert!(!result.contains("__mf_missing"));
}

#[test]
fn ambiguous_wire_layouts_are_rejected_explicitly() {
    assert_error(
        parse_quote! { #[serde(untagged)] enum Config { Body { body: Flow } } },
        "unsupported serde container option",
    );
    for option in [
        quote!(transparent),
        quote!(from = "String"),
        quote!(try_from = "String"),
    ] {
        assert_error(
            parse_quote! { #[serde(#option)] struct Config { body: Flow } },
            "unsupported serde container option",
        );
    }
    assert_error(
        parse_quote! { #[serde(content = "data")] enum Config { Body(Flow) } },
        "must be used together",
    );
    assert_error(
        parse_quote! { union Config { count: usize } },
        "does not support derive for unions",
    );
    assert_error(
        parse_quote! { struct Config<T> { body: T } },
        "does not support generic",
    );
}

#[test]
fn duplicate_wire_names_and_aliases_are_rejected() {
    assert_error(
        parse_quote! { struct Config { #[serde(rename = "same")] body: Flow, #[serde(alias = "same")] other: usize } },
        "duplicate configuration field",
    );
    assert_error(
        parse_quote! { enum Config { #[serde(rename = "same")] First, #[serde(alias = "same")] Second } },
        "duplicate NodeConfig enum variant",
    );
    assert_error(
        parse_quote! { struct Config { #[serde(rename(deserialize = "a", deserialize = "b"))] body: Flow } },
        "duplicate serde attribute `rename`",
    );
    assert_error(
        parse_quote! { struct Config { #[serde(rename(foo = "a"))] body: Flow } },
        "malformed rename attribute",
    );
}

#[test]
fn runtime_aliases_are_used_by_all_generated_dispatchers() {
    let result = expansion(
        parse_quote! { #[config(runtime = "::runtime_alias")] struct Config { body: Flow } },
    );
    assert!(result.contains("impl :: runtime_alias :: NodeConfig"));
    assert!(result.contains("runtime_alias :: node_config :: Presence"));
    assert!(!result.contains(":: mf_runtime"));
    assert_error(
        parse_quote! { #[config(runtim = "crate")] struct Config {} },
        "expected one",
    );
    assert_error(
        parse_quote! { #[config(runtime = "")] struct Config {} },
        "expected",
    );
}

#[test]
fn malformed_serde_values_are_not_ignored() {
    for input in [
        parse_quote! { struct Config { #[serde(rename = 42)] body: Flow } },
        parse_quote! { #[serde(rename_all = "not-a-rule")] struct Config { body: Flow } },
    ] {
        assert!(expand(input).is_err());
    }
}

fn ordinary_config(attrs: proc_macro2::TokenStream) -> DeriveInput {
    parse_quote! { struct Config { #attrs ordinary: String, body: Flow } }
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
        "expected serde rename_all attribute to be a string",
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
