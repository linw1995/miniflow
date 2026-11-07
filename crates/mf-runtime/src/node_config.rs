//! Typed, value-driven configuration traversal used by `NodeConfig` derives.
//!
//! The hidden field dispatcher deliberately prefers `NodeConfig` and otherwise
//! treats an ordinary Serde value as opaque. No executable value is deserialized.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{NodeBuildError, PreparedSubgraph};

type Flows = BTreeMap<String, PreparedSubgraph>;

/// Configuration with typed executable leaves. Paths are RFC 6901 JSON pointers.
///
/// Derive this on nested configuration structures as well as the provider's root.
/// Ordinary `Deserialize` fields remain opaque, including `serde_json::Value`.
/// Manual implementations compose the hidden traversal methods; collection must
/// inspect only wire structure, without executing codecs, defaults, or factories.
pub trait NodeConfig: Sized {
    /// Select declared Flow roles without constructing configuration or executing codecs.
    /// Keys are canonical RFC 6901 pointers, including the empty root pointer.
    /// Missing and null leaves remain declared roles with a null value.
    fn flow_parameters(config: &Value) -> Result<BTreeMap<String, Value>, NodeBuildError> {
        // Ordinary configuration is opaque to receiver discovery, even when its
        // wire shape is invalid. Validation belongs to the provider decoder.
        if !Self::contains_flows(&mut BTreeSet::new()) {
            return Ok(BTreeMap::new());
        }
        let mut paths = Vec::new();
        Self::collect(Some(config), "", &mut paths)?;
        validate_flow_paths(&paths)?;
        Ok(paths
            .into_iter()
            .map(|path| {
                let value = config.pointer(&path).cloned().unwrap_or(Value::Null);
                (path, value)
            })
            .collect())
    }

    /// Decode ordinary values and consume exactly the handles for non-null Flow sources.
    fn from_config(
        config: Value,
        mut flows: BTreeMap<String, PreparedSubgraph>,
    ) -> Result<Self, NodeBuildError> {
        let roles = Self::flow_parameters(&config)?;
        for path in flows.keys() {
            if !roles.contains_key(path) {
                return Err(invalid(format!("unexpected prepared Flow field `{path}`")));
            }
        }
        for (path, value) in &roles {
            let present = !value.is_null();
            if present != flows.contains_key(path) {
                return Err(invalid(format!(
                    "prepared Flow field `{path}` does not match its configuration"
                )));
            }
        }
        let result = Self::decode(&mut Some(config), "", &mut flows)?;
        if let Some(path) = flows.keys().next() {
            return Err(invalid(format!("unconsumed prepared Flow field `{path}`")));
        }
        Ok(result)
    }

