use mf_runtime::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    error::Error,
    sync::atomic::{AtomicUsize, Ordering},
};

static DEFAULTS: AtomicUsize = AtomicUsize::new(0);
fn default_limit() -> i64 {
    DEFAULTS.fetch_add(1, Ordering::SeqCst);
    7
}

#[derive(Debug, NodeEnum)]
enum Mode {
    #[value(rename = "fast")]
    Fast,
    Thorough,
}
#[derive(Debug, NodeEnum)]
#[value(tag = "kind")]
enum Filter {
    All,
    Match {
        #[value(rename = "field.path", description = "The exact field to match")]
        field: String,
        expected: Nullable<String>,
        #[value(default)]
        optional: bool,
    },
}
#[derive(Debug, NodeValue)]
struct Request {
    #[value(description = "Search text")]
    query: String,
    mode: Mode,
    filters: Vec<Filter>,
    extra: Option<BTreeMap<String, Nullable<String>>>,
    #[value(default = "default_limit")]
    limit: i64,
}
#[derive(NodeInputs)]
struct InputsWithSchema {
    #[input(
        rename = "request.payload",
        description = "The structured search request"
    )]
    payload: Shared<Request>,
    optional: Option<Nullable<Mode>>,
    plain: String,
}

#[test]
fn reflection_retains_fields_closed_variants_descriptions_and_presence_without_defaults() {
    DEFAULTS.store(0, Ordering::SeqCst);
    let schemas = <InputsWithSchema as NodeInputs>::input_schemas();
    assert_eq!(DEFAULTS.load(Ordering::SeqCst), 0);
    let schema = &schemas["request.payload"];
    assert!(schema.is_complete());
    let ValueSchema::Described {
        schema,
        description,
    } = schema
    else {
        panic!("missing public description")
    };
    assert_eq!(description, "The structured search request");
    let ValueSchema::Object(fields) = schema.as_ref() else {
        panic!("missing object fields")
    };
    assert!(fields["query"].required);
    assert!(!fields["extra"].required && !fields["limit"].required);
    assert!(
        matches!(&fields["mode"].schema, ValueSchema::Enum(values) if values == &["fast", "Thorough"])
    );
    let serialized = serde_json::to_value(&schemas).unwrap();
    assert!(!serialized.to_string().contains("default_limit"));
    let roundtrip: BTreeMap<String, ValueSchema> = serde_json::from_value(serialized).unwrap();
    assert_eq!(schemas, roundtrip);
    assert!(
        schemas["optional"]
            .validate_shared(&json!(null).into())
            .is_ok()
    );
    assert!(schemas["optional"].validate_shared(&"fast".into()).is_ok());
    assert!(
        schemas["optional"]
            .validate_shared(&"unknown".into())
            .is_err()
    );
    let decoded = InputsWithSchema::from_inputs(Inputs::from([
        (
            "request.payload".into(),
            json!({"query":"q","mode":"fast","filters":[]}).into(),
        ),
        ("plain".into(), "text".into()),
    ]))
    .unwrap();
    assert!(decoded.optional.is_none());
    assert_eq!(decoded.plain, "text");
    assert_eq!(DEFAULTS.load(Ordering::SeqCst), 0);
    assert_eq!(decoded.payload.decode().unwrap().limit, 7);
    assert_eq!(DEFAULTS.load(Ordering::SeqCst), 1);
}

