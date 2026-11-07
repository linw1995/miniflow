use mf_runtime::{
    Flow, NodeBuildError, NodeConfig, NodeFactory, NodeRegistration, PreparedSubgraph,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

fn prepared() -> PreparedSubgraph {
    PreparedSubgraph::from_flow(
        Flow::from_plan(
            vec![],
            mf_runtime::FlowPlan {
                connections: vec![].into(),
                dependencies: vec![].into(),
                execution_order: vec![].into(),
                outputs: vec![].into(),
                execution_domains: mf_runtime::ExecutionDomains::from_parts(
                    vec![].into(),
                    vec![].into(),
                ),
            },
            Default::default(),
        ),
        vec![],
    )
}

fn bindings(paths: &[&str]) -> BTreeMap<String, PreparedSubgraph> {
    paths
        .iter()
        .map(|path| ((*path).into(), prepared()))
        .collect()
}

fn assert_paths<T: NodeConfig>(value: &Value, expected: &[&str]) {
    let mut actual: Vec<_> = T::flow_parameters(value).unwrap().into_keys().collect();
    actual.sort();
    let mut expected = expected.to_vec();
    expected.sort();
    assert_eq!(actual, expected);
}

#[derive(NodeConfig)]
struct Branch {
    body: Flow,
    #[serde(default)]
    label: String,
}

#[test]
fn typed_containers_compose_at_actual_value_locations() {
    #[derive(NodeConfig)]
    struct Config {
        branch: Branch,
        maybe: Option<Branch>,
        absent: Option<Branch>,
        branches: Vec<Branch>,
        boxed: Box<Branch>,
        array: [Flow; 2],
        tuple: (usize, Flow, Option<Flow>),
        handlers: BTreeMap<String, Flow>,
        hashed: HashMap<String, Branch>,
        ordinary: Value,
    }
    let value = json!({
        "branch": {"body": {}, "label": "selected"},
        "maybe": {"body": {}}, "absent": null,
        "branches": [{"body": {}}, {"body": {}}],
        "boxed": {"body": {}}, "array": [{}, {}], "tuple": [9, {}, null],
        "handlers": {"a/b~": {}, "": {}},
        "hashed": {"x/y~": {"body": {}}},
        "ordinary": {"body": {"version": "2026-10-28", "dependencies": {}, "nodes": []}}
    });
    let paths = [
        "/branch/body",
        "/maybe/body",
        "/branches/0/body",
        "/branches/1/body",
        "/boxed/body",
        "/array/0",
        "/array/1",
        "/tuple/1",
        "/tuple/2",
        "/handlers/a~1b~0",
        "/handlers/",
        "/hashed/x~1y~0/body",
    ];
    assert_paths::<Config>(&value, &paths);
    // Optional Flow leaves are declared even when null, but absent optional
    // parents must not manufacture children or require prepared handles.
    let bound: Vec<_> = paths
        .iter()
        .copied()
        .filter(|path| *path != "/tuple/2")
        .collect();
    let config = Config::from_config(value.clone(), bindings(&bound)).unwrap();
    assert_eq!(config.branch.label, "selected");
    let _: Flow = config.branch.body;
    let _: Flow = config.maybe.unwrap().body;
    assert!(config.absent.is_none());
    assert_eq!(config.branches.len(), 2);
    let _: Flow = config.boxed.body;
    assert_eq!(config.array.len(), 2);
    assert_eq!(config.tuple.0, 9);
    assert!(config.tuple.2.is_none());
    assert_eq!(config.handlers.len(), 2);
    assert_eq!(config.hashed.len(), 1);
    assert_eq!(config.ordinary, value["ordinary"]);

    assert!(Config::from_config(value.clone(), BTreeMap::new()).is_err());
    let mut wrong = bindings(&bound);
    wrong.insert("/absent/body".into(), prepared());
    assert!(Config::from_config(value.clone(), wrong).is_err());
    let mut wrong = bindings(&bound);
    wrong.insert("/handlers/a/b~".into(), prepared());
    assert!(Config::from_config(value, wrong).is_err());
}

#[test]
fn prepared_handles_must_match_nonnull_flow_sources_without_creating_parents() {
    #[derive(NodeConfig)]
    struct Config {
        body: Option<Flow>,
        parent: Option<Branch>,
    }
    for value in [json!({}), json!({"body": null, "parent": null})] {
        assert_paths::<Config>(&value, &["/body"]);
        assert!(Config::from_config(value.clone(), bindings(&["/body"])).is_err());
        let config = Config::from_config(value.clone(), BTreeMap::new()).unwrap();
        assert!(config.body.is_none());
        assert!(config.parent.is_none());
        assert!(Config::from_config(value, bindings(&["/parent/body"])).is_err());
    }
    assert!(Branch::from_config(json!({"body": null}), bindings(&["/body"])).is_err());
    assert!(Config::from_config(json!({"body": {}}), BTreeMap::new()).is_err());
    let config = Config::from_config(json!({"body": {}}), bindings(&["/body"])).unwrap();
    assert!(config.body.is_some());
    assert!(
        Config::from_config(
            json!({"parent": {"body": null}}),
            bindings(&["/parent/body"]),
        )
        .is_err()
    );
    assert!(Config::from_config(json!({"parent": {"body": {}}}), BTreeMap::new(),).is_err());
}

#[test]
fn finite_recursive_configs_walk_only_present_values() {
    #[derive(NodeConfig)]
    struct Tree {
        body: Option<Flow>,
        child: Box<Option<Tree>>,
    }
    let value = json!({"body": {}, "child": {"child": {"body": {}, "child": null}}});
    assert_paths::<Tree>(&value, &["/body", "/child/body", "/child/child/body"]);
    let tree = Tree::from_config(value, bindings(&["/body", "/child/child/body"])).unwrap();
    assert!(tree.body.is_some());
    let child = tree.child.unwrap();
    assert!(child.body.is_none());
    let last = child.child.unwrap();
    assert!(last.body.is_some());
    assert!(last.child.is_none());
}

#[test]
fn tagged_enums_discover_and_decode_only_the_active_variant() {
    #[derive(NodeConfig)]
    enum External {
        Run { body: Flow },
        Wrapped(Branch),
        Data { body: Value },
    }
    #[derive(NodeConfig)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Internal {
        Run { body: Flow },
        Data { body: Value },
    }
    #[derive(NodeConfig)]
    #[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
    enum Adjacent {
        Run { body: Flow },
        Data { body: Value },
    }
    let shape = json!({"version": "2026-10-28", "dependencies": {}, "nodes": []});
    let run = json!({"Run": {"body": shape}});
    assert_paths::<External>(&run, &["/Run/body"]);
    let External::Run { body } = External::from_config(run, bindings(&["/Run/body"])).unwrap()
    else {
        panic!("wrong variant")
    };
    let _: Flow = body;
    let wrapped = json!({"Wrapped": {"body": shape}});
    assert_paths::<External>(&wrapped, &["/Wrapped/body"]);
    let External::Wrapped(branch) =
        External::from_config(wrapped, bindings(&["/Wrapped/body"])).unwrap()
    else {
        panic!("wrong variant")
    };
    let _: Flow = branch.body;
    let data = json!({"Data": {"body": shape}});
    assert_paths::<External>(&data, &[]);
    let External::Data { body } = External::from_config(data, BTreeMap::new()).unwrap() else {
        panic!("wrong variant")
    };
    assert_eq!(body, shape);

    let run = json!({"kind": "run", "body": shape});
    assert_paths::<Internal>(&run, &["/body"]);
    let Internal::Run { body } = Internal::from_config(run, bindings(&["/body"])).unwrap() else {
        panic!("wrong variant")
    };
    let _: Flow = body;
    let data = json!({"kind": "data", "body": shape});
    assert_paths::<Internal>(&data, &[]);
    let Internal::Data { body } = Internal::from_config(data, BTreeMap::new()).unwrap() else {
        panic!("wrong variant")
    };
    assert_eq!(body, shape);

    let run = json!({"kind": "run", "payload": {"body": shape}});
    assert_paths::<Adjacent>(&run, &["/payload/body"]);
    let Adjacent::Run { body } = Adjacent::from_config(run, bindings(&["/payload/body"])).unwrap()
    else {
        panic!("wrong variant")
    };
    let _: Flow = body;
    let data = json!({"kind": "data", "payload": {"body": shape}});
    assert_paths::<Adjacent>(&data, &[]);
    assert!(Adjacent::from_config(data.clone(), bindings(&["/payload/body"])).is_err());
    let Adjacent::Data { body } = Adjacent::from_config(data, BTreeMap::new()).unwrap() else {
        panic!("wrong variant")
    };
    assert_eq!(body, shape);
}