    #[doc(hidden)]
    fn collect(
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError>;
    #[doc(hidden)]
    fn decode(
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<Self, NodeBuildError>;
    #[doc(hidden)]
    fn contains_flows(visited: &mut BTreeSet<&'static str>) -> bool;
    #[doc(hidden)]
    fn is_flow_leaf() -> bool {
        false
    }
    #[doc(hidden)]
    fn allows_empty_default() -> bool {
        false
    }
    #[doc(hidden)]
    fn is_empty_default(&self) -> bool {
        false
    }
}

fn collect_if_flows<T: NodeConfig>(
    config: Option<&Value>,
    prefix: &str,
    paths: &mut Vec<String>,
) -> Result<(), NodeBuildError> {
    if T::contains_flows(&mut BTreeSet::new()) {
        T::collect(config, prefix, paths)?;
    }
    Ok(())
}

/// Append a literal object key or array index to a JSON pointer.
pub fn join_config_path(prefix: &str, segment: &str) -> String {
    format!("{prefix}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

pub fn validate_flow_paths(paths: &[String]) -> Result<(), NodeBuildError> {
    let mut seen = BTreeSet::new();
    for path in paths {
        let mut chars = path.chars();
        if !path.is_empty() && chars.next() != Some('/') {
            return Err(invalid(format!("Flow path `{path}` is not a JSON pointer")));
        }
        while let Some(ch) = chars.next() {
            if ch == '~' && !matches!(chars.next(), Some('0' | '1')) {
                return Err(invalid(format!(
                    "Flow path `{path}` has an invalid JSON pointer escape"
                )));
            }
        }
        if !seen.insert(path) {
            return Err(invalid(format!("duplicate Flow path `{path}`")));
        }
    }
    Ok(())
}

pub fn invalid(message: impl Into<String>) -> NodeBuildError {
    crate::NodeInvalidSubgraphSnafu {
        message: message.into(),
    }
    .build()
}

pub fn object(
    config: Option<&Value>,
) -> Result<Option<&serde_json::Map<String, Value>>, NodeBuildError> {
    config
        .map(|value| {
            value
                .as_object()
                .ok_or_else(|| invalid("expected a configuration object"))
        })
        .transpose()
}

pub fn array(config: Option<&Value>) -> Result<Option<&Vec<Value>>, NodeBuildError> {
    config
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| invalid("expected a configuration array"))
        })
        .transpose()
}

pub fn take_array(config: &mut Option<Value>) -> Result<Vec<Value>, NodeBuildError> {
    match config.take() {
        Some(Value::Array(values)) => Ok(values),
        _ => Err(invalid("expected a configuration array")),
    }
}

pub fn take_object(
    config: &mut Option<Value>,
) -> Result<serde_json::Map<String, Value>, NodeBuildError> {
    match config.take() {
        Some(Value::Object(values)) => Ok(values),
        _ => Err(invalid("expected a configuration object")),
    }
}

// Let Serde distinguish a missing ordinary field from an explicitly null one.
// Its missing-field deserializer also preserves Option and custom nullable codecs'
// own semantics instead of fabricating a JSON null before decoding.
fn decode_ordinary<T: DeserializeOwned>(config: &mut Option<Value>) -> Result<T, NodeBuildError> {
    #[derive(serde::Deserialize)]
    struct Field<T> {
        value: T,
    }
    let mut object = serde_json::Map::new();
    if let Some(value) = config.take() {
        object.insert("value".into(), value);
    }
    Ok(crate::deserialize_config::<Field<T>>(Value::Object(object))?.value)
}

/// Unlike `Option<Value>`, this preserves an explicit JSON null.
#[derive(Default)]
pub struct Presence(pub Option<Value>);
impl<'de> serde::Deserialize<'de> for Presence {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <Value as serde::Deserialize>::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

pub struct ConfigField<T, Canonical = ()>(PhantomData<fn() -> (T, Canonical)>);
impl<T, Canonical> Default for ConfigField<T, Canonical> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

pub trait ConfigFieldDispatch<T> {
    fn field_collect(
        self,
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError>;
    fn field_decode(
        self,
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<T, NodeBuildError>;
    fn field_contains(self, visited: &mut BTreeSet<&'static str>) -> bool;
    fn field_is_leaf(self) -> bool;
    fn field_default_allowed(self) -> bool;
    fn field_default_empty(self, value: &T) -> bool;
}

impl<T: NodeConfig> ConfigFieldDispatch<T> for &&ConfigField<T> {
    fn field_collect(
        self,
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError> {
        collect_if_flows::<T>(config, prefix, paths)
    }
    fn field_decode(
        self,
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<T, NodeBuildError> {
        T::decode(config, prefix, flows)
    }
    fn field_contains(self, visited: &mut BTreeSet<&'static str>) -> bool {
        T::contains_flows(visited)
    }
    fn field_is_leaf(self) -> bool {
        T::is_flow_leaf()
    }
    fn field_default_allowed(self) -> bool {
        T::allows_empty_default()
    }
    fn field_default_empty(self, value: &T) -> bool {
        value.is_empty_default()
    }
}
impl<T: DeserializeOwned> ConfigFieldDispatch<T> for &ConfigField<T> {
    fn field_collect(
        self,
        _: Option<&Value>,
        _: &str,
        _: &mut Vec<String>,
    ) -> Result<(), NodeBuildError> {
        Ok(())
    }
    fn field_decode(
        self,
        config: &mut Option<Value>,
        _: &str,
        _: &mut Flows,
    ) -> Result<T, NodeBuildError> {
        decode_ordinary(config)
    }
    fn field_contains(self, _: &mut BTreeSet<&'static str>) -> bool {
        false
    }
    fn field_is_leaf(self) -> bool {
        false
    }
    fn field_default_allowed(self) -> bool {
        false
    }
    fn field_default_empty(self, _: &T) -> bool {
        true
    }
}

/// Container syntax is only a candidate for generated traversal. Autoref dispatch
/// first honors an actual NodeConfig implementation, then requires exact identity
/// with the canonical standard container, and finally falls back to opaque Serde.
/// The canonical decoder callback is type-checked even when it is not selected;
/// no type-name comparisons, dynamic casts, or executable deserialization are used.
pub trait ConfigContainerDispatch<T, Canonical> {
    fn container_collect<F>(
        self,
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
        traverse: F,
    ) -> Result<(), NodeBuildError>
    where
        F: FnOnce(Option<&Value>, &str, &mut Vec<String>) -> Result<(), NodeBuildError>;
    fn container_decode<F>(
        self,
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
        traverse: F,
    ) -> Result<T, NodeBuildError>
    where
        F: FnOnce(&mut Option<Value>, &str, &mut Flows) -> Result<Canonical, NodeBuildError>;
    fn container_contains<F>(self, visited: &mut BTreeSet<&'static str>, traverse: F) -> bool
    where
        F: FnOnce(&mut BTreeSet<&'static str>) -> bool;
    fn container_is_leaf<F>(self, traverse: F) -> bool
    where
        F: FnOnce() -> bool;
    fn container_default_allowed(self) -> bool;
    fn container_default_empty<F>(self, value: &T, empty: F) -> bool
    where
        F: FnOnce(&Canonical) -> bool;
}

impl<T: NodeConfig, C> ConfigContainerDispatch<T, C> for &&&ConfigField<T, C> {
    fn container_collect<F>(
        self,
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
        _: F,
    ) -> Result<(), NodeBuildError>
    where
        F: FnOnce(Option<&Value>, &str, &mut Vec<String>) -> Result<(), NodeBuildError>,
    {
        collect_if_flows::<T>(config, prefix, paths)
    }
    fn container_decode<F>(
        self,
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
        _: F,
    ) -> Result<T, NodeBuildError>
    where
        F: FnOnce(&mut Option<Value>, &str, &mut Flows) -> Result<C, NodeBuildError>,
    {
        T::decode(config, prefix, flows)
    }
    fn container_contains<F>(self, visited: &mut BTreeSet<&'static str>, _: F) -> bool
    where
        F: FnOnce(&mut BTreeSet<&'static str>) -> bool,
    {
        T::contains_flows(visited)
    }
    fn container_is_leaf<F>(self, _: F) -> bool
    where
        F: FnOnce() -> bool,
    {
        T::is_flow_leaf()
    }
    fn container_default_allowed(self) -> bool {
        T::allows_empty_default()
    }
    fn container_default_empty<F>(self, value: &T, _: F) -> bool
    where
        F: FnOnce(&C) -> bool,
    {
        value.is_empty_default()
    }
}

impl<T> ConfigContainerDispatch<T, T> for &&ConfigField<T, T> {
    fn container_collect<F>(
        self,
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
        traverse: F,
    ) -> Result<(), NodeBuildError>
    where
        F: FnOnce(Option<&Value>, &str, &mut Vec<String>) -> Result<(), NodeBuildError>,
    {
        traverse(config, prefix, paths)
    }
    fn container_decode<F>(
        self,
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
        traverse: F,
    ) -> Result<T, NodeBuildError>
    where
        F: FnOnce(&mut Option<Value>, &str, &mut Flows) -> Result<T, NodeBuildError>,
    {
        traverse(config, prefix, flows)
    }
    fn container_contains<F>(self, visited: &mut BTreeSet<&'static str>, traverse: F) -> bool
    where
        F: FnOnce(&mut BTreeSet<&'static str>) -> bool,
    {
        traverse(visited)
    }
    fn container_is_leaf<F>(self, traverse: F) -> bool
    where
        F: FnOnce() -> bool,
    {
        traverse()
    }
    fn container_default_allowed(self) -> bool {
        true
    }
    fn container_default_empty<F>(self, value: &T, empty: F) -> bool
    where
        F: FnOnce(&T) -> bool,
    {
        empty(value)
    }
}

impl<T: DeserializeOwned, C> ConfigContainerDispatch<T, C> for &ConfigField<T, C> {
    fn container_collect<F>(
        self,
        _: Option<&Value>,
        _: &str,
        _: &mut Vec<String>,
        _: F,
    ) -> Result<(), NodeBuildError>
    where
        F: FnOnce(Option<&Value>, &str, &mut Vec<String>) -> Result<(), NodeBuildError>,
    {
        Ok(())
    }
    fn container_decode<F>(
        self,
        config: &mut Option<Value>,
        _: &str,
        _: &mut Flows,
        _: F,
    ) -> Result<T, NodeBuildError>
    where
        F: FnOnce(&mut Option<Value>, &str, &mut Flows) -> Result<C, NodeBuildError>,
    {
        decode_ordinary(config)
    }
    fn container_contains<F>(self, _: &mut BTreeSet<&'static str>, _: F) -> bool
    where
        F: FnOnce(&mut BTreeSet<&'static str>) -> bool,
    {
        false
    }
    fn container_is_leaf<F>(self, _: F) -> bool
    where
        F: FnOnce() -> bool,
    {
        false
    }
    fn container_default_allowed(self) -> bool {
        false
    }
    fn container_default_empty<F>(self, _: &T, _: F) -> bool
    where
        F: FnOnce(&C) -> bool,
    {
        true
    }
}

impl NodeConfig for PreparedSubgraph {
    fn collect(
        _: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError> {
        paths.push(prefix.to_owned());
        Ok(())
    }
    fn decode(
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<Self, NodeBuildError> {
        config.take();
        flows
            .remove(prefix)
            .ok_or_else(|| invalid(format!("missing prepared Flow field `{prefix}`")))
    }
    fn contains_flows(_: &mut BTreeSet<&'static str>) -> bool {
        true
    }
    fn is_flow_leaf() -> bool {
        true
    }
}
impl NodeConfig for crate::Flow {
    fn collect(
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError> {
        PreparedSubgraph::collect(config, prefix, paths)
    }
    fn decode(
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<Self, NodeBuildError> {
        PreparedSubgraph::decode(config, prefix, flows)?.into_flow()
    }
    fn contains_flows(_: &mut BTreeSet<&'static str>) -> bool {
        true
    }
    fn is_flow_leaf() -> bool {
        true
    }
}

macro_rules! opaque {
    ($($ty:ty),* $(,)?) => { $(impl NodeConfig for $ty {
        fn collect(_: Option<&Value>, _: &str, _: &mut Vec<String>) -> Result<(), NodeBuildError> { Ok(()) }
        fn decode(config: &mut Option<Value>, _: &str, _: &mut Flows) -> Result<Self, NodeBuildError> { decode_ordinary(config) }
        fn contains_flows(_: &mut BTreeSet<&'static str>) -> bool { false }
    })* };
}
opaque!(
    Value,
    String,
    bool,
    char,
    (),
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    f32,
    f64,
    serde_json::Number
);

impl<T: NodeConfig> NodeConfig for Option<T> {
    fn collect(
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError> {
        if T::is_flow_leaf() || config.is_some_and(|value| !value.is_null()) {
            collect_if_flows::<T>(config, prefix, paths)?;
        }
        Ok(())
    }
    fn decode(
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<Self, NodeBuildError> {
        if config.as_ref().is_none_or(Value::is_null) && !flows.contains_key(prefix) {
            config.take();
            Ok(None)
        } else {
            T::decode(config, prefix, flows).map(Some)
        }
    }
    fn contains_flows(visited: &mut BTreeSet<&'static str>) -> bool {
        T::contains_flows(visited)
    }
    fn is_flow_leaf() -> bool {
        T::is_flow_leaf()
    }
    fn allows_empty_default() -> bool {
        true
    }
    fn is_empty_default(&self) -> bool {
        self.is_none()
    }
}
impl<T: NodeConfig> NodeConfig for Box<T> {
    fn collect(
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError> {
        collect_if_flows::<T>(config, prefix, paths)
    }
    fn decode(
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<Self, NodeBuildError> {
        T::decode(config, prefix, flows).map(Box::new)
    }
    fn contains_flows(visited: &mut BTreeSet<&'static str>) -> bool {
        T::contains_flows(visited)
    }
    fn is_flow_leaf() -> bool {
        T::is_flow_leaf()
    }
}
impl<T: NodeConfig> NodeConfig for Vec<T> {
    fn collect(
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError> {
        if let Some(values) = array(config)? {
            for (index, value) in values.iter().enumerate() {
                collect_if_flows::<T>(
                    Some(value),
                    &join_config_path(prefix, &index.to_string()),
                    paths,
                )?;
            }
        }
        Ok(())
    }
    fn decode(
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<Self, NodeBuildError> {
        take_array(config)?
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                T::decode(
                    &mut Some(value),
                    &join_config_path(prefix, &index.to_string()),
                    flows,
                )
            })
            .collect()
    }
    fn contains_flows(visited: &mut BTreeSet<&'static str>) -> bool {
        T::contains_flows(visited)
    }
    fn allows_empty_default() -> bool {
        true
    }
    fn is_empty_default(&self) -> bool {
        self.is_empty()
    }
}
impl<T: NodeConfig, const N: usize> NodeConfig for [T; N] {
    fn collect(
        config: Option<&Value>,
        prefix: &str,
        paths: &mut Vec<String>,
    ) -> Result<(), NodeBuildError> {
        if let Some(values) = array(config)?
            && values.len() != N
        {
            return Err(invalid(format!("expected an array of length {N}")));
        }
        Vec::<T>::collect(config, prefix, paths)
    }
    fn decode(
        config: &mut Option<Value>,
        prefix: &str,
        flows: &mut Flows,
    ) -> Result<Self, NodeBuildError> {
        match Vec::<T>::decode(config, prefix, flows)?.try_into() {
            Ok(values) => Ok(values),
            Err(_) => crate::NodeInvalidSubgraphSnafu {
                message: format!("expected an array of length {N}"),
            }
            .fail(),
        }
    }
    fn contains_flows(visited: &mut BTreeSet<&'static str>) -> bool {
        T::contains_flows(visited)
    }
}
macro_rules! map_config {
    ($map:ident) => {
        impl<T: NodeConfig> NodeConfig for $map<String, T> {
            fn collect(
                config: Option<&Value>,
                prefix: &str,
                paths: &mut Vec<String>,
            ) -> Result<(), NodeBuildError> {
                if let Some(values) = object(config)? {
                    for (key, value) in values {
                        collect_if_flows::<T>(Some(value), &join_config_path(prefix, key), paths)?;
                    }
                }
                Ok(())
            }
            fn decode(
                config: &mut Option<Value>,
                prefix: &str,
                flows: &mut Flows,
            ) -> Result<Self, NodeBuildError> {
                take_object(config)?
                    .into_iter()
                    .map(|(key, value)| {
                        let decoded =
                            T::decode(&mut Some(value), &join_config_path(prefix, &key), flows)?;
                        Ok((key, decoded))
                    })
                    .collect()
            }
            fn contains_flows(visited: &mut BTreeSet<&'static str>) -> bool {
                T::contains_flows(visited)
            }
            fn allows_empty_default() -> bool {
                true
            }
            fn is_empty_default(&self) -> bool {
                self.is_empty()
            }
        }
    };
}
map_config!(BTreeMap);
map_config!(HashMap);

macro_rules! tuple_config {
    ($($ty:ident:$index:tt),+) => { impl<$($ty: NodeConfig),+> NodeConfig for ($($ty,)+) {
        fn collect(config: Option<&Value>, prefix: &str, paths: &mut Vec<String>) -> Result<(), NodeBuildError> {
            if let Some(values) = array(config)? {
                let expected = 0 $(+ { let _ = stringify!($ty); 1 })+;
                if values.len() != expected { return Err(invalid(format!("expected a tuple of length {expected}"))); }
                $( collect_if_flows::<$ty>(values.get($index), &join_config_path(prefix, stringify!($index)), paths)?; )+
            }
            Ok(())
        }
        fn decode(config: &mut Option<Value>, prefix: &str, flows: &mut Flows) -> Result<Self, NodeBuildError> {
            let values = take_array(config)?;
            let expected = 0 $(+ { let _ = stringify!($ty); 1 })+;
            if values.len() != expected { return Err(invalid(format!("expected a tuple of length {expected}"))); }
            let mut values = values.into_iter();
            Ok(($($ty::decode(&mut values.next(), &join_config_path(prefix, stringify!($index)), flows)?,)+))
        }
        fn contains_flows(visited: &mut BTreeSet<&'static str>) -> bool { false $(|| $ty::contains_flows(visited))+ }
    } };
}
tuple_config!(A:0);
tuple_config!(A:0, B:1);
tuple_config!(A:0, B:1, C:2);
tuple_config!(A:0, B:1, C:2, D:3);
tuple_config!(A:0, B:1, C:2, D:3, E:4);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8, J:9);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8, J:9, K:10);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8, J:9, K:10, L:11);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8, J:9, K:10, L:11, M:12);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8, J:9, K:10, L:11, M:12, N:13);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8, J:9, K:10, L:11, M:12, N:13, O:14);
tuple_config!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7, I:8, J:9, K:10, L:11, M:12, N:13, O:14, P:15);

#[cfg(test)]
mod tests {
    #![allow(dead_code)]
    use super::*;
    use serde_json::json;

    type Body = PreparedSubgraph;
    #[derive(serde::Deserialize)]
    struct Ordinary {
        count: usize,
    }
    #[derive(crate::NodeConfig)]
    #[config(runtime = "crate")]
    struct Tree {
        body: Option<Body>,
        next: Option<Box<Tree>>,
    }
    #[derive(crate::NodeConfig)]
    #[config(runtime = "crate")]
    struct Nested {
        #[serde(alias = "legacy")]
        body: Body,
        tree: Option<Box<Tree>>,
        mixed: (Ordinary, Vec<Body>),
        array: [Body; 1],
        maps: HashMap<String, Box<Body>>,
        value: Value,
    }
    #[derive(crate::NodeConfig)]
    #[config(runtime = "crate")]
    enum External {
        Unit,
        Named { body: Body },
        Newtype(Body),
        Tuple(usize, Body),
    }
    #[derive(crate::NodeConfig)]
    #[config(runtime = "crate")]
    #[serde(tag = "kind", deny_unknown_fields)]
    enum Internal {
        Unit,
        Named { body: Body },
        Wrapped(Inner),
    }
    #[derive(crate::NodeConfig)]
    #[config(runtime = "crate")]
    #[serde(tag = "kind", content = "data", deny_unknown_fields)]
    enum Adjacent {
        Unit,
        Named { body: Body },
        Newtype(Body),
        Tuple(usize, Body),
    }
    #[derive(crate::NodeConfig)]
    #[config(runtime = "crate")]
    struct Inner {
        body: Body,
    }
    #[derive(crate::NodeConfig)]
    #[config(runtime = "crate")]
    struct Flatten {
        ordinary: usize,
        #[serde(flatten)]
        inner: Inner,
    }
    #[derive(crate::NodeConfig)]
    #[config(runtime = "crate")]
    struct Unit;

    fn body() -> Body {
        Body::new(vec![], vec![], |_| panic!("must not execute"))
    }
    fn inject<T: NodeConfig>(config: Value, paths: &[&str]) -> T {
        assert_eq!(
            T::flow_parameters(&config)
                .unwrap()
                .into_keys()
                .collect::<Vec<_>>(),
            paths
        );
        T::from_config(
            config,
            paths
                .iter()
                .map(|path| (path.to_string(), body()))
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn concrete_paths_recurse_through_aliases_mixed_containers_and_finite_trees() {
        let config = json!({"legacy": {}, "tree": {"body": null, "next": {"body": {}, "next": null}}, "mixed": [{"count": 7}, [{}]], "array": [{}], "maps": {"a/~": {}}, "value": {"body": {}}});
        assert_eq!(
            Nested::flow_parameters(&config)
                .unwrap()
                .into_keys()
                .collect::<Vec<_>>(),
            [
                "/array/0",
                "/legacy",
                "/maps/a~1~0",
                "/mixed/1/0",
                "/tree/body",
                "/tree/next/body"
            ]
        );
        let value = Nested::from_config(
            config,
            [
                "/legacy",
                "/tree/next/body",
                "/mixed/1/0",
                "/array/0",
                "/maps/a~1~0",
            ]
            .iter()
            .map(|path| (path.to_string(), body()))
            .collect(),
        )
        .unwrap();
        assert_eq!(value.mixed.0.count, 7);
        assert!(value.tree.unwrap().body.is_none());
    }

    #[test]
    fn tagged_enums_inject_only_the_selected_variant() {
        let _: External = inject(json!({"Named": {"body": {}}}), &["/Named/body"]);
        let _: External = inject(json!({"Newtype": {}}), &["/Newtype"]);
        let _: External = inject(json!({"Tuple": [3, {}]}), &["/Tuple/1"]);
        let _: External = inject(json!("Unit"), &[]);
        let _: Internal = inject(json!({"kind": "Named", "body": {}}), &["/body"]);
        let _: Internal = inject(json!({"kind": "Wrapped", "body": {}}), &["/body"]);
        let _: Internal = inject(json!({"kind": "Unit"}), &[]);
        let _: Adjacent = inject(
            json!({"kind": "Named", "data": {"body": {}}}),
            &["/data/body"],
        );
        let _: Adjacent = inject(json!({"kind": "Newtype", "data": {}}), &["/data"]);
        let _: Adjacent = inject(json!({"kind": "Tuple", "data": [3, {}]}), &["/data/1"]);
        let _: Adjacent = inject(json!({"kind": "Unit"}), &[]);
        assert!(External::flow_parameters(&json!({"Unit": {"unexpected": 1}})).is_err());
        for value in [
            json!({"kind": "Unit", "unexpected": 1}),
            json!({"kind": "Named", "body": {}, "unexpected": 1}),
        ] {
            assert!(Internal::flow_parameters(&value).is_ok());
            assert!(Internal::from_config(value, BTreeMap::new()).is_err());
        }
        let value = json!({"kind": "Unit", "unexpected": 1});
        assert!(Adjacent::flow_parameters(&value).unwrap().is_empty());
        assert!(Adjacent::from_config(value, BTreeMap::new()).is_err());
        assert!(
            Adjacent::flow_parameters(&json!({"kind": "Unit", "data": {"unexpected": 1}})).is_err()
        );
    }

    #[test]
    fn flatten_uses_the_enclosing_object_and_units_require_null() {
        let value: Flatten = inject(json!({"ordinary": 3, "body": {}}), &["/body"]);
        assert_eq!(value.ordinary, 3);
        let _: Unit = inject(json!(null), &[]);
        assert!(Unit::flow_parameters(&json!({})).unwrap().is_empty());
        assert!(Unit::from_config(json!(1), BTreeMap::new()).is_err());
    }

    #[test]
    fn malformed_array_lengths_are_rejected_before_leaf_discovery() {
        type Array = [Body; 1];
        assert!(Array::flow_parameters(&json!([{}, {}])).is_err());
        #[derive(crate::NodeConfig)]
        #[config(runtime = "crate")]
        struct Config {
            bodies: [Body; 1],
        }
        assert!(Config::flow_parameters(&json!({"bodies": [{}, {}]})).is_err());
        assert_eq!(
            Body::flow_parameters(&json!({})).unwrap(),
            BTreeMap::from([("".into(), json!({}))])
        );
    }

    #[test]
    fn aliases_preserve_empty_optional_defaults_without_metadata_execution() {
        type OptionalBody = Option<Body>;
        #[derive(crate::NodeConfig)]
        #[config(runtime = "crate")]
        struct Config {
            #[serde(default)]
            body: OptionalBody,
        }
        assert_eq!(
            Config::flow_parameters(&json!({})).unwrap(),
            BTreeMap::from([("/body".into(), Value::Null)])
        );
        assert!(
            Config::from_config(json!({}), BTreeMap::new())
                .unwrap()
                .body
                .is_none()
        );

        static DEFAULT_CALLS: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        #[derive(crate::NodeConfig)]
        #[config(runtime = "crate")]
        #[serde(default)]
        struct Container {
            body: OptionalBody,
        }
        impl Default for Container {
            fn default() -> Self {
                DEFAULT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Self { body: Some(body()) }
            }
        }
        assert_eq!(
            Container::flow_parameters(&json!({})).unwrap(),
            BTreeMap::from([("/body".into(), Value::Null)])
        );
        assert_eq!(DEFAULT_CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(Container::from_config(json!({}), BTreeMap::new()).is_err());
        assert_eq!(DEFAULT_CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn blended_flow_leaves_are_rejected_before_source_discovery() {
        #[derive(crate::NodeConfig)]
        #[config(runtime = "crate")]
        #[serde(tag = "kind")]
        enum Blended {
            Body(Body),
        }
        assert!(Blended::flow_parameters(&json!({"kind": "Body", "source": "unopened"})).is_err());
        #[derive(crate::NodeConfig)]
        #[config(runtime = "crate")]
        struct BlendedFlatten {
            ordinary: usize,
            #[serde(flatten)]
            body: Body,
        }
        assert!(
            BlendedFlatten::flow_parameters(&json!({"ordinary": 1, "source": "unopened"})).is_err()
        );
    }

    #[test]
    fn prepared_presence_must_match_the_non_null_source() {
        for value in [json!({}), json!({"body": null})] {
            assert!(
                Inner::from_config(value.clone(), BTreeMap::from([("/body".into(), body())]))
                    .is_err()
            );
            assert!(Tree::from_config(value, BTreeMap::from([("/body".into(), body())])).is_err());
        }
        assert!(Inner::from_config(json!({"body": {}}), BTreeMap::new()).is_err());
        assert!(Body::from_config(Value::Null, BTreeMap::from([("".into(), body())])).is_err());
    }

    mod foreign_container_authority {
        use crate::NodeConfig;
        use serde_json::json;
        use std::marker::PhantomData;

        struct BTreeMap<K, V>(PhantomData<(K, V)>);
        impl<K, V> Default for BTreeMap<K, V> {
            fn default() -> Self {
                Self(PhantomData)
            }
        }
        impl<K, V> BTreeMap<K, V> {
            fn insert(&mut self, _key: K, _value: V) {}
        }
        impl<'de, K, V> serde::Deserialize<'de> for BTreeMap<K, V> {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                let _ = <serde_json::Value as serde::Deserialize>::deserialize(decoder)?;
                Ok(Self::default())
            }
        }
        struct Vec<T>(PhantomData<T>);
        impl<T> Vec<T> {
            fn new() -> Self {
                Self(PhantomData)
            }
            fn push(&mut self, _value: T) {}
        }
        impl<'de, T> serde::Deserialize<'de> for Vec<T> {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                let _ = <serde_json::Value as serde::Deserialize>::deserialize(decoder)?;
                Ok(Self(PhantomData))
            }
        }
        struct Box<T>(PhantomData<T>);
        impl<'de, T> serde::Deserialize<'de> for Box<T> {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                let _ = <serde_json::Value as serde::Deserialize>::deserialize(decoder)?;
                Ok(Self(PhantomData))
            }
        }
        struct Option<T>(PhantomData<T>);
        impl<'de, T> serde::Deserialize<'de> for Option<T> {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                let _ = <serde_json::Value as serde::Deserialize>::deserialize(decoder)?;
                Ok(Self(PhantomData))
            }
        }
        #[derive(crate::NodeConfig)]
        #[config(runtime = "crate")]
        struct Config {
            bodies: BTreeMap<String, crate::PreparedSubgraph>,
        }
        #[derive(crate::NodeConfig)]
        #[config(runtime = "crate")]
        struct OtherContainers {
            vector: Vec<crate::Flow>,
            boxed: Box<crate::Flow>,
            optional: Option<crate::Flow>,
        }

        #[test]
        fn foreign_deserialize_map_does_not_authorize_executable_elements() {
            let paths = Config::flow_parameters(&json!({"bodies": {"candidate": {}}})).unwrap();
            assert!(paths.is_empty());
            assert!(
                Config::from_config(
                    json!({"bodies": {"candidate": {}}}),
                    std::collections::BTreeMap::new()
                )
                .is_ok()
            );
            let config = json!({"vector": [{"source": "never-open"}], "boxed": {}, "optional": {}});
            assert!(
                OtherContainers::flow_parameters(&config)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                OtherContainers::from_config(config, std::collections::BTreeMap::new()).is_ok()
            );
        }
    }

    mod manual_container_authority {
        use crate::{NodeBuildError, NodeConfig, PreparedSubgraph};
        use serde_json::{Value, json};
        struct Vec<T>(T);
        impl<T: NodeConfig> NodeConfig for Vec<T> {
            fn collect(
                config: Option<&Value>,
                prefix: &str,
                paths: &mut std::vec::Vec<String>,
            ) -> Result<(), NodeBuildError> {
                T::collect(config, prefix, paths)
            }
            fn decode(
                config: &mut Option<Value>,
                prefix: &str,
                flows: &mut std::collections::BTreeMap<String, PreparedSubgraph>,
            ) -> Result<Self, NodeBuildError> {
                T::decode(config, prefix, flows).map(Self)
            }
            fn contains_flows(visited: &mut std::collections::BTreeSet<&'static str>) -> bool {
                T::contains_flows(visited)
            }
            fn is_flow_leaf() -> bool {
                T::is_flow_leaf()
            }
        }
        #[derive(crate::NodeConfig)]
        #[config(runtime = "crate")]
        struct Config {
            body: Vec<PreparedSubgraph>,
        }
        #[test]
        fn explicit_node_config_implementation_wins_over_container_spelling() {
            let config = json!({"body": {}});
            assert_eq!(
                Config::flow_parameters(&config).unwrap(),
                std::collections::BTreeMap::from([("/body".into(), json!({}))])
            );
            assert!(
                Config::from_config(
                    config,
                    std::collections::BTreeMap::from([("/body".into(), super::body())])
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn paths_accept_root_and_escape_literal_segments() {
        assert_eq!(join_config_path("", "a/~"), "/a~1~0");
        assert_eq!(join_config_path("/a", ""), "/a/");
        assert!(validate_flow_paths(&["".into(), "/a/~01/~1/".into()]).is_ok());
        for invalid_path in ["body", "/~", "/~2"] {
            assert!(validate_flow_paths(&[invalid_path.into()]).is_err());
        }
        assert!(validate_flow_paths(&["/body".into(), "/body".into()]).is_err());
    }
}
