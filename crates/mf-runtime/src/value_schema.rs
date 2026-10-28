use crate::{InputDecodeError, InputValue, PortSpec, TypeMismatch, ValueRef, ValueType};
use snafu::{ResultExt, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_SCHEMA_NODES: usize = 4096;
pub const MAX_SCHEMA_DEPTH: usize = 32;
pub const MAX_SCHEMA_TEXT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaOpacity {
    Codec,
    Recursive,
    Limit,
}

/// Structural constraints on decoded values, without defaults or runtime data.
/// Shared payloads retain their underlying schema while decoding may remain lazy.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ValueSchema {
    Type(ValueType),
    Object(BTreeMap<String, SchemaField>),
    List(Box<ValueSchema>),
    Map(Box<ValueSchema>),
    Nullable(Box<ValueSchema>),
    Enum(Vec<String>),
    Tagged {
        tag: String,
        variants: BTreeMap<String, BTreeMap<String, SchemaField>>,
    },
    Described {
        schema: Box<ValueSchema>,
        description: String,
    },
    Opaque {
        value_type: ValueType,
        reason: SchemaOpacity,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaField {
    pub schema: ValueSchema,
    pub required: bool,
}

impl ValueSchema {
    pub const MAX_DEPTH: usize = MAX_SCHEMA_DEPTH;
    pub const MAX_NODES: usize = MAX_SCHEMA_NODES * 2;
    pub const MAX_TEXT_BYTES: usize = MAX_SCHEMA_TEXT_BYTES;

    pub fn opaque(value_type: ValueType) -> Self {
        Self::Opaque {
            value_type,
            reason: SchemaOpacity::Codec,
        }
    }

    pub fn described(self, description: impl Into<String>) -> Self {
        let description = description.into();
        if description.is_empty() {
            self
        } else {
            Self::Described {
                schema: Box::new(self),
                description,
            }
        }
    }

    pub fn object(ports: Vec<PortSpec>, mut schemas: BTreeMap<String, Self>) -> Self {
        Self::Object(
            ports
                .into_iter()
                .map(|port| {
                    let schema = schemas
                        .remove(port.name.as_ref())
                        .unwrap_or_else(|| Self::opaque(port.value_type));
                    (
                        port.name.to_string(),
                        SchemaField {
                            schema,
                            required: port.required,
                        },
                    )
                })
                .collect(),
        )
    }

    pub fn list(self) -> Self {
        match self {
            Self::Type(inner) => Self::Type(ValueType::List(Box::new(inner))),
            inner => Self::List(Box::new(inner)),
        }
    }
    pub fn map(self) -> Self {
        match self {
            Self::Type(inner) => Self::Type(ValueType::Map(Box::new(inner))),
            inner => Self::Map(Box::new(inner)),
        }
    }
    pub fn nullable(self) -> Self {
        match self {
            Self::Type(inner) => Self::Type(ValueType::Nullable(Box::new(inner))),
            inner => Self::Nullable(Box::new(inner)),
        }
    }

    pub fn value_type(&self) -> ValueType {
        match self {
            Self::Type(value_type) | Self::Opaque { value_type, .. } => value_type.clone(),
            Self::Object(_) | Self::Tagged { .. } => ValueType::Object,
            Self::Enum(_) => ValueType::String,
            Self::List(inner) => ValueType::List(Box::new(inner.value_type())),
            Self::Map(inner) => ValueType::Map(Box::new(inner.value_type())),
            Self::Nullable(inner) => ValueType::Nullable(Box::new(inner.value_type())),
            Self::Described { schema, .. } => schema.value_type(),
        }
    }

    pub fn is_complete(&self) -> bool {
        match self {
            Self::Opaque { .. } => false,
            Self::Type(_) | Self::Enum(_) => true,
            Self::List(inner) | Self::Map(inner) | Self::Nullable(inner) => inner.is_complete(),
            Self::Described { schema, .. } => schema.is_complete(),
            Self::Object(fields) => fields.values().all(|field| field.schema.is_complete()),
            Self::Tagged { variants, .. } => variants
                .values()
                .flatten()
                .all(|(_, field)| field.schema.is_complete()),
        }
    }

    /// Checks known decoded constraints. Opaque portions retain only their coarse type check.
    /// Consumers requiring a complete contract must also inspect `is_complete()`.
    pub fn validate_shared(&self, value: &ValueRef) -> Result<(), TypeMismatch> {
        match self {
            Self::Type(value_type) | Self::Opaque { value_type, .. } => {
                value_type.validate_shared(value)
            }
            Self::Described { schema, .. } => schema.validate_shared(value),
            Self::Nullable(_) if value.is_null() => Ok(()),
            Self::Nullable(inner) => inner.validate_shared(value),
            Self::Enum(variants) => {
                ValueType::String.validate_shared(value)?;
                if variants
                    .iter()
                    .any(|variant| Some(variant.as_str()) == value.as_str())
                {
                    Ok(())
                } else {
                    crate::unknown_enum_variant()
                }
            }
            Self::Object(fields) => crate::decode_object_value(value.clone(), |mut inputs| {
                validate_fields(fields, &mut inputs)
            }),
            Self::Tagged { tag, variants } => {
                crate::decode_object_value(value.clone(), |mut inputs| {
                    let name: String = crate::decode_input(&mut inputs, tag)?;
                    let Some(fields) = variants.get(&name) else {
                        return crate::unknown_enum_tag(tag);
                    };
                    validate_fields(fields, &mut inputs)
                })
            }
            Self::List(inner) => {
                ValueType::Array.validate_shared(value)?;
                for (index, item) in value.as_array().unwrap().iter().enumerate() {
                    inner.validate_shared(item).map_err(|mut error| {
                        error.path = format!("/{index}{}", error.path);
                        error
                    })?;
                }
                Ok(())
            }
            Self::Map(inner) => {
                ValueType::Object.validate_shared(value)?;
                for (key, item) in value.as_object().unwrap().iter() {
                    inner.validate_shared(item).map_err(|mut error| {
                        error.path = format!(
                            "/{}{}",
                            key.replace('~', "~0").replace('/', "~1"),
                            error.path
                        );
                        error
                    })?;
                }
                Ok(())
            }
        }
    }
}

fn validate_fields(
    fields: &BTreeMap<String, SchemaField>,
    inputs: &mut crate::Inputs,
) -> Result<(), InputDecodeError> {
    for (name, field) in fields {
        match inputs.remove(name) {
            Some(value) => field
                .schema
                .validate_shared(&value)
                .context(crate::typed_inputs::InvalidValueSnafu { port: name })?,
            None if field.required => {
                crate::decode_required_input::<ValueRef>(name, None)?;
            }
            None => {}
        }
    }
    crate::reject_unknown_inputs(std::mem::take(inputs))
}

/// Bounded, per-reflection traversal. Recursive or unknown codecs remain explicitly opaque.
pub struct ValueSchemaContext {
    active: Vec<&'static str>,
    remaining: usize,
}
impl Default for ValueSchemaContext {
    fn default() -> Self {
        Self {
            active: vec![],
            remaining: MAX_SCHEMA_NODES,
        }
    }
}
struct Visit<'a>(&'a mut ValueSchemaContext);
impl Drop for Visit<'_> {
    fn drop(&mut self) {
        self.0.active.pop();
    }
}
impl ValueSchemaContext {
    pub fn describe<T: InputValue>(&mut self) -> ValueSchema {
        let name = std::any::type_name::<T>();
        let reason = if self.active.contains(&name) {
            Some(SchemaOpacity::Recursive)
        } else if self.active.len() >= ValueType::MAX_DEPTH || self.remaining == 0 {
            Some(SchemaOpacity::Limit)
        } else {
            None
        };
        if let Some(reason) = reason {
            return ValueSchema::Opaque {
                value_type: T::value_type(),
                reason,
            };
        }
        self.remaining -= 1;
        self.active.push(name);
        let visit = Visit(self);
        T::value_schema(visit.0)
    }
}