#[test]
fn tagged_metadata_defers_unknown_ordinary_keys_without_authorizing_them() {
    #[derive(NodeConfig)]
    #[serde(tag = "kind", deny_unknown_fields)]
    #[allow(dead_code)]
    enum Internal {
        Run { body: Flow },
    }
    #[derive(NodeConfig)]
    #[serde(tag = "kind", content = "data", deny_unknown_fields)]
    #[allow(dead_code)]
    enum Adjacent {
        Run { body: Flow },
    }
    let unknown = json!({"version": "2026-10-28", "nodes": []});
    let internal = json!({"kind": "Run", "body": {}, "unexpected": unknown});
    assert_eq!(
        Internal::flow_parameters(&internal).unwrap(),
        BTreeMap::from([("/body".into(), json!({}))])
    );
    assert!(
        matches!(Internal::from_config(internal, bindings(&["/body"])),
        Err(NodeBuildError::InvalidConfiguration { source }) if source.to_string().contains("unknown field `unexpected`"))
    );
    for adjacent in [
        json!({"kind": "Run", "data": {"body": {}}, "unexpected": unknown}),
        json!({"kind": "Run", "data": {"body": {}, "unexpected": unknown}}),
    ] {
        assert_eq!(
            Adjacent::flow_parameters(&adjacent).unwrap(),
            BTreeMap::from([("/data/body".into(), json!({}))])
        );
        assert!(Adjacent::from_config(adjacent, bindings(&["/data/body"])).is_err());
    }
    assert!(Internal::flow_parameters(&json!({"kind": "Unknown", "body": {}})).is_err());
    assert!(Adjacent::flow_parameters(&json!({"kind": false, "data": {"body": {}}})).is_err());
}

