//! Rust expressions for compiler-resolved runtime data, without an intermediate wire format.

use mf_runtime::{
    InputBinding, NodeContract, PortSpec, StdinRequirement, ValueType, WorkflowInputSchema,
};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

pub fn value_type(value: &ValueType) -> TokenStream {
    use ValueType::*;
    match value {
        List(inner) | Map(inner) | Nullable(inner) => {
            let kind = match value {
                List(_) => quote!(List),
                Map(_) => quote!(Map),
                Nullable(_) => quote!(Nullable),
                _ => unreachable!(),
            };
            let inner = value_type(inner);
            quote!(mf_runtime::ValueType::#kind(Box::new(#inner)))
        }
        Any | Null | Boolean | Number | Int64 | Uint64 | Usize | Float32 | Float64 | String
        | Array | Object | Flow => {
            let kind = format_ident!("{value:?}");
            quote!(mf_runtime::ValueType::#kind)
        }
    }
}

pub fn json(value: &serde_json::Value) -> TokenStream {
    use serde_json::Value;
    match value {
        Value::Null => quote!(serde_json::Value::Null),
        Value::Bool(value) => quote!(serde_json::Value::Bool(#value)),
        Value::Number(value) => {
            if let Some(value) = value.as_u64() {
                quote!(serde_json::Value::Number(serde_json::Number::from(#value)))
            } else if let Some(value) = value.as_i64() {
                quote!(serde_json::Value::Number(serde_json::Number::from(#value)))
            } else {
                let value = value.as_f64().expect("JSON numbers are finite");
                quote!(serde_json::Value::Number(serde_json::Number::from_f64(#value).expect("compiled finite number")))
            }
        }
        Value::String(value) => quote!(serde_json::Value::String(#value.into())),
        Value::Array(values) => {
            let values = values.iter().map(json);
            quote!(serde_json::Value::Array(vec![#(#values),*]))
        }
        Value::Object(values) => {
            let entries = values.iter().map(|(name, value)| {
                let value = json(value);
                quote!((#name.into(), #value))
            });
            quote!(serde_json::Value::Object(serde_json::Map::from_iter([#(#entries),*])))
        }
    }
}

pub fn option<T>(value: Option<&T>, tokens: impl FnOnce(&T) -> TokenStream) -> TokenStream {
    match value {
        Some(value) => {
            let value = tokens(value);
            quote!(Some(#value))
        }
        None => quote!(None),
    }
}

pub fn input_binding(binding: &InputBinding) -> TokenStream {
    let env = option(binding.env.as_ref(), |name| quote!(#name.into()));
    let default = option(binding.default.as_ref(), json);
    let secret = binding.secret;
    quote!(mf_runtime::InputBinding { env: #env, default: #default, secret: #secret })
}

pub fn stdin(requirement: &StdinRequirement) -> TokenStream {
    match requirement {
        StdinRequirement::Always => quote!(mf_runtime::StdinRequirement::Always),
        StdinRequirement::UnlessInput(name) => {
            quote!(mf_runtime::StdinRequirement::UnlessInput(#name.into()))
        }
    }
}

pub fn port(port: &PortSpec) -> TokenStream {
    let name = port.name.as_ref();
    let kind = value_type(&port.value_type);
    let required = port.required;
    quote!(mf_runtime::PortSpec::new(#name, #kind, #required))
}

pub fn rust_type(value: &mf_runtime::RustValueType) -> TokenStream {
    use mf_runtime::RustValueType::*;
    match value {
        List(inner) | Map(inner) | Defaulted(inner) | Nullable(inner) | SharedPayload(inner)
        | Optional(inner) => {
            let kind = match value {
                List(_) => quote!(List),
                Map(_) => quote!(Map),
                Defaulted(_) => quote!(Defaulted),
                Nullable(_) => quote!(Nullable),
                SharedPayload(_) => quote!(SharedPayload),
                Optional(_) => quote!(Optional),
                _ => unreachable!(),
            };
            let inner = rust_type(inner);
            quote!(mf_runtime::RustValueType::#kind(Box::new(#inner)))
        }
        Boolean | Int64 | Uint64 | Usize | Float32 | Float64 | String | Shared => {
            let kind = format_ident!("{value:?}");
            quote!(mf_runtime::RustValueType::#kind)
        }
    }
}

fn typed_generation(generation: &mf_runtime::TypedGeneration) -> TokenStream {
    let package = generation.constructor.package;
    let path = generation.constructor.path;
    let fields = |fields: &[mf_runtime::TypedPort]| {
        fields
            .iter()
            .map(|field| {
                let port = port(&field.port);
                let kind = rust_type(&field.rust_type);
                quote!(mf_runtime::TypedPort { port: #port, rust_type: #kind })
            })
            .collect::<Vec<_>>()
    };
    let inputs = fields(&generation.inputs);
    let outputs = fields(&generation.outputs);
    let context_free = generation.context_free;
    quote!(mf_runtime::TypedGeneration {
        constructor: mf_runtime::TypedConstructor { package: #package, path: &[#(#path),*] },
        inputs: vec![#(#inputs),*], outputs: vec![#(#outputs),*], context_free: #context_free,
    })
}

pub fn node_contract(contract: &NodeContract) -> TokenStream {
    let generation = option(contract.typed_generation.as_ref(), typed_generation);
    let inputs = contract.ports.inputs.iter().map(port);
    let outputs = contract.ports.outputs.iter().map(port);
    let streams = contract.stream_outputs.iter().map(port);
    let exclusive = &contract.exclusive_outputs;
    let bindings = contract.input_bindings.iter().map(|(name, binding)| {
        let binding = input_binding(binding);
        quote!((#name.into(), #binding))
    });
    let references = contract.context_references.iter().map(|reference| {
        let output = &reference.output;
        let label = &reference.label;
        quote!(mf_runtime::ContextReference::new(#output, #label))
    });
    let stdin = option(contract.stdin.as_ref(), stdin);
    quote!(mf_runtime::NodeContract {
        typed_generation: #generation,
        ports: mf_runtime::NodePorts { inputs: vec![#(#inputs),*], outputs: vec![#(#outputs),*] },
        stream_outputs: vec![#(#streams),*],
        exclusive_outputs: vec![#(#exclusive.into()),*],
        input_bindings: std::collections::BTreeMap::from([#(#bindings),*]),
        context_references: vec![#(#references),*], stdin: #stdin,
    })
}

pub fn input_schema(schema: &WorkflowInputSchema) -> TokenStream {
    let nodes = schema.inputs.iter().map(|(node, ports)| {
        let ports = ports.iter().map(|(name, port)| {
            let kind = value_type(&port.value_type);
            let required = port.required;
            let binding = input_binding(&port.binding);
            quote!((#name.into(), mf_runtime::WorkflowInput { value_type: #kind, required: #required, binding: #binding }))
        });
        quote!((#node.into(), std::collections::BTreeMap::from([#(#ports),*])))
    });
    let owners = schema.stdin.iter().map(|(node, requirement)| {
        let requirement = stdin(requirement);
        quote!((#node.into(), #requirement))
    });
    quote!(mf_runtime::WorkflowInputSchema {
        inputs: std::collections::BTreeMap::from([#(#nodes),*]),
        stdin: std::collections::BTreeMap::from([#(#owners),*]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expressions_are_rust_not_serialized_runtime_descriptors() {
        for value in [
            ValueType::Any,
            ValueType::List(Box::new(ValueType::Nullable(Box::new(ValueType::Int64)))),
            ValueType::Flow,
        ] {
            syn::parse2::<syn::Expr>(value_type(&value)).unwrap();
        }
        let binding = InputBinding {
            env: Some("API_KEY".into()),
            default: Some(json!({"null":null, "numbers":[-1, u64::MAX, 1.25], "text":"quoted\""})),
            secret: true,
        };
        syn::parse2::<syn::Expr>(input_binding(&binding)).unwrap();
        syn::parse2::<syn::Expr>(input_schema(&WorkflowInputSchema::default())).unwrap();
        let metadata = mf_runtime::NodeMetadata::new(mf_runtime::NodePorts {
            inputs: vec![PortSpec::new("input", ValueType::String, true)],
            outputs: vec![],
        });
        let expression = node_contract(&NodeContract::from(&metadata));
        syn::parse2::<syn::Expr>(expression.clone()).unwrap();
        assert!(!expression.to_string().contains("serde_json"));
    }
}
