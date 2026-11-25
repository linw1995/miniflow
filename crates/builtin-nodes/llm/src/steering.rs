//! The Agent samples a turn-scoped host inbox only at complete reply boundaries.

use crate::ChatMessage;
use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeMetadata,
    NodeRegistration, NodeValue, Nullable, OutputValue, PreparedNode, Shared, TypedNodeResult,
    TypedTaskNode, deserialize_config,
};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu, ensure};
use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    time::Duration,
};
use uuid::Uuid;

/// The token authenticates pulls, not pushes. Neither it nor the port is model input.
#[derive(Clone, NodeValue)]
pub struct Endpoint {
    pub port: u64,
    pub token: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("Endpoint").finish_non_exhaustive()
    }
}

/// The host can validate positions without receiving any model or tool text.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    pub position: i64,
    pub outcome: crate::CallOutcome,
}

pub const MAX_PENDING_BYTES: usize = 256 * 1024;
pub const MAX_PENDING_MESSAGES: usize = 32;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Snafu)]
pub enum SteeringError {
    #[snafu(display("invalid steering token: {source}"))]
    Token { source: uuid::Error },
    #[snafu(display("invalid steering port: {source}"))]
    Port { source: std::num::TryFromIntError },
    #[snafu(display("invalid steering endpoint or checkpoint"))]
    Invalid,
    #[snafu(display("could not exchange a steering checkpoint: {source}"))]
    Transport { source: io::Error },
    #[snafu(display("invalid steering frame: {source}"))]
    Frame { source: serde_json::Error },
}

impl Endpoint {
    pub fn pull(&self, boundary: Boundary) -> Result<Vec<String>, SteeringError> {
        let token = Uuid::try_parse(&self.token).context(TokenSnafu)?;
        let port = u16::try_from(self.port).context(PortSnafu)?;
        ensure!(port != 0 && boundary.position >= 0, InvalidSnafu);
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into();
        let mut stream = TcpStream::connect_timeout(&address, TIMEOUT).context(TransportSnafu)?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .context(TransportSnafu)?;
        stream
            .set_write_timeout(Some(TIMEOUT))
            .context(TransportSnafu)?;
        stream.write_all(token.as_bytes()).context(TransportSnafu)?;
        serde_json::to_writer(&stream, &boundary).context(FrameSnafu)?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .context(TransportSnafu)?;
        // JSON control characters can expand to six bytes per input byte.
        let limit = 6 * MAX_PENDING_BYTES + 4 * MAX_PENDING_MESSAGES + 2;
        let mut bytes = Vec::new();
        stream
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .context(TransportSnafu)?;
        ensure!(bytes.len() <= limit, InvalidSnafu);
        serde_json::from_slice(&bytes).context(FrameSnafu)
    }
}

#[derive(NodeValue)]
struct Inputs {
    endpoint: Nullable<Endpoint>,
    generated: Shared<Vec<ChatMessage>>,
    outcome: crate::CallOutcome,
}
#[derive(NodeValue)]
struct Outputs {
    generated: Shared<Vec<ChatMessage>>,
    received: bool,
}

struct Checkpoint;
impl TypedTaskNode for Checkpoint {
    type Input = Inputs;
    type Output = Outputs;
    fn execute(
        &self,
        input: Inputs,
        _: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Outputs>, NodeExecutionError> {
        let mut generated = input.generated;
        let original = generated.value();
        let items = original.as_array().expect("typed message list");
        let batch = match input.endpoint {
            Nullable::Null => Vec::new(),
            Nullable::Value(endpoint) => endpoint
                .pull(Boundary {
                    position: i64::try_from(items.len()).boxed()?,
                    outcome: input.outcome,
                })
                .boxed()?,
        };
        let received = !batch.is_empty();
        // The overwhelmingly common empty poll must not decode or recopy tool transcripts.
        if received {
            let mut next = items.to_vec();
            for content in batch {
                next.push(
                    ChatMessage::User {
                        content,
                        metadata: None,
                    }
                    .encode()
                    .boxed()?
                    .into_secret(),
                );
            }
            let value = mf_runtime::ValueRef::array(next);
            generated = Shared::new(if original.is_secret() {
                value.into_secret()
            } else {
                value
            })
            .boxed()?;
        }
        Ok(Outputs {
            generated,
            received,
        }
        .into())
    }
}

fn factory(config: serde_json::Value) -> Result<PreparedNode, NodeBuildError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {}
    let _: Config = deserialize_config(config)?;
    PreparedNode::typed_task(Checkpoint, NodeMetadata::default())
}
inventory::submit! { NodeRegistration { kind: "llm.steering", factory: NodeFactory::Plain(factory) } }

#[cfg(test)]
mod tests {
    use super::*;
    use mf_runtime::ValueRef;
    use std::{net::TcpListener, thread};

    fn run(endpoint: ValueRef, generated: ValueRef) -> Result<ValueRef, NodeExecutionError> {
        let inputs = [
            ("endpoint".into(), endpoint),
            ("generated".into(), generated),
            ("outcome".into(), "completed".into()),
        ]
        .into();
        Ok(
            mf_runtime::execute_typed_task(&Checkpoint, inputs, &mut ExecutionContext::default())?
                .outputs
                .remove("generated")
                .unwrap(),
        )
    }

    #[test]
    fn null_is_offline_and_delivered_text_is_private_and_cannot_set_its_role() {
        let original: ValueRef =
            serde_json::json!([{"role":"assistant","content":"answer"}]).into();
        assert!(
            run(ValueRef::null(), original.clone())
                .unwrap()
                .ptr_eq(&original)
        );
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = Uuid::new_v4();
        let worker = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            connection.read_to_end(&mut bytes).unwrap();
            assert_eq!(&bytes[..16], token.as_bytes());
            let boundary: Boundary = serde_json::from_slice(&bytes[16..]).unwrap();
            assert_eq!(boundary.position, 1);
            assert_eq!(boundary.outcome, crate::CallOutcome::Completed);
            connection.write_all(br#"["private instruction"]"#).unwrap();
        });
        let output = run(
            serde_json::json!({"port":port,"token":token.to_string()}).into(),
            original.clone(),
        )
        .unwrap();
        worker.join().unwrap();
        assert!(!output[0].is_secret());
        assert!(output[0].ptr_eq(&original[0]));
        assert!(output[1].is_secret());
        assert_eq!(
            output[1],
            serde_json::json!({"role":"user","content":"private instruction"})
        );
    }

    #[test]
    fn invalid_capabilities_fail_without_defaulting_to_another_endpoint() {
        for (port, token) in [
            (80, "bad".into()),
            (0, Uuid::new_v4().to_string()),
            (65536, Uuid::new_v4().to_string()),
        ] {
            assert!(
                run(
                    serde_json::json!({"port":port,"token":token}).into(),
                    ValueRef::array([])
                )
                .is_err()
            );
        }
        assert!(factory(serde_json::json!({"endpoint":"not a binding"})).is_err());
    }
}
