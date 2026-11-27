use mf_runtime::{FlowOutputs, PortSpec, StreamCancellation, ValueRef, ValueType, callback::*};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
fn private_paths_round_trip_without_tainting_sibling_messages_or_treating_fields_as_commands() {
    let private = ValueRef::from("instruction").into_secret();
    let payload = ValueRef::object([(
        "a/b".into(),
        ValueRef::array([ValueRef::from("public"), private.clone()]),
    )]);
    let values: FlowOutputs = [
        ("type".into(), "call".into()),
        ("args".into(), payload),
        ("z".into(), private),
    ]
    .into();
    let bytes = serde_json::to_vec(&WireValues::encode(&values)).unwrap();
    let value: WireValues = mf_runtime::decode_unique_json(&bytes).unwrap();
    let decoded = value.decode().unwrap();
    assert_eq!(decoded, values);
    assert_eq!(
        decoded["args"].redacted(),
        json!({"a/b":["public","[REDACTED]"]})
    );
    assert!(decoded["z"].is_secret());
    assert!(
        serde_json::from_value::<WireValues>(json!({"values":{},"private":[["missing"]]}))
            .unwrap()
            .decode()
            .is_err()
    );
}

#[test]
fn a_bound_callback_starts_as_an_owned_invocation_and_keeps_native_secrets() {
    use mf_runtime::{ExecutionContext, NodeRegistry, PreparedSubgraph, WorkflowArguments};
    let output = PortSpec::new("value", ValueType::String, true);
    let fallback = PreparedSubgraph::new(vec![], vec![output], |_| {
        Ok([("value".into(), "fallback".into())].into())
    })
    .with_input_schema(
        serde_json::from_value(json!({"inputs":{"query":{"key":{
            "type":"string", "required":true, "binding":{"secret":true}
        }}},"stdin":{}}))
        .unwrap(),
    );
    let mut registry = NodeRegistry::from_inventory().unwrap();
    let sites: CallbackSites = [("lookup".into(), vec!["site".into()])].into();
    let handler = HostCallback::new(|args, _| {
        Ok(FlowOutputs::from([("value".into(), args.0["query.key"].clone())]).into())
    });
    registry
        .bind_callbacks(&sites, &[], &[("lookup".into(), handler)].into())
        .unwrap();
    let bound = registry.bind_callback_body(&sites["lookup"], "lookup", fallback);
    let start = |text: &str| {
        let mut state = ExecutionContext::default();
        state.set_workflow_arguments(WorkflowArguments(
            [("query.key".into(), text.to_owned().into())].into(),
        ));
        bound.start_in_context(state).unwrap()
    };
    let first = start("first");
    let second = start("second");
    for (job, text) in [(second, "second"), (first, "first")] {
        let values = job.join().unwrap().unwrap();
        assert_eq!(values["value"].as_str(), Some(text));
        assert!(values["value"].is_secret());
    }
    assert!(
        bound
            .start_in_context(Default::default())
            .unwrap()
            .join()
            .is_err()
    );
}

#[test]
fn a_callback_is_settled_once_only_after_a_valid_native_delivery() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let events = received.clone();
    let handler = HostCallback::new(move |_, _| {
        let events = events.clone();
        Ok(
            CallbackReply::from(FlowOutputs::from([("value".into(), 1i64.into())]))
                .on_delivery(move |ok| events.lock().unwrap().push(ok)),
        )
    });
    assert_eq!(format!("{handler:?}"), "HostCallback");
    let reply = || {
        handler
            .call(&Default::default(), &Default::default())
            .unwrap()
    };
    let valid = reply();
    valid
        .validate(&[
            PortSpec::new("value", ValueType::Int64, true),
            PortSpec::new("extra", ValueType::Boolean, false),
        ])
        .unwrap();
    assert_eq!(valid.delivered()["value"].as_i64(), Some(1));
    for ports in [
        vec![],
        vec![PortSpec::new("value", ValueType::String, true)],
        vec![
            PortSpec::new("value", ValueType::Int64, true),
            PortSpec::new("extra", ValueType::Int64, true),
        ],
    ] {
        assert!(reply().validate(&ports).is_err());
    }
    let cancelled = StreamCancellation::default();
    cancelled.cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "cancelled",
        }
        .build(),
    );
    assert!(handler.call(&Default::default(), &cancelled).is_err());
    assert_eq!(*received.lock().unwrap(), [true, false, false, false]);
}
