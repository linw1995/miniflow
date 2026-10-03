use crate::stream_instance::{
    MemorySizeSnafu, OutputEncodeSnafu, PayloadSizeSnafu, PreparationSnafu, ResourceSnafu,
};
use crate::value::VALUE_HEAP_BYTES;
use crate::{MESSAGE_OVERHEAD, Outputs, StreamError, StreamPlan, ValueRef, ValueType, output_id};
use serde::Serialize;
use snafu::{OptionExt, ResultExt, ensure};
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
                .context(ResourceSnafu {
                    message: "byte accounting overflow",
                })?;
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
                    .context(PreparationSnafu {
                        message: "unknown source port in resource plan",
                    })?;
                let target = node
                    .metadata
                    .ports
                    .inputs
                    .iter()
                    .find(|port| port.name == *input)
                    .context(PreparationSnafu {
                        message: "unknown input port in resource plan",
                    })?;
                let bytes = type_bound(&output.value_type, limits.max_message_bytes)
                    .min(type_bound(&target.value_type, limits.max_message_bytes));
                event_bytes[index] = add(event_bytes[index], add(binding_bytes(input)?, bytes)?)?;
            }
            for reference in &node.metadata.context_references {
                let ty = references
                    .get(&reference.output)
                    .context(PreparationSnafu {
                        message: "unknown context reference in resource plan",
                    })?;
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
                        .context(PreparationSnafu {
                            message: "unknown selected output in resource plan",
                        })?;
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
        ensure!(
            required <= limits.max_buffered_bytes,
            PreparationSnafu {
                message: format!(
                    "max_buffered_bytes must be at least {required} for this graph's frame, callback, flush, and input reserves"
                ),
            }
        );
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
        ValueType::Int64 | ValueType::Boolean | ValueType::Null => VALUE_HEAP_BYTES.min(limit),
        _ => limit,
    }
}

pub fn binding_bytes(name: &str) -> Result<usize, StreamError> {
    add(MESSAGE_OVERHEAD, name.len())
}

pub fn output_bytes(outputs: &Outputs, limit: usize) -> Result<usize, StreamError> {
    payload_bytes(outputs.values(), limit)
}

pub fn payload_bytes<'a>(
    values: impl IntoIterator<Item = &'a ValueRef>,
    limit: usize,
) -> Result<usize, StreamError> {
    values.into_iter().try_fold(0usize, |sum, value| {
        add(sum, memory_size(value, limit.saturating_sub(sum))?)
    })
}

pub fn memory_size(value: &ValueRef, limit: usize) -> Result<usize, StreamError> {
    let bytes = value.estimated_heap_bytes();
    ensure!(bytes <= limit, MemorySizeSnafu { limit });
    Ok(bytes)
}

pub fn add(left: usize, right: usize) -> Result<usize, StreamError> {
    left.checked_add(right).context(ResourceSnafu {
        message: "byte accounting overflow",
    })
}

pub fn encode_json(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, StreamError> {
    struct Buffer {
        bytes: Vec<u8>,
        limit: usize,
        exceeded: bool,
    }
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self
                .bytes
                .len()
                .checked_add(bytes.len())
                .is_none_or(|length| length > self.limit)
            {
                self.exceeded = true;
                return Err(io::Error::other("encoded payload exceeds its byte limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Buffer {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    let result = serde_json::to_writer(&mut buffer, value);
    ensure!(!buffer.exceeded, PayloadSizeSnafu { limit });
    result.context(OutputEncodeSnafu)?;
    Ok(buffer.bytes)
}

#[cfg(test)]
mod tests {
    use super::encode_json;
    use crate::StreamError;
    use serde::Serialize;
    use std::cell::Cell;

    #[test]
    fn output_encoding_is_single_pass_and_bounded() {
        struct Record<'a>(&'a Cell<usize>);
        impl Serialize for Record<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.set(self.0.get() + 1);
                serializer.serialize_str("hello")
            }
        }
        let calls = Cell::new(0);
        let record = Record(&calls);
        assert_eq!(encode_json(&record, 7).unwrap(), br#""hello""#);
        assert_eq!(calls.get(), 1);
        assert!(matches!(
            encode_json(&record, 6),
            Err(StreamError::PayloadSize { limit: 6 })
        ));
        assert_eq!(calls.get(), 2);

        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("invalid output"))
            }
        }
        let error = encode_json(&Invalid, 64).unwrap_err();
        assert!(
            matches!(&error, StreamError::OutputEncode { source } if source.to_string() == "invalid output")
        );
    }
}
