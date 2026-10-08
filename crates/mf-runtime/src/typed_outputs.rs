use crate::{Outputs, PortSpec, TypeDepthError, TypeMismatch, ValueKind, ValueRef, ValueType};
use snafu::{OptionExt, ResultExt, Snafu};
use std::collections::BTreeMap;

/// One authoritative declaration and encoder for a task's owned output fields.
///
/// Implementations must emit exactly the names, requiredness, and JSON types
/// reported by `ports()`. Shared JSON payloads must remain shared.
///
/// ```
/// use mf_runtime::{NodeOutputs, ValueRef, ValueType};
///
/// #[derive(NodeOutputs)]
/// struct Response {
///     status: i64,
///     body: ValueRef,
///     #[output(rename = "response.label")]
///     label: Option<String>,
/// }
///
/// let ports = Response::ports();
/// let outputs = Response {
///     status: 200,
///     body: ValueRef::null(),
///     label: None,
/// }.into_outputs()?;
/// assert_eq!(ports[0].value_type, ValueType::Int64);
/// assert!(!ports[2].required);
/// assert!(outputs["body"].is_null());
/// assert!(!outputs.contains_key("response.label"));
/// # Ok::<(), mf_runtime::OutputEncodeError>(())
/// ```
///
pub trait NodeOutputs: Sized {
    fn ports() -> Vec<PortSpec>;

    fn into_outputs(self) -> Result<Outputs, OutputEncodeError>;
}

/// Encodes a JSON value without coercion or an intermediate JSON tree.
///
/// Supported implementations mirror `InputValue`. `Option` is only an
/// `OutputField`: omission is distinct from a supplied JSON null.
pub trait OutputValue: Sized {
    fn value_type() -> ValueType;

    fn encode(self) -> Result<ValueRef, TypeMismatch>;
}

/// Adds presence semantics to a top-level output value.
pub trait OutputField: Sized {
    const REQUIRED: bool;

    fn value_type() -> ValueType;

    fn encode_field(self) -> Result<Option<ValueRef>, TypeMismatch>;

    fn port(name: &'static str) -> PortSpec {
        PortSpec::new(name, Self::value_type(), Self::REQUIRED)
    }
}

#[derive(Debug, Snafu)]
pub enum OutputEncodeError {
    #[snafu(display("output `{port}`: {source}"))]
    InvalidValue { port: String, source: TypeMismatch },
    #[snafu(display("output `{port}` has an invalid descriptor: {source}"))]
    InvalidType {
        port: String,
        source: TypeDepthError,
    },
}

impl OutputEncodeError {
    /// Returns the failing path relative to the complete node output object.
    pub fn pointer(&self) -> String {
        let (port, path) = match self {
            Self::InvalidValue { port, source } => (port, source.path.as_str()),
            Self::InvalidType { port, .. } => (port, ""),
        };
        format!("/{}{path}", escape_pointer(port))
    }
}

/// Encodes one field, omitting `None` and preserving `Some(ValueRef::null())`.
pub fn encode_output<T: OutputField>(
    outputs: &mut Outputs,
    port: &str,
    value: T,
) -> Result<(), OutputEncodeError> {
    T::value_type()
        .check_depth()
        .context(InvalidTypeSnafu { port })?;
    if let Some(value) = value.encode_field().context(InvalidValueSnafu { port })? {
        outputs.insert(port.to_owned(), value);
    }
    Ok(())
}

impl<T: OutputValue> OutputField for T {
    const REQUIRED: bool = true;

    fn value_type() -> ValueType {
        <Self as OutputValue>::value_type()
    }

    fn encode_field(self) -> Result<Option<ValueRef>, TypeMismatch> {
        self.encode().map(Some)
    }
}

macro_rules! scalar_value {
    ($ty:ty, $descriptor:ident) => {
        impl OutputValue for $ty {
            fn value_type() -> ValueType {
                ValueType::$descriptor
            }

            fn encode(self) -> Result<ValueRef, TypeMismatch> {
                Ok(self.into())
            }
        }
    };
}

scalar_value!(bool, Boolean);
scalar_value!(i64, Int64);
scalar_value!(String, String);
scalar_value!(ValueRef, Any);

impl OutputValue for f64 {
    fn value_type() -> ValueType {
        ValueType::Float64
    }

    fn encode(self) -> Result<ValueRef, TypeMismatch> {
        // JSON cannot represent non-finite floats; silently converting them to
        // null would violate the declared Float64 contract.
        let number =
            serde_json::Number::from_f64(self).context(crate::node::TypeMismatchSnafu {
                path: "",
                expected: ValueType::Float64,
                actual: "non-finite float",
            })?;
        Ok(ValueRef::new(ValueKind::Number(number)))
    }
}

impl<T: OutputValue> OutputValue for Vec<T> {
    fn value_type() -> ValueType {
        ValueType::List(Box::new(T::value_type()))
    }