#[test]
fn decoded_schema_validation_agrees_with_strict_codecs_for_structs_enums_and_collections() {
    let mut context = ValueSchemaContext::default();
    let schema = context.describe::<Request>();
    let valid = json!({"query":"q","mode":"fast","filters":[{"kind":"All"},{"kind":"Match","field.path":"name","expected":null}],"extra":{"x":null},"limit":7});
    let mut cases = vec![
        valid.clone(),
        json!(null),
        json!({}),
        json!({"unexpected":1}),
    ];
    for (pointer, value) in [
        ("/query", json!(null)),
        ("/mode", json!("unknown")),
        ("/filters/0/kind", json!(false)),
        ("/filters/1/expected", json!(5)),
        ("/extra/x", json!(false)),
        ("/limit", json!(1.5)),
    ] {
        let mut bad = valid.clone();
        *bad.pointer_mut(pointer)
            .unwrap_or_else(|| panic!("missing {pointer}")) = value;
        cases.push(bad);
    }
    let mut extra = valid;
    extra["filters"][0]["extra"] = json!(true);
    cases.push(extra);
    for value in cases {
        let value = ValueRef::from(value);
        assert_eq!(
            schema.validate_shared(&value).is_ok(),
            <Request as InputValue>::decode(value).is_ok()
        );
    }
    let value = ValueRef::from(
        json!({"query":"q","mode":"fast","filters":[{"kind":"Match","field.path":"name","expected":3}]}),
    );
    let error = schema.validate_shared(&value).unwrap_err();
    assert_eq!(error.path, "/filters/0/expected");
    assert!(error.source().is_some());
    assert!(
        context
            .describe::<BTreeMap<String, Mode>>()
            .validate_shared(&json!({"a/b~c":"bad"}).into())
            .unwrap_err()
            .path
            .starts_with("/a~1b~0c")
    );
}

struct Opaque;
impl InputValue for Opaque {
    fn value_type() -> ValueType {
        ValueType::String
    }
    fn decode(_: ValueRef) -> Result<Self, TypeMismatch> {
        unreachable!("reflection never decodes")
    }
}
struct Recursive;
impl InputValue for Recursive {
    fn value_type() -> ValueType {
        ValueType::Array
    }
    fn value_schema(context: &mut ValueSchemaContext) -> ValueSchema {
        context.describe::<Self>().list()
    }
    fn decode(_: ValueRef) -> Result<Self, TypeMismatch> {
        unreachable!("reflection never decodes")
    }
}
struct Panicking;
impl InputValue for Panicking {
    fn value_type() -> ValueType {
        ValueType::Object
    }
    fn value_schema(_: &mut ValueSchemaContext) -> ValueSchema {
        panic!("schema provider panic")
    }
    fn decode(_: ValueRef) -> Result<Self, TypeMismatch> {
        unreachable!()
    }
}
#[test]
fn opaque_recursive_and_unwinding_reflection_never_widen_a_known_contract() {
    let mut context = ValueSchemaContext::default();
    let opaque = context.describe::<Opaque>();
    assert!(matches!(
        opaque,
        ValueSchema::Opaque {
            reason: SchemaOpacity::Codec,
            ..
        }
    ));
    assert!(!opaque.is_complete());
    assert!(opaque.validate_shared(&true.into()).is_err());
    let recursive = context.describe::<Recursive>();
    assert!(!recursive.is_complete());
    assert!(
        serde_json::to_string(&recursive)
            .unwrap()
            .contains("recursive")
    );
    for _ in 0..2 {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || context.describe::<Panicking>()
            ))
            .is_err()
        );
        assert!(context.describe::<String>().is_complete());
    }
    let mut last = context.describe::<String>();
    for _ in 0..5000 {
        last = context.describe::<String>();
    }
    assert!(matches!(
        last,
        ValueSchema::Opaque {
            reason: SchemaOpacity::Limit,
            ..
        }
    ));
}

