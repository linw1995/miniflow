use crate::{Inputs, PortSpec, TypeDepthError, TypeMismatch, ValueRef, ValueType};
use snafu::{OptionExt, ResultExt, Snafu};
use std::collections::BTreeMap;

/// One authoritative declaration and decoder for a task's owned input fields.
///
/// Implementations must accept exactly the names, requiredness, and JSON types
/// reported by `ports()`, reject unknown names, and preserve shared JSON payloads.
pub trait NodeInputs: Sized {
    fn ports() -> Vec<PortSpec>;

    fn from_inputs(inputs: Inputs) -> Result<Self, InputDecodeError>;
}

/// Decodes one supplied JSON value without coercion or an intermediate JSON tree.
///
/// The decoder must agree with `value_type()` and report paths relative to the
/// supplied value. Supported implementations are bool, i64, f64, String, ValueRef,
/// Vec of supported values, and string-keyed BTreeMap of supported values.
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

#[derive(Debug, Snafu)]
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

/// Consumes a node input map using the runtime's field codecs.
///
/// ```
/// use mf_runtime::{InputDecoder, Inputs, ValueRef};
///
/// let payload = ValueRef::from("shared");
/// let mut decoder = InputDecoder::new(Inputs::from([
///     ("payload".into(), payload.clone()),
/// ]));
/// let decoded: ValueRef = decoder.take("payload")?;
/// let path: Option<String> = decoder.take("path")?;
/// decoder.finish()?;
/// assert!(decoded.ptr_eq(&payload));
/// assert!(path.is_none());
/// # Ok::<(), mf_runtime::InputDecodeError>(())
/// ```
pub struct InputDecoder {
    inputs: Inputs,
}

impl InputDecoder {
    pub fn new(inputs: Inputs) -> Self {
        Self { inputs }
    }

    pub fn take<T: InputField>(&mut self, port: &str) -> Result<T, InputDecodeError> {
        T::value_type()
            .check_depth()
            .context(InvalidTypeSnafu { port })?;
        T::decode_field(port, self.inputs.remove(port))
    }

    pub fn finish(self) -> Result<(), InputDecodeError> {
        if let Some((port, _)) = self.inputs.into_iter().next() {
            return UnknownFieldSnafu { port }.fail();
        }
        Ok(())
    }
}

fn decode_required<T: InputValue>(
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
                decode_required(port, value)
            }
        }
    };
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
scalar_value!(f64, Float64, as_f64);

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
        <Self as InputValue>::value_type().validate_shared(&value)?;
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
        <Self as InputValue>::value_type().validate_shared(&value)?;
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
        let inputs = value
            .map(|value| Inputs::from([("input".into(), value.into())]))
            .unwrap_or_default();
        let mut decoder = InputDecoder::new(inputs);
        let decoded = decoder.take("input")?;
        decoder.finish()?;
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
        let decoder = InputDecoder::new(Inputs::from([("a/b~c".into(), true.into())]));
        let error = decoder.finish().unwrap_err();
        assert_eq!(error.pointer(), "/a~1b~0c");
        assert!(matches!(error, InputDecodeError::UnknownField { port } if port == "a/b~c"));
        let mut decoder = InputDecoder::new(Inputs::new());
        assert_eq!(
            decoder.take::<i64>("a/b~c").unwrap_err().pointer(),
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
        let mut decoder = InputDecoder::new(Inputs::from([(
            "a/b~c".into(),
            json!([{"count": 1}, {"x/y~z": "two"}]).into(),
        )]));
        let error = decoder.take::<Rows>("a/b~c").unwrap_err();
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
        let mut decoder = InputDecoder::new(Inputs::from([
            ("root".into(), root.clone()),
            ("items".into(), root["items"].clone()),
            ("map".into(), root.clone()),
        ]));
        let decoded: ValueRef = decoder.take("root").unwrap();
        assert!(decoded.ptr_eq(&root));
        let items: Vec<ValueRef> = decoder.take("items").unwrap();
        assert!(items[0].ptr_eq(&root["items"][0]));
        assert!(items[1].ptr_eq(&root["items"][1]));
        let map: BTreeMap<String, Vec<ValueRef>> = decoder.take("map").unwrap();
        assert!(map["items"][0].ptr_eq(&root["items"][0]));
        decoder.finish().unwrap();
    }

    #[test]
    fn decoding_rejects_descriptors_over_the_shared_depth_limit() {
        type L1 = Vec<ValueRef>;
        type L2 = Vec<L1>;
        type L4 = Vec<Vec<L2>>;
        type L8 = Vec<Vec<Vec<Vec<L4>>>>;
        type L16 = Vec<Vec<Vec<Vec<Vec<Vec<Vec<Vec<L8>>>>>>>>;
        let mut decoder = InputDecoder::new(Inputs::new());
        let error = decoder.take::<L16>("deep").unwrap_err();
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
