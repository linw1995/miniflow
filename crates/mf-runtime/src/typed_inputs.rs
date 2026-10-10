use crate::{Inputs, PortSpec, TypeDepthError, TypeMismatch, ValueRef, ValueType};
use snafu::{OptionExt, ResultExt, Snafu};
use std::collections::BTreeMap;

/// One authoritative declaration and decoder for a task's owned input fields.
///
/// Implementations must accept exactly the names, requiredness, and JSON types
/// reported by `ports()`, reject unknown names, and preserve shared JSON payloads.
///
/// ```
/// use mf_runtime::{Inputs, NodeInputs, ValueRef};
/// use std::collections::BTreeMap;
///
/// #[derive(NodeInputs)]
/// struct RequestInputs {
///     url: String,
///     headers: Option<BTreeMap<String, String>>,
///     body: Option<ValueRef>,
///     #[input(rename = "request.path")]
///     path: Option<String>,
/// }
///
/// let ports = RequestInputs::ports();
/// let request = RequestInputs::from_inputs(Inputs::from([
///     ("url".into(), "https://example.test".into()),
/// ]))?;
/// assert_eq!(ports[3].name, "request.path");
/// assert_eq!(request.url, "https://example.test");
/// assert!(request.headers.is_none() && request.body.is_none() && request.path.is_none());
/// # Ok::<(), mf_runtime::InputDecodeError>(())
/// ```
pub trait NodeInputs: Sized {
    fn ports() -> Vec<PortSpec>;

    fn from_inputs(inputs: Inputs) -> Result<Self, InputDecodeError>;
}

/// Decodes one supplied JSON value without coercion or an intermediate JSON tree.
///
/// The decoder must agree with `value_type()` and report paths relative to the
/// supplied value. Supported implementations are bool, i64, u64, usize, f32, f64, String, ValueRef,
/// Vec of supported values, string-keyed BTreeMap of supported values, and
/// objects deriving [`crate::NodeValue`].
/// `Option` is intentionally only an [`InputField`], not an element value codec.
pub trait InputValue: Sized {
    fn value_type() -> ValueType;

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch>;
}

/// Adds presence semantics to a top-level input value.
///
/// Ordinary supported values are required. `Option<T>` uses T's descriptor and
/// permits omission, without making T nullable. Manual implementations must keep
/// their descriptor, requiredness, and decoding behavior consistent.
pub trait InputField: Sized {
    const REQUIRED: bool;

    fn value_type() -> ValueType;

    fn decode_field(port: &str, value: Option<ValueRef>) -> Result<Self, InputDecodeError>;

    fn port(name: &'static str) -> PortSpec {
        PortSpec::new(name, Self::value_type(), Self::REQUIRED)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Snafu)]
pub enum InputDecodeError {
    #[snafu(display("required input `{port}` was not provided"))]
    MissingField { port: String },
    #[snafu(display("unknown input `{port}`"))]
    UnknownField { port: String },
    #[snafu(display("input `{port}`: {source}"))]
    InvalidValue { port: String, source: TypeMismatch },
    #[snafu(display("input `{port}` has an invalid descriptor: {source}"))]
    InvalidType {
        port: String,
        source: TypeDepthError,
    },
}

impl InputDecodeError {
    /// Returns the failing path relative to the complete node input object.
    pub fn pointer(&self) -> String {
        let (port, path) = match self {
            Self::InvalidValue { port, source } => (port, source.path.as_str()),
            Self::MissingField { port }
            | Self::UnknownField { port }
            | Self::InvalidType { port, .. } => (port, ""),
        };
        format!("/{}{path}", escape_pointer(port))
    }
}

/// Consumes one declared input using its strict field codec.
///
/// ```
/// use mf_runtime::{Inputs, ValueRef, decode_input, reject_unknown_inputs};
///
/// let payload = ValueRef::from("shared");
/// let mut inputs = Inputs::from([("payload".into(), payload.clone())]);
/// let decoded: ValueRef = decode_input(&mut inputs, "payload")?;
/// let path: Option<String> = decode_input(&mut inputs, "path")?;
/// reject_unknown_inputs(inputs)?;
/// assert!(decoded.ptr_eq(&payload));
/// assert!(path.is_none());
/// # Ok::<(), mf_runtime::InputDecodeError>(())
/// ```
pub fn decode_input<T: InputField>(inputs: &mut Inputs, port: &str) -> Result<T, InputDecodeError> {
    T::value_type()
        .check_depth()
        .context(InvalidTypeSnafu { port })?;
    T::decode_field(port, inputs.remove(port))
}

/// Uses a field default only when the binding is absent; supplied values stay strict.
pub fn decode_default_input<T: InputField>(
    inputs: &mut Inputs,
    port: &str,
    default: impl FnOnce() -> T,
) -> Result<T, InputDecodeError> {
    T::value_type()
        .check_depth()
        .context(InvalidTypeSnafu { port })?;
    match inputs.remove(port) {
        Some(value) => T::decode_field(port, Some(value)),
        None => Ok(default()),
    }
}