struct Task;
impl TypedTaskNode for Task {
    type Input = InputsWithSchema;
    type Output = Empty;
    fn execute(
        &self,
        _: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Self::Output>, NodeExecutionError> {
        panic!("preparation must not execute")
    }
}
#[derive(NodeOutputs)]
struct Empty {}
#[test]
fn prepared_metadata_reflects_only_refinements_and_rejects_schema_drift_or_overrides() {
    let prepared = PreparedNode::typed_task(Task, NodeMetadata::default()).unwrap();
    assert!(
        prepared
            .metadata
            .input_schemas
            .contains_key("request.payload")
    );
    assert!(prepared.metadata.input_schemas.contains_key("optional"));
    assert!(!prepared.metadata.input_schemas.contains_key("plain"));
    let expected = serde_json::to_value(&prepared.metadata).unwrap();
    let mut changed = prepared.metadata.clone();
    changed.input_schemas.remove("optional");
    assert!(verify_generated_metadata(&changed, &expected).is_err());
    assert!(
        PreparedNode::typed_task(
            Task,
            NodeMetadata {
                input_schemas: prepared.metadata.input_schemas,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn malformed_schema_metadata_fails_before_serialization() {
    let metadata = |schema| NodeMetadata {
        ports: NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::String, true)],
            outputs: vec![],
        },
        input_schemas: BTreeMap::from([("input".into(), schema)]),
        ..Default::default()
    };
    assert!(
        metadata(ValueSchema::Type(ValueType::Boolean))
            .validate_input_schemas()
            .is_err()
    );
    assert!(
        metadata(ValueSchema::Enum(vec!["a".into(), "a".into()]))
            .validate_input_schemas()
            .is_err()
    );
    assert!(
        NodeMetadata {
            input_schemas: BTreeMap::from([(
                "missing".into(),
                ValueSchema::Type(ValueType::String)
            )]),
            ..Default::default()
        }
        .validate_input_schemas()
        .is_err()
    );
    let mut deep = ValueSchema::Type(ValueType::String);
    for _ in 0..40 {
        deep = deep.described("nested");
    }
    assert!(metadata(deep).validate_input_schemas().is_err());
    assert!(
        metadata(ValueSchema::Type(ValueType::String).described("x".repeat(70_000)))
            .validate_input_schemas()
            .is_err()
    );
}

struct Layer<T>(std::marker::PhantomData<T>);
impl<T: InputValue> InputValue for Layer<T> {
    fn value_type() -> ValueType {
        ValueType::Object
    }
    fn value_schema(context: &mut ValueSchemaContext) -> ValueSchema {
        ValueSchema::Object(BTreeMap::from([(
            "nested".into(),
            SchemaField {
                schema: context.describe::<T>(),
                required: true,
            },
        )]))
    }
    fn decode(_: ValueRef) -> Result<Self, TypeMismatch> {
        unreachable!("reflection does not decode")
    }
}
#[test]
fn bounded_reflection_and_partial_manual_contracts_remain_explicit() {
    type L2<T> = Layer<Layer<T>>;
    type L4<T> = L2<L2<T>>;
    type L8<T> = L4<L4<T>>;
    type Deep = L8<L8<Layer<String>>>;
    let mut context = ValueSchemaContext::default();
    let deep = context.describe::<Deep>();
    assert!(!deep.is_complete());
    assert!(serde_json::to_string(&deep).unwrap().contains("limit"));
    let incomplete = ValueSchema::object(
        vec![PortSpec::new("x", ValueType::String, true)],
        BTreeMap::new(),
    );
    assert!(!incomplete.is_complete());
    incomplete
        .validate_shared(&json!({"x":"valid coarse type"}).into())
        .unwrap();
    assert!(incomplete.validate_shared(&json!({"x":1}).into()).is_err());
    let plain = ValueSchema::Type(ValueType::String);
    assert_eq!(plain.clone().described(""), plain);
    context
        .describe::<BTreeMap<String, Mode>>()
        .validate_shared(&json!({"mode":"fast"}).into())
        .unwrap();
    assert_eq!(
        context.describe::<Vec<String>>(),
        ValueSchema::Type(ValueType::List(Box::new(ValueType::String)))
    );
}

#[test]
fn schema_metadata_rejects_conflicting_tags_and_excessive_fields() {
    let object = |schema| NodeMetadata {
        ports: NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::Object, true)],
            outputs: vec![],
        },
        input_schemas: BTreeMap::from([("input".into(), schema)]),
        ..Default::default()
    };
    let tagged = ValueSchema::Tagged {
        tag: "kind".into(),
        variants: BTreeMap::from([(
            "one".into(),
            BTreeMap::from([(
                "kind".into(),
                SchemaField {
                    schema: ValueSchema::Type(ValueType::String),
                    required: true,
                },
            )]),
        )]),
    };
    assert!(matches!(
        object(tagged).validate_input_schemas(),
        Err(SchemaContractError::EnumConflict)
    ));
    let fields = (0..ValueSchema::MAX_NODES)
        .map(|index| {
            (
                index.to_string(),
                SchemaField {
                    schema: ValueSchema::Type(ValueType::String),
                    required: false,
                },
            )
        })
        .collect();
    assert!(matches!(
        object(ValueSchema::Object(fields)).validate_input_schemas(),
        Err(SchemaContractError::Limit { .. })
    ));
}