#[test]
fn newtypes_keep_their_wire_location_including_the_root_pointer() {
    #[derive(NodeConfig)]
    struct Wrapped(Branch);
    #[derive(NodeConfig)]
    struct Root(Flow);
    let value = json!({"body": {}});
    assert_paths::<Wrapped>(&value, &["/body"]);
    let _: Flow = Wrapped::from_config(value, bindings(&["/body"]))
        .unwrap()
        .0
        .body;
    assert_paths::<Root>(&json!({}), &[""]);
    let _: Flow = Root::from_config(json!({}), bindings(&[""])).unwrap().0;
}

#[test]
fn optional_transparent_flow_newtypes_authorize_their_own_missing_location() {
    #[derive(NodeConfig)]
    struct Wrapped(Flow);
    #[derive(NodeConfig)]
    struct WrappedOptional(Option<Flow>);
    #[derive(NodeConfig)]
    struct Config {
        body: Option<Wrapped>,
        optional: Option<WrappedOptional>,
    }
    for value in [json!({}), json!({"body": null, "optional": null})] {
        assert_eq!(
            Config::flow_parameters(&value).unwrap(),
            BTreeMap::from([
                ("/body".into(), Value::Null),
                ("/optional".into(), Value::Null),
            ])
        );
        let config = Config::from_config(value, BTreeMap::new()).unwrap();
        assert!(config.body.is_none());
        assert!(config.optional.is_none());
    }
    let config = Config::from_config(
        json!({"body": {}, "optional": {}}),
        bindings(&["/body", "/optional"]),
    )
    .unwrap();
    let _: Flow = config.body.unwrap().0;
    let _: Flow = config.optional.unwrap().0.unwrap();
    assert_paths::<WrappedOptional>(&Value::Null, &[""]);
    assert!(
        WrappedOptional::from_config(Value::Null, BTreeMap::new())
            .unwrap()
            .0
            .is_none()
    );

    #[derive(NodeConfig)]
    struct Recursive(Box<Option<Recursive>>);
    assert!(!Recursive::is_flow_leaf());
    assert!(Recursive::flow_parameters(&Value::Null).unwrap().is_empty());
    assert!(
        Recursive::from_config(Value::Null, BTreeMap::new())
            .unwrap()
            .0
            .is_none()
    );
}