/// Rejects any bindings left after the declared input fields have been consumed.
pub fn reject_unknown_inputs(inputs: Inputs) -> Result<(), InputDecodeError> {
    if let Some((port, _)) = inputs.into_iter().next() {
        return UnknownFieldSnafu { port }.fail();
    }
    Ok(())
}

/// Decodes a required field using its single-value codec.
pub fn decode_required_input<T: InputValue>(
    port: &str,
    value: Option<ValueRef>,
) -> Result<T, InputDecodeError> {
    let value = value.context(MissingFieldSnafu { port })?;
    T::decode(value).context(InvalidValueSnafu { port })
}

macro_rules! required_field {
    ($ty:ty $(, $param:ident)?) => {
        impl$(<$param: InputValue>)? InputField for $ty {
            const REQUIRED: bool = true;

            fn value_type() -> ValueType {
                <Self as InputValue>::value_type()
            }

            fn decode_field(port: &str, value: Option<ValueRef>) -> Result<Self, InputDecodeError> {
                decode_required_input(port, value)
            }
        }
    };
}

/// Decodes a named-port contract from a shared JSON object.
pub fn decode_node_value<T: crate::NodeValue>(value: ValueRef) -> Result<T, TypeMismatch> {
    decode_object_value(value, T::from_values)
}

/// Decodes an object with a generated strict field decoder, retaining error sources.
pub fn decode_object_value<T>(
    value: ValueRef,
    decode: impl FnOnce(Inputs) -> Result<T, InputDecodeError>,
) -> Result<T, TypeMismatch> {
    ValueType::Object.validate_shared(&value)?;
    let values = value
        .as_object()
        .expect("validated object")
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect();
    decode(values).with_context(|error| {
        let (expected, actual) = match error {
            InputDecodeError::InvalidValue { source, .. } => {
                (source.expected.clone(), source.actual)
            }
            InputDecodeError::MissingField { .. } => (ValueType::Object, "missing field"),
            InputDecodeError::InvalidType { .. } => (ValueType::Object, "invalid descriptor"),
            InputDecodeError::UnknownField { .. } => (ValueType::Object, "unknown field"),
        };
        crate::node::InputSnafu {
            details: crate::TypeMismatchDetails {
                path: error.pointer(),
                expected,
                actual,
            },
        }
    })
}

/// Reports a wire name outside a generated enum's closed variant set.
pub fn unknown_enum_variant<T>() -> Result<T, TypeMismatch> {
    crate::node::ValueSnafu {
        details: crate::TypeMismatchDetails {
            path: String::new(),
            expected: ValueType::String,
            actual: "unknown enum variant",
        },
    }
    .fail()
}

/// Attaches the tag's field context to an unknown enum variant.
pub fn unknown_enum_tag<T>(tag: &str) -> Result<T, InputDecodeError> {
    unknown_enum_variant().context(InvalidValueSnafu { port: tag })
}

macro_rules! scalar_value {
    ($ty:ty, $descriptor:ident, $method:ident) => {
        impl InputValue for $ty {
            fn value_type() -> ValueType {
                ValueType::$descriptor
            }

            fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
                ValueType::$descriptor.validate_shared(&value)?;
                Ok(value.$method().expect("validated scalar"))
            }
        }
        required_field!($ty);
    };
}

scalar_value!(bool, Boolean, as_bool);
scalar_value!(i64, Int64, as_i64);
scalar_value!(u64, Uint64, as_u64);
scalar_value!(f64, Float64, as_f64);

impl InputValue for usize {
    fn value_type() -> ValueType {
        ValueType::Usize
    }

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        ValueType::Usize.validate_shared(&value)?;
        Ok(usize::try_from(value.as_u64().expect("validated integer")).expect("validated range"))
    }
}
required_field!(usize);

impl InputValue for f32 {
    fn value_type() -> ValueType {
        ValueType::Float32
    }

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        ValueType::Float32.validate_shared(&value)?;
        Ok(value.as_f64().expect("validated float") as f32)
    }
}
required_field!(f32);

/// A supplied null or a supplied value. Use `Option<Nullable<T>>` for three-state fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Nullable<T> {
    Null,
    Value(T),
}

impl<T: InputValue> InputValue for Nullable<T> {
    fn value_type() -> ValueType {
        ValueType::Nullable(Box::new(T::value_type()))
    }

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        if value.is_null() {
            Ok(Self::Null)
        } else {
            T::decode(value).map(Self::Value)
        }
    }
}
required_field!(Nullable<T>, T);