    fn encode(self) -> Result<ValueRef, TypeMismatch> {
        self.into_iter()
            .enumerate()
            .map(|(index, item)| {
                item.encode().map_err(|mut error| {
                    error.path = format!("/{index}{}", error.path);
                    error
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(ValueRef::array)
    }
}

impl<T: OutputValue> OutputValue for BTreeMap<String, T> {
    fn value_type() -> ValueType {
        ValueType::Map(Box::new(T::value_type()))
    }

    fn encode(self) -> Result<ValueRef, TypeMismatch> {
        self.into_iter()
            .map(|(key, item)| {
                let value = item.encode().map_err(|mut error| {
                    error.path = format!("/{}{}", escape_pointer(&key), error.path);
                    error
                })?;
                Ok((key.into(), value))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(ValueRef::object)
    }
}

impl<T: OutputValue> OutputField for Option<T> {
    const REQUIRED: bool = false;

    fn value_type() -> ValueType {
        T::value_type()
    }

    fn encode_field(self) -> Result<Option<ValueRef>, TypeMismatch> {
        self.map(T::encode).transpose()
    }
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

/// Closed Rust representations for which direct transfer has a runtime-owned codec proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RustValueType {
    Boolean,
    Int64,
    Float64,
    String,
    Shared,
    List(Box<Self>),
    Map(Box<Self>),
    Optional(Box<Self>),
}

/// Seals certified codecs to the implementations whose validation is runtime-owned.
pub trait CertifiedCodec {}

pub trait TypedValueCodec: crate::InputValue + OutputValue + CertifiedCodec {
    fn rust_type() -> RustValueType;
    fn validate_typed(&self) -> Result<(), TypeMismatch>;
}

pub trait TypedField: crate::InputField + OutputField + CertifiedCodec {
    fn rust_type() -> RustValueType;
    fn validate_typed(&self) -> Result<bool, TypeMismatch>;
}

impl<T: TypedValueCodec + crate::InputField> TypedField for T {
    fn rust_type() -> RustValueType {
        <T as TypedValueCodec>::rust_type()
    }

    fn validate_typed(&self) -> Result<bool, TypeMismatch> {
        <T as TypedValueCodec>::validate_typed(self)?;
        Ok(true)
    }
}

macro_rules! certified_scalar {
    ($ty:ty, $kind:ident) => {
        impl CertifiedCodec for $ty {}
        impl TypedValueCodec for $ty {
            fn rust_type() -> RustValueType {
                RustValueType::$kind
            }
            fn validate_typed(&self) -> Result<(), TypeMismatch> {
                Ok(())
            }
        }
    };
}

certified_scalar!(bool, Boolean);
certified_scalar!(i64, Int64);
certified_scalar!(String, String);
certified_scalar!(ValueRef, Shared);

impl CertifiedCodec for f64 {}
impl TypedValueCodec for f64 {
    fn rust_type() -> RustValueType {
        RustValueType::Float64
    }

    fn validate_typed(&self) -> Result<(), TypeMismatch> {
        serde_json::Number::from_f64(*self).context(crate::node::TypeMismatchSnafu {
            path: "",
            expected: ValueType::Float64,
            actual: "non-finite float",
        })?;
        Ok(())
    }
}

impl<T: TypedValueCodec> CertifiedCodec for Vec<T> {}
impl<T: TypedValueCodec> TypedValueCodec for Vec<T> {
    fn rust_type() -> RustValueType {
        RustValueType::List(Box::new(T::rust_type()))
    }

    fn validate_typed(&self) -> Result<(), TypeMismatch> {
        for (index, value) in self.iter().enumerate() {
            value.validate_typed().map_err(|mut error| {
                error.path = format!("/{index}{}", error.path);
                error
            })?;
        }
        Ok(())
    }
}

impl<T: TypedValueCodec> CertifiedCodec for BTreeMap<String, T> {}
impl<T: TypedValueCodec> TypedValueCodec for BTreeMap<String, T> {
    fn rust_type() -> RustValueType {
        RustValueType::Map(Box::new(T::rust_type()))
    }

    fn validate_typed(&self) -> Result<(), TypeMismatch> {
        for (key, value) in self {
            value.validate_typed().map_err(|mut error| {
                error.path = format!("/{}{}", escape_pointer(key), error.path);
                error
            })?;
        }
        Ok(())
    }
}

impl<T: TypedValueCodec> CertifiedCodec for Option<T> {}
impl<T: TypedValueCodec> TypedField for Option<T> {
    fn rust_type() -> RustValueType {
        RustValueType::Optional(Box::new(T::rust_type()))
    }

    fn validate_typed(&self) -> Result<bool, TypeMismatch> {
        if let Some(value) = self {
            value.validate_typed()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Validates a field before generated execution makes any typed payload available.
pub fn validate_typed_output<T: TypedField>(
    value: &T,
    port: &'static str,
) -> Result<bool, OutputEncodeError> {
    <T as OutputField>::value_type()
        .check_depth()
        .context(InvalidTypeSnafu { port })?;
    value.validate_typed().context(InvalidValueSnafu { port })
}
