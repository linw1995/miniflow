use crate::ChatMessage;
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, PreparedNode, Shared, TypedNodeResult, TypedTaskNode, ValueRef,
    deserialize_config,
};
use serde::Deserialize;
use serde_json::Value;
use snafu::ResultExt;

type Messages = Shared<Vec<ChatMessage>>;

struct AgentRequest;

#[derive(NodeValue)]
struct RequestInputs {
    prefix: Messages,
    generated: Messages,
}

#[derive(NodeValue)]
struct RequestOutputs {
    messages: Messages,
}

impl TypedTaskNode for AgentRequest {
    type Input = RequestInputs;
    type Output = RequestOutputs;

    fn execute(
        &self,
        input: RequestInputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<RequestOutputs>, NodeExecutionError> {
        let prefix = input.prefix.into_value();
        let generated = input.generated.value();
        let suffix = generated.as_array().expect("validated message list");
        // A CEL join here would apply a JSON budget before the provider request.
        // Leave validation to the model and the existing bounded append steps.
        let messages = if suffix.is_empty() {
            prefix.inherit_secrets([generated])
        } else {
            let initial = prefix.as_array().expect("validated message list");
            ValueRef::array(initial.iter().chain(suffix).cloned())
                .inherit_secrets([&prefix, generated])
        };
        Ok(RequestOutputs {
            messages: Shared::new(messages).boxed()?,
        }
        .into())
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    PreparedNode::typed_task(AgentRequest, NodeMetadata::default())
}

inventory::submit! {
    NodeRegistration { kind: "llm.agent_request", factory: NodeFactory::Plain(factory) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::{Inputs, execute_typed_task};
    use serde_json::json;

    fn request(prefix: ValueRef, generated: ValueRef) -> ValueRef {
        let inputs = Inputs::from([("prefix".into(), prefix), ("generated".into(), generated)]);
        execute_typed_task(&AgentRequest, inputs, &mut ExecutionContext::default())
            .unwrap()
            .outputs
            .remove("messages")
            .unwrap()
    }

    #[test]
    fn empty_suffix_reuses_the_initial_request() {
        let prefix = ValueRef::from(json!([{"role":"user","content":"Hello"}])).into_secret();
        assert!(request(prefix.clone(), json!([]).into()).ptr_eq(&prefix));
    }

    #[test]
    fn assembled_request_preserves_order_and_inherits_either_segments_secrecy() {
        let user = json!({"role":"user","content":"Hello"});
        let assistant = json!({"role":"assistant","content":"World"});
        for (secret_prefix, secret_generated) in [(false, false), (true, false), (false, true)] {
            let prefix = ValueRef::from(json!([user]));
            let generated = ValueRef::from(json!([assistant]));
            // A nested secret must propagate just like a secret container.
            let generated = if secret_generated {
                ValueRef::array([generated[0].clone().into_secret()])
            } else {
                generated
            };
            let prefix = if secret_prefix {
                prefix.into_secret()
            } else {
                prefix
            };
            let output = request(prefix, generated);
            assert_eq!(
                serde_json::to_value(&output).unwrap(),
                json!([user, assistant])
            );
            assert_eq!(output.is_secret(), secret_prefix || secret_generated);
        }
    }
}