/// A typed view of a shared JSON payload that defers owned decoding.
///
/// Construction validates the wire descriptor without constructing `T`. Broad
/// object/enum descriptors do not replace their strict codecs: `decode()` can
/// still reject a payload. Inspect `value()` and apply a budget before decoding.
#[derive(Debug)]
pub struct Shared<T> {
    value: ValueRef,
    marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            marker: std::marker::PhantomData,
        }
    }
}

impl<T> Shared<T> {
    pub fn value(&self) -> &ValueRef {
        &self.value
    }

    pub fn into_value(self) -> ValueRef {
        self.value
    }
}

impl<T: InputValue> Shared<T> {
    pub fn new(value: ValueRef) -> Result<Self, TypeMismatch> {
        T::value_type().validate_shared(&value)?;
        Ok(Self {
            value,
            marker: std::marker::PhantomData,
        })
    }

    pub fn decode(&self) -> Result<T, TypeMismatch> {
        T::decode(self.value.clone())
    }
}

impl<T: InputValue> InputValue for Shared<T> {
    fn value_type() -> ValueType {
        T::value_type()
    }

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        Self::new(value)
    }
}
required_field!(Shared<T>, T);

impl InputValue for String {
    fn value_type() -> ValueType {
        ValueType::String
    }

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        ValueType::String.validate_shared(&value)?;
        Ok(value.as_str().expect("validated string").to_owned())
    }
}
required_field!(String);

impl InputValue for ValueRef {
    fn value_type() -> ValueType {
        ValueType::Any
    }

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        Ok(value)
    }
}
required_field!(ValueRef);

impl<T: InputValue> InputValue for Vec<T> {
    fn value_type() -> ValueType {
        ValueType::List(Box::new(T::value_type()))
    }

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        if !value.is_array() {
            <Self as InputValue>::value_type().validate_shared(&value)?;
        }
        value
            .as_array()
            .expect("validated list")
            .iter()
            .enumerate()
            .map(|(index, item)| {
                T::decode(item.clone()).map_err(|mut error| {
                    error.path = format!("/{index}{}", error.path);
                    error
                })
            })
            .collect()
    }
}
required_field!(Vec<T>, T);

impl<T: InputValue> InputValue for BTreeMap<String, T> {
    fn value_type() -> ValueType {
        ValueType::Map(Box::new(T::value_type()))
    }

    fn decode(value: ValueRef) -> Result<Self, TypeMismatch> {
        if !value.is_object() {
            <Self as InputValue>::value_type().validate_shared(&value)?;
        }
        value
            .as_object()
            .expect("validated map")
            .iter()
            .map(|(key, item)| {
                let decoded = T::decode(item.clone()).map_err(|mut error| {
                    error.path = format!("/{}{}", escape_pointer(key), error.path);
                    error
                })?;
                Ok((key.to_string(), decoded))
            })
            .collect()
    }
}
required_field!(BTreeMap<String, T>, T);

impl<T: InputValue> InputField for Option<T> {
    const REQUIRED: bool = false;

    fn value_type() -> ValueType {
        T::value_type()
    }

    fn decode_field(port: &str, value: Option<ValueRef>) -> Result<Self, InputDecodeError> {
        value
            .map(|value| T::decode(value).context(InvalidValueSnafu { port }))
            .transpose()
    }
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::error::Error;

    fn decode<T: InputField>(value: Option<serde_json::Value>) -> Result<T, InputDecodeError> {
        let mut inputs = value
            .map(|value| Inputs::from([("input".into(), value.into())]))
            .unwrap_or_default();
        let decoded = decode_input(&mut inputs, "input")?;
        reject_unknown_inputs(inputs)?;
        Ok(decoded)
    }

    #[test]
    fn codecs_agree_with_descriptors_and_numeric_representations() {
        assert!(decode::<bool>(Some(json!(true))).unwrap());
        assert_eq!(decode::<i64>(Some(json!(i64::MIN))).unwrap(), i64::MIN);
        assert_eq!(decode::<f64>(Some(json!(1.5))).unwrap(), 1.5);
        assert_eq!(decode::<String>(Some(json!("text"))).unwrap(), "text");
        assert_eq!(decode::<ValueRef>(Some(json!(null))).unwrap(), json!(null));
        assert_eq!(
            <bool as InputField>::port("flag").value_type,
            ValueType::Boolean
        );
        assert_eq!(
            <i64 as InputField>::port("count").value_type,
            ValueType::Int64
        );
        assert_eq!(
            <f64 as InputField>::port("ratio").value_type,
            ValueType::Float64
        );
        assert_eq!(
            <ValueRef as InputField>::port("data").value_type,
            ValueType::Any
        );
        for value in [json!(1.5), json!(u64::MAX), json!("1"), json!(null)] {
            assert!(decode::<i64>(Some(value)).is_err());
        }
        for value in [json!(1), json!("1.5"), json!(null)] {
            assert!(decode::<f64>(Some(value)).is_err());
        }
        assert!(decode::<bool>(Some(json!(1))).is_err());
        assert!(decode::<String>(Some(json!(false))).is_err());
        let negative_zero = decode::<f64>(Some(json!(-0.0))).unwrap();
        assert_eq!(negative_zero.to_bits(), (-0.0_f64).to_bits());
    }

