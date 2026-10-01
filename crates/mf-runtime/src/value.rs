use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::{Number, Value};
use std::{
    collections::BTreeMap,
    fmt,
    ops::Index,
    sync::{Arc, LazyLock},
};

#[derive(Clone, Debug)]
pub enum ValueKind {
    Null,
    Bool(bool),
    Number(Number),
    String(Arc<str>),
    Array(Vec<ValueRef>),
    Object(BTreeMap<Arc<str>, ValueRef>),
}

/// An immutable JSON value whose descendants are shared between versions.
#[derive(Clone, Debug)]
pub struct ValueRef(Arc<ValueKind>);

impl ValueRef {
    pub fn new(kind: ValueKind) -> Self {
        Self(Arc::new(kind))
    }
    pub fn null() -> Self {
        Self::new(ValueKind::Null)
    }
    pub fn array(items: impl IntoIterator<Item = Self>) -> Self {
        Self::new(ValueKind::Array(items.into_iter().collect()))
    }
    pub fn object(entries: impl IntoIterator<Item = (Arc<str>, Self)>) -> Self {
        Self::new(ValueKind::Object(entries.into_iter().collect()))
    }
    pub fn kind(&self) -> &ValueKind {
        &self.0
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn is_null(&self) -> bool {
        matches!(self.kind(), ValueKind::Null)
    }
    pub fn is_boolean(&self) -> bool {
        matches!(self.kind(), ValueKind::Bool(_))
    }
    pub fn is_number(&self) -> bool {
        self.as_number().is_some()
    }
    pub fn is_string(&self) -> bool {
        self.as_str().is_some()
    }
    pub fn is_array(&self) -> bool {
        self.as_array().is_some()
    }
    pub fn is_object(&self) -> bool {
        self.as_object().is_some()
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let ValueKind::Bool(value) = self.kind() {
            Some(*value)
        } else {
            None
        }
    }
    pub fn as_number(&self) -> Option<&Number> {
        if let ValueKind::Number(value) = self.kind() {
            Some(value)
        } else {
            None
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().and_then(Number::as_i64)
    }
    pub fn as_u64(&self) -> Option<u64> {
        self.as_number().and_then(Number::as_u64)
    }
    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().and_then(Number::as_f64)
    }
    pub fn is_f64(&self) -> bool {
        self.as_number().is_some_and(Number::is_f64)
    }
    pub fn as_str(&self) -> Option<&str> {
        if let ValueKind::String(value) = self.kind() {
            Some(value)
        } else {
            None
        }
    }
    pub fn as_array(&self) -> Option<&[ValueRef]> {
        if let ValueKind::Array(value) = self.kind() {
            Some(value)
        } else {
            None
        }
    }
    pub fn as_object(&self) -> Option<&BTreeMap<Arc<str>, ValueRef>> {
        if let ValueKind::Object(value) = self.kind() {
            Some(value)
        } else {
            None
        }
    }
    pub fn get(&self, key: &str) -> Option<&Self> {
        self.as_object()?.get(key)
    }

    pub fn pointer(&self, pointer: &str) -> Option<&Self> {
        if pointer.is_empty() {
            return Some(self);
        }
        pointer
            .strip_prefix('/')?
            .split('/')
            .try_fold(self, |value, segment| {
                let key = unescape(segment)?;
                match value.kind() {
                    ValueKind::Object(entries) => entries.get(key.as_ref()),
                    ValueKind::Array(items) => items.get(array_index(&key)?),
                    _ => None,
                }
            })
    }
}

fn unescape(segment: &str) -> Option<std::borrow::Cow<'_, str>> {
    if !segment.contains('~') {
        return Some(segment.into());
    }
    let mut result = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(character) = chars.next() {
        result.push(if character == '~' {
            match chars.next()? {
                '0' => '~',
                '1' => '/',
                _ => return None,
            }
        } else {
            character
        });
    }
    Some(result.into())
}
fn array_index(index: &str) -> Option<usize> {
    if index.starts_with('+') || (index.starts_with('0') && index.len() > 1) {
        return None;
    }
    index.parse().ok()
}

fn numbers_equal(left: &Number, right: &Number) -> bool {
    left == right
        && (!left.is_f64() || left.as_f64().map(f64::to_bits) == right.as_f64().map(f64::to_bits))
}