#[derive(Debug, Snafu)]
pub enum SchemaContractError {
    #[snafu(display("schema names unknown input `{port}`"))]
    UnknownPort { port: String },
    #[snafu(display("input `{port}` schema is incompatible with its declared type"))]
    Incompatible { port: String },
    #[snafu(display("input `{port}` schema: {source}"))]
    TypeDepth {
        port: String,
        source: crate::TypeDepthError,
    },
    #[snafu(display("input schema exceeds its {resource} limit"))]
    Limit { resource: &'static str },
    #[snafu(display("input schema contains a duplicate enum value or conflicting enum tag"))]
    EnumConflict,
}

struct Budget {
    nodes: usize,
    text: usize,
}
impl Budget {
    fn text(&mut self, text: &str) -> Result<(), SchemaContractError> {
        ensure!(text.len() <= self.text, LimitSnafu { resource: "text" });
        self.text -= text.len();
        Ok(())
    }
    fn schema(
        &mut self,
        schema: &ValueSchema,
        port: &str,
        depth: usize,
    ) -> Result<(), SchemaContractError> {
        ensure!(
            depth <= MAX_SCHEMA_DEPTH && self.nodes > 0,
            LimitSnafu {
                resource: "depth or node count"
            }
        );
        self.nodes -= 1;
        match schema {
            ValueSchema::Type(value_type) | ValueSchema::Opaque { value_type, .. } => {
                value_type.check_depth().context(TypeDepthSnafu { port })?
            }
            ValueSchema::Described {
                schema,
                description,
            } => {
                self.text(description)?;
                self.schema(schema, port, depth + 1)?;
            }
            ValueSchema::List(inner) | ValueSchema::Map(inner) | ValueSchema::Nullable(inner) => {
                self.schema(inner, port, depth + 1)?
            }
            ValueSchema::Object(fields) => self.fields(fields, port, depth)?,
            ValueSchema::Enum(variants) => {
                ensure!(
                    variants.iter().collect::<BTreeSet<_>>().len() == variants.len(),
                    EnumConflictSnafu
                );
                for variant in variants {
                    self.text(variant)?;
                }
            }
            ValueSchema::Tagged { tag, variants } => {
                self.text(tag)?;
                for (name, fields) in variants {
                    self.text(name)?;
                    ensure!(!fields.contains_key(tag), EnumConflictSnafu);
                    self.fields(fields, port, depth)?;
                }
            }
        }
        Ok(())
    }
    fn fields(
        &mut self,
        fields: &BTreeMap<String, SchemaField>,
        port: &str,
        depth: usize,
    ) -> Result<(), SchemaContractError> {
        for (name, field) in fields {
            self.text(name)?;
            self.schema(&field.schema, port, depth + 1)?;
        }
        Ok(())
    }
}

pub fn validate_port_schemas(
    ports: &[PortSpec],
    schemas: &BTreeMap<String, ValueSchema>,
) -> Result<(), SchemaContractError> {
    let mut budget = Budget {
        nodes: ValueSchema::MAX_NODES,
        text: MAX_SCHEMA_TEXT_BYTES,
    };
    for (name, schema) in schemas {
        use snafu::OptionExt;
        let port = ports
            .iter()
            .find(|port| port.name == *name)
            .context(UnknownPortSnafu { port: name })?;
        budget.text(name)?;
        budget.schema(schema, name, 1)?;
        let value_type = schema.value_type();
        value_type
            .check_depth()
            .context(TypeDepthSnafu { port: name })?;
        ensure!(
            value_type.compatibility_with(&port.value_type)
                != crate::TypeCompatibility::Incompatible,
            IncompatibleSnafu { port: name }
        );
    }
    Ok(())
}