    #[test]
    fn omission_is_separate_from_supplied_null() {
        type Path = Option<String>;
        assert!(!Path::port("path").required);
        assert_eq!(Path::port("path").value_type, ValueType::String);
        assert_eq!(decode::<Path>(None).unwrap(), None);
        assert_eq!(
            decode::<Path>(Some(json!("file"))).unwrap(),
            Some("file".into())
        );
        assert!(decode::<Path>(Some(json!(null))).is_err());
        assert!(decode::<Option<ValueRef>>(None).unwrap().is_none());
        assert!(
            decode::<Option<ValueRef>>(Some(json!(null)))
                .unwrap()
                .unwrap()
                .is_null()
        );
        assert!(
            matches!(decode::<String>(None), Err(InputDecodeError::MissingField { port }) if port == "input")
        );
    }

    #[test]
    fn unknown_names_and_escaped_paths_are_reported() {
        let inputs = Inputs::from([("a/b~c".into(), true.into())]);
        let error = reject_unknown_inputs(inputs).unwrap_err();
        assert_eq!(error.pointer(), "/a~1b~0c");
        assert!(matches!(error, InputDecodeError::UnknownField { port } if port == "a/b~c"));
        let mut inputs = Inputs::new();
        assert_eq!(
            decode_input::<i64>(&mut inputs, "a/b~c")
                .unwrap_err()
                .pointer(),
            "/a~1b~0c"
        );
    }

    #[test]
    fn recursive_collections_preserve_typed_mismatch_sources() {
        type Rows = Vec<BTreeMap<String, i64>>;
        assert_eq!(
            Rows::port("rows").value_type,
            ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))))
        );
        assert!(Rows::port("rows").required);
        let rows = decode::<Rows>(Some(json!([{"count": 1}, {}]))).unwrap();
        assert_eq!(rows[0]["count"], 1);
        assert!(rows[1].is_empty());
        assert!(decode::<Rows>(Some(json!({}))).is_err());
        assert!(decode::<BTreeMap<String, i64>>(Some(json!([]))).is_err());
        let mut inputs = Inputs::from([(
            "a/b~c".into(),
            json!([{"count": 1}, {"x/y~z": "two"}]).into(),
        )]);
        let error = decode_input::<Rows>(&mut inputs, "a/b~c").unwrap_err();
        assert_eq!(error.pointer(), "/a~1b~0c/1/x~1y~0z");
        let source = error
            .source()
            .unwrap()
            .downcast_ref::<TypeMismatch>()
            .unwrap();
        assert_eq!(source.path, "/1/x~1y~0z");
        assert_eq!(source.expected, ValueType::Int64);
        assert_eq!(source.actual, "string");
    }

    #[test]
    fn raw_and_nested_values_keep_payload_identity() {
        let root = ValueRef::from(json!({"items": [{"value": [1, 2]}, null]}));
        let mut inputs = Inputs::from([
            ("root".into(), root.clone()),
            ("items".into(), root["items"].clone()),
            ("map".into(), root.clone()),
        ]);
        let decoded: ValueRef = decode_input(&mut inputs, "root").unwrap();
        assert!(decoded.ptr_eq(&root));
        let items: Vec<ValueRef> = decode_input(&mut inputs, "items").unwrap();
        assert!(items[0].ptr_eq(&root["items"][0]));
        assert!(items[1].ptr_eq(&root["items"][1]));
        let map: BTreeMap<String, Vec<ValueRef>> = decode_input(&mut inputs, "map").unwrap();
        assert!(map["items"][0].ptr_eq(&root["items"][0]));
        reject_unknown_inputs(inputs).unwrap();
    }

    #[test]
    fn decoding_rejects_descriptors_over_the_shared_depth_limit() {
        type L1 = Vec<ValueRef>;
        type L2 = Vec<L1>;
        type L4 = Vec<Vec<L2>>;
        type L8 = Vec<Vec<Vec<Vec<L4>>>>;
        type L16 = Vec<Vec<Vec<Vec<Vec<Vec<Vec<Vec<L8>>>>>>>>;
        let mut inputs = Inputs::new();
        let error = decode_input::<L16>(&mut inputs, "deep").unwrap_err();
        assert_eq!(error.pointer(), "/deep");
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<TypeDepthError>()
                .unwrap()
                .depth,
            ValueType::MAX_DEPTH + 1
        );
        assert!(L8::port("items").value_type.check_depth().is_ok());
    }
}