impl PartialEq for ValueRef {
    fn eq(&self, other: &Self) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        match (self.kind(), other.kind()) {
            (ValueKind::Null, ValueKind::Null) => true,
            (ValueKind::Bool(left), ValueKind::Bool(right)) => left == right,
            (ValueKind::Number(left), ValueKind::Number(right)) => numbers_equal(left, right),
            (ValueKind::String(left), ValueKind::String(right)) => left == right,
            (ValueKind::Array(left), ValueKind::Array(right)) => left == right,
            (ValueKind::Object(left), ValueKind::Object(right)) => left == right,
            _ => false,
        }
    }
}
impl Eq for ValueRef {}
impl PartialEq<Value> for ValueRef {
    fn eq(&self, other: &Value) -> bool {
        match (self.kind(), other) {
            (ValueKind::Null, Value::Null) => true,
            (ValueKind::Bool(left), Value::Bool(right)) => left == right,
            (ValueKind::Number(left), Value::Number(right)) => numbers_equal(left, right),
            (ValueKind::String(left), Value::String(right)) => left.as_ref() == right,
            (ValueKind::Array(left), Value::Array(right)) => {
                left.len() == right.len()
                    && left.iter().zip(right).all(|(left, right)| left == right)
            }
            (ValueKind::Object(left), Value::Object(right)) => {
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right.get(key.as_ref()).is_some_and(|right| value == right)
                    })
            }
            _ => false,
        }
    }
}
impl PartialEq<ValueRef> for Value {
    fn eq(&self, other: &ValueRef) -> bool {
        other == self
    }
}
impl From<Value> for ValueRef {
    fn from(value: Value) -> Self {
        Self::new(match value {
            Value::Null => ValueKind::Null,
            Value::Bool(value) => ValueKind::Bool(value),
            Value::Number(value) => ValueKind::Number(value),
            Value::String(value) => ValueKind::String(value.into()),
            Value::Array(items) => ValueKind::Array(items.into_iter().map(Into::into).collect()),
            Value::Object(entries) => ValueKind::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (Arc::from(key), value.into()))
                    .collect(),
            ),
        })
    }
}
impl From<bool> for ValueRef {
    fn from(value: bool) -> Self {
        Self::new(ValueKind::Bool(value))
    }
}
impl From<i64> for ValueRef {
    fn from(value: i64) -> Self {
        Self::new(ValueKind::Number(value.into()))
    }
}
impl From<u64> for ValueRef {
    fn from(value: u64) -> Self {
        Self::new(ValueKind::Number(value.into()))
    }
}
impl From<i32> for ValueRef {
    fn from(value: i32) -> Self {
        Self::from(i64::from(value))
    }
}
impl From<String> for ValueRef {
    fn from(value: String) -> Self {
        Self::new(ValueKind::String(value.into()))
    }
}
impl From<&str> for ValueRef {
    fn from(value: &str) -> Self {
        Self::new(ValueKind::String(value.into()))
    }
}
static NULL: LazyLock<ValueRef> = LazyLock::new(ValueRef::null);
impl Index<&str> for ValueRef {
    type Output = Self;
    fn index(&self, key: &str) -> &Self {
        self.get(key).unwrap_or(&NULL)
    }
}
impl Index<usize> for ValueRef {
    type Output = Self;
    fn index(&self, index: usize) -> &Self {
        self.as_array()
            .and_then(|items| items.get(index))
            .unwrap_or(&NULL)
    }
}
impl fmt::Display for ValueRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&serde_json::to_string(self).map_err(|_| fmt::Error)?)
    }
}
impl Serialize for ValueRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.kind() {
            ValueKind::Null => serializer.serialize_unit(),
            ValueKind::Bool(value) => serializer.serialize_bool(*value),
            ValueKind::Number(value) => value.serialize(serializer),
            ValueKind::String(value) => serializer.serialize_str(value),
            ValueKind::Array(items) => {
                let mut output = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    output.serialize_element(item)?;
                }
                output.end()
            }
            ValueKind::Object(entries) => {
                let mut output = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    output.serialize_entry(key.as_ref(), value)?;
                }
                output.end()
            }
        }
    }
}
impl<'de> Deserialize<'de> for ValueRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Into::into)
    }
}

pub trait JsonView: Sized {
    fn shape(&self) -> &'static str;
    fn number(&self) -> Option<&Number>;
    fn array_items(&self) -> impl Iterator<Item = &Self>;
    fn object_items(&self) -> impl Iterator<Item = (&str, &Self)>;
}
impl JsonView for Value {
    fn shape(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
        }
    }
    fn number(&self) -> Option<&Number> {
        self.as_number()
    }
    fn array_items(&self) -> impl Iterator<Item = &Self> {
        self.as_array().into_iter().flatten()
    }
    fn object_items(&self) -> impl Iterator<Item = (&str, &Self)> {
        self.as_object()
            .into_iter()
            .flatten()
            .map(|(key, value)| (key.as_str(), value))
    }
}
impl JsonView for ValueRef {
    fn shape(&self) -> &'static str {
        match self.kind() {
            ValueKind::Null => "null",
            ValueKind::Bool(_) => "boolean",
            ValueKind::Number(_) => "number",
            ValueKind::String(_) => "string",
            ValueKind::Array(_) => "array",
            ValueKind::Object(_) => "object",
        }
    }
    fn number(&self) -> Option<&Number> {
        self.as_number()
    }
    fn array_items(&self) -> impl Iterator<Item = &Self> {
        self.as_array().into_iter().flat_map(|items| items.iter())
    }
    fn object_items(&self) -> impl Iterator<Item = (&str, &Self)> {
        self.as_object()
            .into_iter()
            .flat_map(|entries| entries.iter())
            .map(|(key, value)| (key.as_ref(), value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pointers_decode_escapes_and_reject_invalid_array_indices() {
        let value = ValueRef::from(json!({"a/b": {"~": [1]}, "": true}));
        assert_eq!(value.pointer("/a~1b/~0/0").unwrap(), &json!(1));
        assert_eq!(value.pointer("/").unwrap(), &json!(true));
        for pointer in [
            "a",
            "/a~2b",
            "/a~1b/~0/00",
            "/a~1b/~0/+0",
            "/a~1b/~0/-",
            "/absent/x",
        ] {
            assert!(value.pointer(pointer).is_none(), "{pointer}");
        }
    }

    #[test]
    fn serialization_and_equality_preserve_numeric_representations() {
        let json = r#"[0,0.0,-0.0,18446744073709551615,1.5]"#;
        let values: ValueRef = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&values).unwrap(), json);
        assert_ne!(values[0], values[1]);
        assert_ne!(values[1], values[2]);
        assert_eq!(values[3].as_u64(), Some(u64::MAX));
        assert_eq!(
            serde_json::to_value(&values).unwrap(),
            serde_json::from_str::<Value>(json).unwrap()
        );
    }
}