#[test]
fn discovery_never_runs_ordinary_codecs_defaults_or_factories() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DEFAULTS: AtomicUsize = AtomicUsize::new(0);
    static CODECS: AtomicUsize = AtomicUsize::new(0);
    static OPAQUE: AtomicUsize = AtomicUsize::new(0);
    struct Ordinary(Value);
    impl<'de> serde::Deserialize<'de> for Ordinary {
        fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            OPAQUE.fetch_add(1, Ordering::SeqCst);
            <Value as serde::Deserialize>::deserialize(decoder).map(Self)
        }
    }
    #[derive(NodeConfig)]
    #[serde(deny_unknown_fields)]
    struct Config {
        branch: Branch,
        opaque: Vec<Ordinary>,
        #[serde(
            default = "Self::default_count",
            deserialize_with = "Self::decode_count"
        )]
        count: usize,
    }
    impl Config {
        fn default_count() -> usize {
            DEFAULTS.fetch_add(1, Ordering::SeqCst);
            7
        }
        fn decode_count<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<usize, D::Error> {
            CODECS.fetch_add(1, Ordering::SeqCst);
            <usize as serde::Deserialize>::deserialize(decoder).map(|value| value + 1)
        }
    }
    let registration = NodeRegistration {
        kind: "test.nested",
        factory: NodeFactory::Subgraph {
            flow_parameters: Config::flow_parameters,
            plain: |_| panic!("metadata must not invoke the factory"),
            prepared: |_, _, _| panic!("metadata must not invoke the factory"),
        },
    };
    let value = json!({"branch": {"body": {}}, "opaque": [{"body": {"nodes": []}}]});
    assert_paths::<Config>(&value, &["/branch/body"]);
    assert_eq!(
        registration.flow_parameters(&value).unwrap(),
        BTreeMap::from([("/branch/body".into(), json!({}))])
    );
    assert_eq!(DEFAULTS.load(Ordering::SeqCst), 0);
    assert_eq!(CODECS.load(Ordering::SeqCst), 0);
    assert_eq!(OPAQUE.load(Ordering::SeqCst), 0);
    let config = Config::from_config(value.clone(), bindings(&["/branch/body"])).unwrap();
    assert_eq!(config.count, 7);
    assert_eq!(config.opaque[0].0, value["opaque"][0]);
    let _: Flow = config.branch.body;
    assert_eq!(DEFAULTS.load(Ordering::SeqCst), 1);
    assert_eq!(CODECS.load(Ordering::SeqCst), 0);
    assert_eq!(OPAQUE.load(Ordering::SeqCst), 1);
    let mut explicit = value.clone();
    explicit["count"] = json!(4);
    let config = Config::from_config(explicit, bindings(&["/branch/body"])).unwrap();
    assert_eq!(config.count, 5);
    assert_eq!(DEFAULTS.load(Ordering::SeqCst), 1);
    assert_eq!(CODECS.load(Ordering::SeqCst), 1);
    let mut unknown = value;
    unknown["unexpected"] = json!(true);
    assert!(Config::from_config(unknown, bindings(&["/branch/body"])).is_err());
}

