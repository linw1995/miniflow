use crate::{MESSAGE_OVERHEAD, Outputs, StreamError, StreamPlan, ValueRef, ValueType, output_id};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{self, Write},
};

pub struct StreamResources {
    pub frame_bytes: Vec<usize>,
    pub event_bytes: Vec<usize>,
    pub domain_credits: Vec<usize>,
    pub seal_bytes: Vec<usize>,
    pub fixed_bytes: usize,
    pub soft_bytes: usize,
    pub hard_bytes: usize,
    pub source_overhead: usize,
    pub source_bytes: usize,
}

impl StreamResources {
    pub fn new(plan: &StreamPlan) -> Result<Self, StreamError> {
        let limits = &plan.execution().limits;
        if limits.max_pending_messages < plan.domains().len() {
            return Err(invalid(format!(
                "max_pending_messages must reserve at least {} domain slots",
                plan.domains().len()
            )));
        }
        let mut output_bounds = Vec::new();
        let mut indices = BTreeMap::new();
        let mut references = BTreeMap::new();
        for (index, node) in plan.nodes().iter().enumerate() {
            indices.insert(node.definition_id.as_str(), index);
            let mut metadata = 0;
            let mut payload = 0usize;
            for port in &node.metadata.ports.outputs {
                let name = output_id(node.definition_id.as_str(), &port.name);
                metadata = add(metadata, binding_bytes(&name)?)?;
                payload = payload
                    .saturating_add(type_bound(&port.value_type, limits.max_message_bytes))
                    .min(limits.max_message_bytes);
                references.insert(name, &port.value_type);
            }
            output_bounds.push(add(metadata, payload)?);
        }
        let mut event_bytes = vec![0; plan.nodes().len()];
        let mut seal_bytes = vec![0; plan.nodes().len()];
        for (index, node) in plan.nodes().iter().enumerate() {
            if index == 0 || node.node.is_some() {
                continue;
            }
            seal_bytes[index] = add(MESSAGE_OVERHEAD, output_bounds[index])?
                .checked_mul(2)
                .ok_or_else(overflow)?;
            for dependency in plan.dependencies(index) {
                let Some(input) = &dependency.input else {
                    continue;
                };
                let source = &plan.nodes()[indices[dependency.source_node.as_str()]];
                let output = source
                    .metadata
                    .ports
                    .outputs
                    .iter()
                    .find(|port| port.name == dependency.source_output)
                    .ok_or_else(|| invalid("unknown source port in resource plan"))?;
                let target = node
                    .metadata
                    .ports
                    .inputs
                    .iter()
                    .find(|port| port.name == *input)
                    .ok_or_else(|| invalid("unknown input port in resource plan"))?;
                let bytes = type_bound(&output.value_type, limits.max_message_bytes)
                    .min(type_bound(&target.value_type, limits.max_message_bytes));
                event_bytes[index] = add(event_bytes[index], add(binding_bytes(input)?, bytes)?)?;
            }
            for reference in &node.metadata.context_references {
                let ty = references
                    .get(&reference.output)
                    .ok_or_else(|| invalid("unknown context reference in resource plan"))?;
                event_bytes[index] = add(
                    event_bytes[index],
                    add(
                        binding_bytes(&reference.output)?,
                        type_bound(ty, limits.max_message_bytes),
                    )?,
                )?;
            }
        }
        let mut frame_bytes = Vec::new();
        let mut domain_credits = Vec::new();
        for (index, domain) in plan.domains().iter().enumerate() {
            let mut bytes = add(MESSAGE_OVERHEAD, output_bounds[domain.source])?;
            let mut credits = 0;
            for &node in &domain.steps {
                if plan.nodes()[node].node.is_some() {
                    bytes = add(bytes, output_bounds[node])?;
                } else {
                    credits = add(credits, event_bytes[node])?;
                }
            }
            if plan.selected_domain() == Some(index) {
                let mut payload = 0usize;
                bytes = add(bytes, MESSAGE_OVERHEAD)?;
                for output in plan.outputs() {
                    let source = &plan.nodes()[indices[output.node.as_str()]];
                    let port = source
                        .metadata
                        .ports
                        .outputs
                        .iter()
                        .find(|port| port.name == output.port)
                        .ok_or_else(|| invalid("unknown selected output in resource plan"))?;
                    bytes = add(bytes, binding_bytes(&output.name)?)?;
                    payload = payload
                        .saturating_add(type_bound(&port.value_type, limits.max_message_bytes))
                        .min(limits.max_message_bytes);
                }
                bytes = add(bytes, payload)?;
            }
            frame_bytes.push(bytes);
            domain_credits.push(credits);
        }
        let fixed_bytes = frame_bytes
            .iter()
            .chain(seal_bytes.iter())
            .try_fold(0usize, |sum, bytes| add(sum, *bytes))?;
        let credits = event_bytes
            .iter()
            .try_fold(0usize, |sum, bytes| add(sum, *bytes))?;
        let reserve = add(fixed_bytes, credits)?;
        let minimum_input = add(MESSAGE_OVERHEAD, output_bounds[0])?;
        let required = add(reserve, minimum_input)?;
        if required > limits.max_buffered_bytes {
            return Err(invalid(format!(
                "max_buffered_bytes must be at least {required} for this graph's frame, callback, flush, and input reserves"
            )));
        }
        let source_overhead = add(
            MESSAGE_OVERHEAD,
            binding_bytes(&output_id(crate::STREAM_INPUT_ID, "item"))?,
        )?;
        Ok(Self {
            frame_bytes,
            event_bytes,
            domain_credits,
            seal_bytes,
            fixed_bytes,
            soft_bytes: limits.max_buffered_bytes - reserve,
            hard_bytes: limits.max_buffered_bytes - fixed_bytes,
            source_overhead,
            source_bytes: minimum_input,
        })
    }
}

fn type_bound(ty: &ValueType, limit: usize) -> usize {
    match ty {
        ValueType::Int64 => 20.min(limit),
        ValueType::Boolean => 5.min(limit),
        ValueType::Null => 4.min(limit),
        _ => limit,
    }
}

pub fn binding_bytes(name: &str) -> Result<usize, StreamError> {
    add(MESSAGE_OVERHEAD, encoded_size(&name, usize::MAX)?)
}

pub fn output_bytes(outputs: &Outputs, limit: usize) -> Result<usize, StreamError> {
    payload_bytes(outputs.values(), limit)
}

pub fn payload_bytes<'a>(
    values: impl IntoIterator<Item = &'a ValueRef>,
    limit: usize,
) -> Result<usize, StreamError> {
    values.into_iter().try_fold(0usize, |sum, value| {
        add(sum, encoded_size(value, limit.saturating_sub(sum))?)
    })
}

pub fn add(left: usize, right: usize) -> Result<usize, StreamError> {
    left.checked_add(right).ok_or_else(overflow)
}

fn overflow() -> StreamError {
    StreamError::Resource {
        message: "byte accounting overflow".into(),
    }
}
fn invalid(message: impl Into<String>) -> StreamError {
    StreamError::Preparation {
        message: message.into(),
    }
}

pub fn encoded_size(value: &impl Serialize, limit: usize) -> Result<usize, StreamError> {
    struct Counter {
        length: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let length = self
                .length
                .checked_add(bytes.len())
                .filter(|length| *length <= self.limit)
                .ok_or_else(|| io::Error::other("encoded payload exceeds its byte limit"))?;
            self.length = length;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { length: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|error| StreamError::Resource {
        message: error.to_string(),
    })?;
    Ok(counter.length)
}
