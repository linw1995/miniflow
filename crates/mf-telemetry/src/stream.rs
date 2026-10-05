use crate::{
    ContractError, Count, INSTRUMENTATION_SCOPE, InvalidSnafu, LOOP_EVENT_SCHEMA_VERSION,
    STREAM_EVENT_SCHEMA_VERSION,
    event::{Event, LifecycleEvent, NodeIdentity},
    identity::{RunId, WorkflowId},
    wire::{TraceContext, WireRecord},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use snafu::{OptionExt, ensure};

pub mod counter {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        let value = text.parse::<u64>().map_err(serde::de::Error::custom)?;
        if value.to_string() != text {
            return Err(serde::de::Error::custom(
                "counter must be a canonical unsigned decimal string",
            ));
        }
        Ok(value)
    }
}

mod optional_counter {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
        value.map(|value| value.to_string()).serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                let value = text.parse::<u64>().map_err(serde::de::Error::custom)?;
                if value.to_string() != text {
                    return Err(serde::de::Error::custom("counter must be canonical"));
                }
                Ok(value)
            })
            .transpose()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamMessage {
    pub domain: usize,
    #[serde(with = "counter")]
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamTrigger {
    Startup,
    Message,
    Input,
    Timer,
    UpstreamClosed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamIdentity {
    #[serde(with = "counter")]
    pub invocation: u64,
    pub trigger: StreamTrigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<StreamMessage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_counter"
    )]
    pub parent: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamOutcome {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamCounts {
    #[serde(default, with = "counter")]
    pub startup_frames: u64,
    #[serde(default, with = "counter")]
    pub accepted_inputs: u64,
    #[serde(with = "counter")]
    pub emitted_messages: u64,
    #[serde(with = "counter")]
    pub completed_frames: u64,
    #[serde(with = "counter")]
    pub delivered_outputs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFailure {
    pub phase: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event_name", content = "body")]
pub enum StreamEvent {
    #[serde(rename = "mf.workflow.started")]
    Started {
        node_count: Count,
        elapsed_ns: Count,
    },
    #[serde(rename = "mf.workflow.finished")]
    Finished {
        final_sequence: Count,
        elapsed_ns: Count,
        outcome: StreamOutcome,
        counts: StreamCounts,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<StreamFailure>,
    },
    #[serde(rename = "mf.batch.buffered")]
    Buffered {
        node: NodeIdentity,
        item_count: Count,
        elapsed_ns: Count,
    },
    #[serde(rename = "mf.batch.flushed")]
    Flushed {
        node: NodeIdentity,
        output: StreamMessage,
        item_count: Count,
        reason: String,
        elapsed_ns: Count,
    },
}

impl StreamEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Started { .. } => "mf.workflow.started",
            Self::Finished { .. } => "mf.workflow.finished",
            Self::Buffered { .. } => "mf.batch.buffered",
            Self::Flushed { .. } => "mf.batch.flushed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamPayload {
    Execution(Event),
    Control(StreamEvent),
}

impl StreamPayload {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Execution(event) => event.name(),
            Self::Control(event) => event.name(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StreamRecord {
    pub schema_version: i64,
    pub workflow_id: WorkflowId,
    pub run_id: RunId,
    pub sequence: Count,
    pub identity: Option<StreamIdentity>,
    pub payload: StreamPayload,
    pub emission_count: Option<Count>,
}

impl StreamRecord {
    pub fn validate(&self) -> Result<(), ContractError> {
        ensure!(
            matches!(
                self.schema_version,
                crate::LEGACY_STREAM_EVENT_SCHEMA_VERSION | STREAM_EVENT_SCHEMA_VERSION
            ),
            InvalidSnafu {
                message: "unsupported stream event schema"
            }
        );
        ensure!(
            self.schema_version == STREAM_EVENT_SCHEMA_VERSION
                || !self
                    .identity
                    .as_ref()
                    .is_some_and(|identity| identity.trigger == StreamTrigger::Startup),
            InvalidSnafu {
                message: "startup requires source-driven stream observation"
            }
        );
        ensure!(
            self.sequence.get() > 0,
            InvalidSnafu {
                message: "stream event sequence must be positive",
            }
        );
        ensure!(
            self.emission_count.is_none()
                || matches!(
                    self.payload,
                    StreamPayload::Execution(
                        Event::NodeFinished {
                            outcome: crate::event::Outcome::Succeeded,
                            ..
                        } | Event::NodeSkipped { .. }
                    )
                ),
            InvalidSnafu {
                message: "emission count requires a successful or skipped invocation",
            }
        );
        if let Some(identity) = &self.identity {
            ensure!(
                identity
                    .parent
                    .is_none_or(|parent| parent < identity.invocation),
                InvalidSnafu {
                    message: "stream invocation parent must precede its child",
                }
            );
            ensure!(
                matches!(
                    identity.trigger,
                    StreamTrigger::Message | StreamTrigger::Input
                ) == identity.message.is_some(),
                InvalidSnafu {
                    message: "stream trigger and message identity disagree",
                }
            );
            if let Some(message) = identity.message {
                validate_message(message)?;
            }
        }
        match &self.payload {
            StreamPayload::Execution(event) => {
                ensure!(
                    !matches!(
                        event,
                        Event::WorkflowStarted { .. } | Event::WorkflowFinished { .. }
                    ),
                    InvalidSnafu {
                        message: "workflow boundaries use stream control records",
                    }
                );
                ensure!(
                    self.sequence.get() > 1 && self.identity.is_some(),
                    InvalidSnafu {
                        message: "stream execution requires invocation identity",
                    }
                );
                event.validate()?;
            }
            StreamPayload::Control(StreamEvent::Started { elapsed_ns, .. }) => {
                ensure!(
                    self.sequence.get() == 1
                        && *elapsed_ns == Count::ZERO
                        && self.identity.is_none(),
                    InvalidSnafu {
                        message: "invalid stream start boundary",
                    }
                );
            }
            StreamPayload::Control(StreamEvent::Finished {
                final_sequence,
                outcome,
                failure,
                counts,
                ..
            }) => {
                ensure!(
                    *final_sequence == self.sequence
                        && self.sequence.get() > 1
                        && self.identity.is_none(),
                    InvalidSnafu {
                        message: "invalid stream terminal boundary",
                    }
                );
                ensure!(
                    (*outcome == StreamOutcome::Succeeded) == failure.is_none(),
                    InvalidSnafu {
                        message: "stream failure must match outcome",
                    }
                );
                ensure!(
                    if self.schema_version == STREAM_EVENT_SCHEMA_VERSION {
                        counts.startup_frames <= 1 && counts.accepted_inputs == 0
                    } else {
                        counts.startup_frames == 0
                    },
                    InvalidSnafu {
                        message: "stream counts disagree with protocol"
                    }
                );
                ensure!(
                    counts.delivered_outputs <= counts.completed_frames
                        && u128::from(counts.completed_frames)
                            <= u128::from(counts.accepted_inputs)
                                + u128::from(counts.startup_frames)
                                + u128::from(counts.emitted_messages),
                    InvalidSnafu {
                        message: "stream terminal counts are inconsistent",
                    }
                );
                if let Some(failure) = failure {
                    ensure!(
                        failure
                            .node
                            .as_ref()
                            .is_none_or(|node| !node.trim().is_empty()),
                        InvalidSnafu {
                            message: "blank stream failure node",
                        }
                    );
                    ensure!(
                        !failure.message.trim().is_empty()
                            && matches!(
                                failure.phase.as_str(),
                                "preparation"
                                    | "input"
                                    | "dependency"
                                    | "execution"
                                    | "publication"
                                    | "output"
                                    | "resource"
                            ),
                        InvalidSnafu {
                            message: "invalid stream failure",
                        }
                    );
                }
            }
            StreamPayload::Control(
                StreamEvent::Buffered { node, .. } | StreamEvent::Flushed { node, .. },
            ) => {
                ensure!(
                    self.identity.is_some() && self.sequence.get() > 1,
                    InvalidSnafu {
                        message: "batch observation requires invocation identity",
                    }
                );
                ensure!(
                    !node.id.trim().is_empty() && !node.kind.trim().is_empty(),
                    InvalidSnafu {
                        message: "blank batch node identity",
                    }
                );
                if let StreamPayload::Control(StreamEvent::Flushed {
                    output,
                    item_count,
                    reason,
                    ..
                }) = &self.payload
                {
                    validate_message(*output)?;
                    ensure!(
                        output.domain > 0
                            && item_count.get() > 0
                            && matches!(
                                reason.as_str(),
                                "size_exceed" | "timeout_exceed" | "upstream_closed"
                            ),
                        InvalidSnafu {
                            message: "invalid batch flush",
                        }
                    );
                }
            }
        }
        Ok(())
    }

    pub fn to_wire(
        &self,
        timestamp: u64,
        trace: Option<TraceContext>,
    ) -> Result<WireRecord, ContractError> {
        self.validate()?;
        let mut wire = match &self.payload {
            StreamPayload::Execution(event) => WireRecord::from_event_with_version(
                &LifecycleEvent {
                    workflow_id: self.workflow_id.clone(),
                    run_id: self.run_id,
                    sequence: self.sequence,
                    event: event.clone(),
                },
                LOOP_EVENT_SCHEMA_VERSION,
                timestamp,
                trace,
            )?,
            StreamPayload::Control(event) => {
                let mut value = serde_json::to_value(event)?;
                let body = value["body"]
                    .as_object_mut()
                    .expect("control body is an object");
                let mut attributes = Map::from_iter([
                    ("mf.workflow.id".into(), json!(self.workflow_id)),
                    ("mf.run.id".into(), json!(self.run_id)),
                    ("mf.event.sequence".into(), json!(self.sequence)),
                ]);
                if let Some(node) = body.remove("node") {
                    attributes.insert("mf.node.id".into(), node["id"].clone());
                    attributes.insert("mf.node.kind".into(), node["kind"].clone());
                }
                if let Some(outcome) = body.remove("outcome") {
                    attributes.insert("mf.outcome".into(), outcome);
                }
                WireRecord {
                    scope: INSTRUMENTATION_SCOPE.into(),
                    event_name: event.name().into(),
                    time_unix_nano: timestamp,
                    trace_context: trace,
                    attributes,
                    body: Value::Object(body.clone()),
                }
            }
        };
        wire.attributes
            .insert("mf.schema.version".into(), json!(self.schema_version));
        if let Some(counts) = wire.body.get_mut("counts").and_then(Value::as_object_mut) {
            counts.remove(if self.schema_version == STREAM_EVENT_SCHEMA_VERSION {
                "accepted_inputs"
            } else {
                "startup_frames"
            });
        }
        if let Some(identity) = &self.identity {
            wire.body
                .as_object_mut()
                .unwrap()
                .insert("stream".into(), serde_json::to_value(identity)?);
        }
        if let Some(count) = self.emission_count {
            wire.body
                .as_object_mut()
                .unwrap()
                .insert("emission_count".into(), json!(count));
        }
        Ok(wire)
    }

    pub fn decode(wire: &WireRecord) -> Result<Self, ContractError> {
        ensure!(
            wire.scope == INSTRUMENTATION_SCOPE
                && matches!(
                    wire.attributes
                        .get("mf.schema.version")
                        .and_then(Value::as_i64),
                    Some(crate::LEGACY_STREAM_EVENT_SCHEMA_VERSION | STREAM_EVENT_SCHEMA_VERSION)
                ),
            InvalidSnafu {
                message: "unsupported stream event schema",
            }
        );
        if let Some(trace) = &wire.trace_context {
            trace.validate()?;
        }
        let attribute = |name: &str| {
            wire.attributes
                .get(name)
                .cloned()
                .with_context(|| InvalidSnafu {
                    message: format!("missing attribute {name}"),
                })
        };
        let mut body = wire
            .body
            .as_object()
            .context(InvalidSnafu {
                message: "stream event body must be a map",
            })?
            .clone();
        let identity = body
            .remove("stream")
            .map(serde_json::from_value)
            .transpose()?;
        let emission_count = body
            .remove("emission_count")
            .map(serde_json::from_value)
            .transpose()?;
        let payload = if wire.event_name.starts_with("mf.node.")
            || wire.event_name.starts_with("mf.loop.pass.")
        {
            let mut legacy = wire.clone();
            legacy.body = Value::Object(body);
            legacy
                .attributes
                .insert("mf.schema.version".into(), json!(LOOP_EVENT_SCHEMA_VERSION));
            StreamPayload::Execution(legacy.decode()?.event)
        } else {
            if wire.event_name.starts_with("mf.batch.") {
                body.insert("node".into(), json!({"id":attribute("mf.node.id")?, "kind":attribute("mf.node.kind")?, "path":[]}));
            }
            if wire.event_name == "mf.workflow.finished" {
                body.insert("outcome".into(), attribute("mf.outcome")?);
            }
            StreamPayload::Control(serde_json::from_value(
                json!({"event_name":wire.event_name, "body":body}),
            )?)
        };
        let record = Self {
            schema_version: wire.attributes["mf.schema.version"]
                .as_i64()
                .expect("validated schema"),
            workflow_id: serde_json::from_value(attribute("mf.workflow.id")?)?,
            run_id: serde_json::from_value(attribute("mf.run.id")?)?,
            sequence: serde_json::from_value(attribute("mf.event.sequence")?)?,
            identity,
            payload,
            emission_count,
        };
        record.validate()?;
        Ok(record)
    }
}

fn validate_message(message: StreamMessage) -> Result<(), ContractError> {
    ensure!(
        i64::try_from(message.domain).is_ok(),
        InvalidSnafu {
            message: "message domain exceeds the observation range",
        }
    );
    Ok(())
}