#[test]
fn metadata_ignores_poisoned_flow_free_shapes_until_provider_decoding() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    #[allow(dead_code)]
    struct Ordinary(usize);
    impl<'de> serde::Deserialize<'de> for Ordinary {
        fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
            CALLS.fetch_add(1, Ordering::SeqCst);
            <usize as serde::Deserialize>::deserialize(decoder).map(Self)
        }
    }
    fn default_count() -> usize {
        CALLS.fetch_add(1, Ordering::SeqCst);
        7
    }
    fn decode_count<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<usize, D::Error> {
        CALLS.fetch_add(1, Ordering::SeqCst);
        <usize as serde::Deserialize>::deserialize(decoder)
    }
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Config {
        body: Option<Flow>,
        #[serde(default = "default_count", deserialize_with = "decode_count")]
        count: usize,
        variables: Vec<Ordinary>,
        policies: BTreeMap<String, Ordinary>,
        fixed: [Ordinary; 1],
        mixed: (Option<Flow>, Vec<Ordinary>, [Ordinary; 1]),
        opaque: Value,
        custom: Ordinary,
    }
    let poison = json!({"nodes": []});
    let valid = json!({
        "variables": [1], "policies": {"a": 1}, "fixed": [1],
        "mixed": [null, [1], [1]], "opaque": poison, "custom": 1,
    });
    for (path, value) in [
        ("/variables", poison.clone()),
        ("/policies", json!([poison])),
        ("/fixed", poison.clone()),
        ("/mixed/1", poison.clone()),
        ("/mixed/2", poison.clone()),
        ("/custom", poison.clone()),
        ("/count", poison.clone()),
    ] {
        let mut input = valid.clone();
        if path == "/count" {
            input["count"] = value;
        } else {
            *input.pointer_mut(path).unwrap() = value;
        }
        let before = CALLS.load(Ordering::SeqCst);
        assert_eq!(
            Config::flow_parameters(&input).unwrap(),
            BTreeMap::from([
                ("/body".into(), Value::Null),
                ("/mixed/0".into(), Value::Null),
            ]),
            "{path}"
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), before, "{path}");
        assert!(
            Config::from_config(input, BTreeMap::new()).is_err(),
            "{path}"
        );
    }

    // Root aliases and runtime-owned tuple implementations must enforce the same
    // opacity as concrete generated fields, without validating ordinary shapes.
    type Mixed = (Option<Flow>, Vec<usize>, [usize; 1]);
    let input = json!([null, poison, poison]);
    assert_eq!(
        Mixed::flow_parameters(&input).unwrap(),
        BTreeMap::from([("/0".into(), Value::Null)])
    );
    assert!(Mixed::from_config(input, BTreeMap::new()).is_err());
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Plain {
        values: Vec<Ordinary>,
        opaque: Value,
    }
    for input in [Value::Null, poison, json!(42), json!([])] {
        let before = CALLS.load(Ordering::SeqCst);
        assert!(Plain::flow_parameters(&input).unwrap().is_empty());
        assert!(Vec::<usize>::flow_parameters(&input).unwrap().is_empty());
        assert!(
            BTreeMap::<String, usize>::flow_parameters(&input)
                .unwrap()
                .is_empty()
        );
        assert!(<[usize; 1]>::flow_parameters(&input).unwrap().is_empty());
        assert_eq!(CALLS.load(Ordering::SeqCst), before);
        assert!(Plain::from_config(input, BTreeMap::new()).is_err());
    }
}

#[test]
fn executable_codecs_defaults_and_skips_are_rejected_without_running_user_code() {
    fn default_body() -> Option<Flow> {
        panic!("metadata must not invoke executable defaults")
    }
    fn decode_body<'de, D: serde::Deserializer<'de>>(_: D) -> Result<Option<Flow>, D::Error> {
        panic!("metadata must not invoke executable codecs")
    }
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Codec {
        #[serde(deserialize_with = "decode_body")]
        body: Option<Flow>,
    }
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Defaulted {
        #[serde(default = "default_body", deserialize_with = "decode_body")]
        body: Option<Flow>,
    }
    #[derive(NodeConfig)]
    #[allow(dead_code)]
    struct Skipped {
        #[serde(skip_deserializing)]
        body: Option<Flow>,
    }
    for value in [
        json!({}),
        json!({"body": null}),
        json!({"body": {"path": "never-open.json"}}),
    ] {
        assert!(Codec::flow_parameters(&value).is_err());
        assert!(Defaulted::flow_parameters(&value).is_err());
        assert!(Skipped::flow_parameters(&value).is_err());
        assert!(Codec::from_config(value.clone(), BTreeMap::new()).is_err());
        assert!(Defaulted::from_config(value.clone(), BTreeMap::new()).is_err());
        assert!(Skipped::from_config(value, BTreeMap::new()).is_err());
    }
}

#[test]
fn fixed_arrays_validate_length_before_authorizing_any_flow_leaves() {
    type FlowPair = [Flow; 2];
    #[derive(NodeConfig)]
    struct Direct {
        bodies: [Flow; 2],
    }
    #[derive(NodeConfig)]
    struct Aliased {
        bodies: FlowPair,
    }
    for value in [
        json!({"bodies": [null]}),
        json!({"bodies": [null, null, null]}),
    ] {
        assert!(Direct::flow_parameters(&value).is_err());
        assert!(Aliased::flow_parameters(&value).is_err());
    }
    let value = json!({"bodies": [{}, {"not": "a Flow source"}]});
    assert_paths::<Direct>(&value, &["/bodies/0", "/bodies/1"]);
    assert_paths::<Aliased>(&value, &["/bodies/0", "/bodies/1"]);
    assert_eq!(
        Direct::from_config(value.clone(), bindings(&["/bodies/0", "/bodies/1"]))
            .unwrap()
            .bodies
            .len(),
        2
    );
    assert_eq!(
        Aliased::from_config(value, bindings(&["/bodies/0", "/bodies/1"]))
            .unwrap()
            .bodies
            .len(),
        2
    );
}

#[test]
fn unit_configs_preserve_serde_wire_validation() {
    #[derive(NodeConfig)]
    struct Unit;
    #[derive(NodeConfig)]
    enum External {
        Unit,
    }
    #[derive(NodeConfig)]
    #[serde(tag = "kind")]
    enum Internal {
        Unit,
    }
    assert!(matches!(
        Unit::from_config(Value::Null, BTreeMap::new()).unwrap(),
        Unit
    ));
    for value in [json!({}), json!(42), json!("Unit")] {
        assert!(Unit::from_config(value, BTreeMap::new()).is_err());
    }
    for value in [json!("Unit"), json!({"Unit": null})] {
        assert!(matches!(
            External::from_config(value, BTreeMap::new()).unwrap(),
            External::Unit
        ));
    }
    for value in [
        json!({"Unit": {}}),
        json!({"Unit": 42}),
        json!({"Unit": []}),
    ] {
        assert!(External::from_config(value, BTreeMap::new()).is_err());
    }
    assert!(matches!(
        Internal::from_config(json!({"kind": "Unit"}), BTreeMap::new()).unwrap(),
        Internal::Unit
    ));
}

#[test]
fn root_flow_roles_support_compiled_flows_and_callback_handles() {
    let value = json!({"nodes": []});
    assert_eq!(
        Flow::flow_parameters(&value).unwrap(),
        BTreeMap::from([("".into(), value.clone())])
    );
    assert_eq!(
        PreparedSubgraph::flow_parameters(&Value::Null).unwrap(),
        BTreeMap::from([("".into(), Value::Null)])
    );
    let flow = Flow::from_config(value.clone(), bindings(&[""])).unwrap();
    assert!(flow.execute().unwrap().is_empty());

    let callback = PreparedSubgraph::from_config(
        value.clone(),
        BTreeMap::from([(
            "".into(),
            PreparedSubgraph::new(vec![], vec![], |_| {
                panic!("preparation must not execute callbacks")
            }),
        )]),
    )
    .unwrap();
    assert!(callback.nodes.is_empty());
    assert!(
        PreparedSubgraph::from_config(Value::Null, BTreeMap::from([("".into(), prepared())]))
            .is_err()
    );
    assert!(Flow::from_config(value.clone(), BTreeMap::new()).is_err());

    let registration = NodeRegistration {
        kind: "test.root",
        factory: NodeFactory::Subgraph {
            flow_parameters: Flow::flow_parameters,
            plain: |_| panic!("metadata must not invoke factories"),
            prepared: |_, _, _| panic!("metadata must not invoke factories"),
        },
    };
    let node: mf_runtime::NodeDefinition = serde_json::from_value(json!({
        "id": "root", "kind": "test.root", "config": value,
    }))
    .unwrap();
    let bodies = registration
        .validate_flow_parameters(&node, mf_runtime::WorkflowDefinitionVersion::V2026_10_25)
        .unwrap();
    assert_eq!(
        bodies[""].version,
        mf_runtime::WorkflowDefinitionVersion::V2026_10_25
    );
}

#[test]
fn callback_handles_are_not_executed_during_nested_preparation() {
    #[derive(NodeConfig)]
    struct Config {
        branches: Vec<Callback>,
    }
    #[derive(NodeConfig)]
    struct Callback {
        body: PreparedSubgraph,
    }
    let config = Config::from_config(
        json!({"branches": [{"body": {}}]}),
        BTreeMap::from([(
            "/branches/0/body".into(),
            PreparedSubgraph::new(vec![], vec![], |_| panic!("must not execute")),
        )]),
    )
    .unwrap();
    let _: &PreparedSubgraph = &config.branches[0].body;
    let error = Branch::from_config(
        json!({"body": {}}),
        BTreeMap::from([(
            "/body".into(),
            PreparedSubgraph::new(vec![], vec![], |_| panic!("must not execute")),
        )]),
    )
    .err()
    .unwrap();
    assert!(matches!(error, NodeBuildError::InvalidSubgraph { .. }));
}
