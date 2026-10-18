use cel_core::{MapKey, Value as CelValue};
use mf_runtime::{ValueKind, ValueRef, ValueType};
use serde_json::Number;
use snafu::{ResultExt, Snafu, ensure};
use std::{fmt, sync::Arc};

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_COLLECTION_ENTRIES: usize = 10_000;

#[derive(Debug, Snafu)]
pub enum ConversionError {
    #[snafu(display("path `{path}`: {message}"))]
    InvalidValue { path: String, message: String },
    #[snafu(display("path `{path}`: {source}"))]
    Evaluate {
        path: String,
        source: cel_core::EvalError,
    },
}

#[derive(Default)]
pub struct Budget {
    entries: usize,
}

impl Budget {
    fn consume(&mut self, count: usize, path: &Path<'_>) -> Result<(), ConversionError> {
        ensure!(
            count <= MAX_COLLECTION_ENTRIES - self.entries,
            InvalidValueSnafu {
                path: path.to_string(),
                message: format!("exceeds the collection entry limit of {MAX_COLLECTION_ENTRIES}"),
            }
        );
        self.entries += count;
        Ok(())
    }
}

enum Path<'a> {
    Root(&'a str),
    Index(&'a Path<'a>, usize),
    Key(&'a Path<'a>, &'a str),
}

impl fmt::Display for Path<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        use fmt::Write as _;
        match self {
            Self::Root(root) => formatter.write_str(root),
            Self::Index(parent, index) => write!(formatter, "{parent}/{index}"),
            Self::Key(parent, key) => {
                write!(formatter, "{parent}/")?;
                for character in key.chars() {
                    match character {
                        '~' => formatter.write_str("~0")?,
                        '/' => formatter.write_str("~1")?,
                        _ => formatter.write_char(character)?,
                    }
                }
                Ok(())
            }
        }
    }
}

fn invalid(path: &Path<'_>, message: impl Into<String>) -> ConversionError {
    InvalidValueSnafu {
        path: path.to_string(),
        message: message.into(),
    }
    .build()
}

fn check_depth(depth: usize, path: &Path<'_>) -> Result<(), ConversionError> {
    ensure!(
        depth <= ValueType::MAX_DEPTH,
        InvalidValueSnafu {
            path: path.to_string(),
            message: format!("exceeds nesting depth {}", ValueType::MAX_DEPTH),
        }
    );
    Ok(())
}

pub fn json_to_cel(
    value: &ValueRef,
    value_type: &ValueType,
    budget: &mut Budget,
    path: &str,
    depth: usize,
) -> Result<CelValue, ConversionError> {
    json_to_cel_at(value, value_type, budget, &Path::Root(path), depth)
}

fn json_to_cel_at(
    value: &ValueRef,
    value_type: &ValueType,
    budget: &mut Budget,
    path: &Path<'_>,
    depth: usize,
) -> Result<CelValue, ConversionError> {
    check_depth(depth, path)?;
    match (value_type, value.kind()) {
        (ValueType::Any | ValueType::Null, ValueKind::Null) => Ok(CelValue::Null),
        (ValueType::Any | ValueType::Boolean, ValueKind::Bool(value)) => Ok(CelValue::Bool(*value)),
        (ValueType::Any, ValueKind::Number(value)) => {
            if let Some(value) = value.as_i64() {
                Ok(CelValue::Int(value))
            } else if let Some(value) = value.as_u64() {
                Ok(CelValue::UInt(value))
            } else {
                mf_runtime::number_to_f64(value)
                    .map(CelValue::Double)
                    .ok_or_else(|| invalid(path, "expected finite JSON number"))
            }
        }
        (ValueType::Uint64, ValueKind::Number(value)) => mf_runtime::number_to_u64(value)
            .map(CelValue::UInt)
            .ok_or_else(|| invalid(path, "expected uint64")),
        (ValueType::Int64, ValueKind::Number(value)) => mf_runtime::number_to_i64(value)
            .map(CelValue::Int)
            .ok_or_else(|| invalid(path, "expected int64")),
        (ValueType::Float64, ValueKind::Number(value)) => mf_runtime::number_to_f64(value)
            .map(CelValue::Double)
            .ok_or_else(|| invalid(path, "expected finite float64")),
        (ValueType::Any | ValueType::String, ValueKind::String(value)) => {
            Ok(CelValue::String(Arc::clone(value)))
        }
        (ValueType::Any | ValueType::Array | ValueType::List(_), ValueKind::Array(items)) => {
            let inner = match value_type {
                ValueType::List(inner) => inner.as_ref(),
                _ => &ValueType::Any,
            };
            budget.consume(items.len(), path)?;
            let values = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    json_to_cel_at(item, inner, budget, &Path::Index(path, index), depth + 1)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CelValue::list(values))
        }
        (ValueType::Any | ValueType::Object | ValueType::Map(_), ValueKind::Object(entries)) => {
            let inner = match value_type {
                ValueType::Map(inner) => inner.as_ref(),
                _ => &ValueType::Any,
            };
            budget.consume(entries.len(), path)?;
            let values = entries
                .iter()
                .map(|(key, item)| {
                    json_to_cel_at(item, inner, budget, &Path::Key(path, key), depth + 1)
                        .map(|converted| (Arc::clone(key), converted))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CelValue::map(values))
        }
        _ => Err(invalid(path, format!("expected {value_type}"))),
    }
}

fn actual_cel_type(value: &CelValue) -> &'static str {
    match value {
        CelValue::Null => "null",
        CelValue::Bool(_) => "boolean",
        CelValue::Int(_) => "int64",
        CelValue::UInt(_) => "uint64",
        CelValue::Double(_) => "float64",
        CelValue::String(_) => "string",
        CelValue::List(_) => "list",
        CelValue::Map(_) => "map",
        _ => "unsupported CEL value",
    }
}

pub fn cel_to_json(
    value: &CelValue,
    value_type: &ValueType,
    budget: &mut Budget,
    path: &str,
    depth: usize,
) -> Result<ValueRef, ConversionError> {
    cel_to_json_at(value, value_type, budget, &Path::Root(path), depth)
}

fn cel_to_json_at(
    value: &CelValue,
    value_type: &ValueType,
    budget: &mut Budget,
    path: &Path<'_>,
    depth: usize,
) -> Result<ValueRef, ConversionError> {
    check_depth(depth, path)?;
    if let CelValue::Error(error) = value {
        return Err(error.as_ref().clone()).context(EvaluateSnafu {
            path: path.to_string(),
        });
    }
    match (value_type, value) {
        (ValueType::Any | ValueType::Null, CelValue::Null) => Ok(ValueRef::null()),
        (ValueType::Any | ValueType::Boolean, CelValue::Bool(value)) => Ok(ValueRef::from(*value)),
        (ValueType::Any | ValueType::Int64, CelValue::Int(value)) => Ok(ValueRef::from(*value)),
        (ValueType::Any | ValueType::Uint64, CelValue::UInt(value)) => Ok(ValueRef::from(*value)),
        (ValueType::Any | ValueType::Float64, CelValue::Double(value)) => Number::from_f64(*value)
            .map(|number| ValueRef::new(ValueKind::Number(number)))
            .ok_or_else(|| invalid(path, "non-finite float64 result")),
        (ValueType::Any | ValueType::String, CelValue::String(value)) => {
            Ok(ValueRef::new(ValueKind::String(Arc::clone(value))))
        }
        (ValueType::Any | ValueType::Array | ValueType::List(_), CelValue::List(items)) => {
            let inner = match value_type {
                ValueType::List(inner) => inner.as_ref(),
                _ => &ValueType::Any,
            };
            budget.consume(items.len(), path)?;
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    cel_to_json_at(item, inner, budget, &Path::Index(path, index), depth + 1)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(ValueRef::array)
        }
        (ValueType::Any | ValueType::Object | ValueType::Map(_), CelValue::Map(entries)) => {
            let inner = match value_type {
                ValueType::Map(inner) => inner.as_ref(),
                _ => &ValueType::Any,
            };
            budget.consume(entries.len(), path)?;
            let mut result = Vec::new();
            for (key, item) in entries.iter() {
                let MapKey::String(key) = key else {
                    return Err(invalid(path, "map key must be a string"));
                };
                let path = Path::Key(path, key);
                result.push((
                    Arc::clone(key),
                    cel_to_json_at(item, inner, budget, &path, depth + 1)?,
                ));
            }
            Ok(ValueRef::object(result))
        }
        _ => Err(invalid(
            path,
            format!("expected {value_type}, found {}", actual_cel_type(value)),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numeric_inputs_convert_only_when_lossless() {
        for (descriptor, input, expected) in [
            (ValueType::Int64, json!(42.0), json!(42)),
            (ValueType::Float64, json!(42), json!(42.0)),
            (ValueType::Uint64, json!(42.0), json!(42)),
            (ValueType::Uint64, json!(u64::MAX), json!(u64::MAX)),
        ] {
            let native =
                json_to_cel(&input.into(), &descriptor, &mut Budget::default(), "", 1).unwrap();
            assert_eq!(
                cel_to_json(&native, &descriptor, &mut Budget::default(), "", 1).unwrap(),
                expected
            );
        }
        for (descriptor, input) in [
            (ValueType::Int64, json!(1.5)),
            (ValueType::Int64, json!(-0.0)),
            (ValueType::Int64, json!(i64::MAX as f64)),
            (ValueType::Float64, json!((1_u64 << 53) + 1)),
            (ValueType::Float64, json!(u64::MAX)),
            (ValueType::Uint64, json!(-1)),
            (ValueType::Uint64, json!(0.5)),
            (ValueType::Uint64, json!(-0.0)),
            (ValueType::Uint64, json!(u64::MAX as f64)),
        ] {
            assert!(
                json_to_cel(&input.into(), &descriptor, &mut Budget::default(), "", 1).is_err()
            );
        }
    }

    #[test]
    fn round_trips_nested_json_through_typed_cel_values() {
        let value_type = ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))));
        let original = json!([{"a/b": 1}, {"a~b": 2}]);
        let cel = json_to_cel(
            &ValueRef::from(original.clone()),
            &value_type,
            &mut Budget::default(),
            "",
            1,
        )
        .unwrap();
        let result = cel_to_json(&cel, &value_type, &mut Budget::default(), "", 1).unwrap();
        assert_eq!(result, original);
    }

    #[test]
    fn round_trips_dynamic_json_without_losing_number_representations() {
        let original = json!({
            "records": [{"id": "call", "path": [{"index": 0}], "usage": null}, {}],
            "mixed": [true, null, "text", [], {}],
            "numbers": [i64::MIN, (1_u64 << 53) + 1, u64::MAX, 42.0, -0.0]
        });
        for descriptor in [
            ValueType::Any,
            ValueType::Object,
            ValueType::Map(Box::new(ValueType::Any)),
        ] {
            let native = json_to_cel(
                &original.clone().into(),
                &descriptor,
                &mut Budget::default(),
                "",
                1,
            )
            .unwrap();
            let result = cel_to_json(&native, &descriptor, &mut Budget::default(), "", 1).unwrap();
            assert_eq!(result, original);
            assert_eq!(
                result["numbers"][4].as_f64().unwrap().to_bits(),
                (-0.0_f64).to_bits()
            );
            assert!(serde_json::to_value(&result).unwrap()["numbers"][3].is_f64());
            assert!(result["records"][1].as_object().unwrap().is_empty());
        }
        for descriptor in [
            ValueType::Any,
            ValueType::Array,
            ValueType::List(Box::new(ValueType::Any)),
        ] {
            let original = original["mixed"].clone();
            let native = json_to_cel(
                &original.clone().into(),
                &descriptor,
                &mut Budget::default(),
                "",
                1,
            )
            .unwrap();
            assert_eq!(
                cel_to_json(&native, &descriptor, &mut Budget::default(), "", 1).unwrap(),
                original
            );
        }
    }

    #[test]
    fn rejects_nested_dynamic_non_json_values_with_paths_and_sources() {
        use std::error::Error;
        for value in [
            CelValue::Double(f64::INFINITY),
            CelValue::Bytes(Arc::from(b"bytes".as_slice())),
            CelValue::timestamp(0, 0),
            CelValue::duration(1, 0),
            CelValue::map([(1_i64, CelValue::Null)]),
            CelValue::error(cel_core::EvalError::division_by_zero()),
        ] {
            let is_error = matches!(value, CelValue::Error(_));
            let nested = CelValue::list([CelValue::map([("a~/b", value)])]);
            let error =
                cel_to_json(&nested, &ValueType::Any, &mut Budget::default(), "", 1).unwrap_err();
            assert!(error.to_string().contains("/0/a~0~1b"), "{error}");
            if is_error {
                assert!(error.source().unwrap().is::<cel_core::EvalError>());
            }
        }
    }

    #[test]
    fn dynamic_descendants_share_depth_and_entry_budgets() {
        let mut json = json!(null);
        let mut cel = CelValue::Null;
        for depth in 1..=ValueType::MAX_DEPTH + 1 {
            let input = json_to_cel(
                &json.clone().into(),
                &ValueType::Any,
                &mut Budget::default(),
                "",
                1,
            );
            let output = cel_to_json(&cel, &ValueType::Any, &mut Budget::default(), "", 1);
            if depth <= ValueType::MAX_DEPTH {
                assert!(input.is_ok());
                assert!(output.is_ok());
            } else {
                assert!(input.unwrap_err().to_string().contains("nesting depth"));
                assert!(output.unwrap_err().to_string().contains("nesting depth"));
            }
            json = json!({"nested": json});
            cel = CelValue::map([("nested", cel)]);
        }
        for count in [MAX_COLLECTION_ENTRIES / 2, MAX_COLLECTION_ENTRIES / 2 + 1] {
            let input = json!(vec![serde_json::Value::Null; count]);
            let mut budget = Budget::default();
            let native = json_to_cel(&input.into(), &ValueType::Any, &mut budget, "", 1).unwrap();
            let result = cel_to_json(&native, &ValueType::Any, &mut budget, "", 1);
            if count == MAX_COLLECTION_ENTRIES / 2 {
                assert!(result.is_ok());
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("collection entry limit")
                );
            }
        }
    }

    #[test]
    fn preserves_root_prefix_and_escaped_paths_in_both_directions() {
        let value_type = ValueType::List(Box::new(ValueType::Map(Box::new(ValueType::Int64))));
        let value = ValueRef::from(json!([{"ok": 1}, {"a~/b": false}]));
        let error =
            json_to_cel(&value, &value_type, &mut Budget::default(), "root", 1).unwrap_err();
        assert_eq!(error.to_string(), "path `root/1/a~0~1b`: expected int64");
        let value = CelValue::list([
            CelValue::map([("ok", CelValue::Int(1))]),
            CelValue::map([("a~/b", CelValue::Bool(false))]),
        ]);
        let error =
            cel_to_json(&value, &value_type, &mut Budget::default(), "root", 1).unwrap_err();
        assert_eq!(
            error.to_string(),
            "path `root/1/a~0~1b`: expected int64, found boolean"
        );
    }

    #[test]
    fn rejects_non_json_cel_results_and_non_string_map_keys() {
        assert!(
            cel_to_json(
                &CelValue::Double(f64::NAN),
                &ValueType::Float64,
                &mut Budget::default(),
                "",
                1
            )
            .unwrap_err()
            .to_string()
            .contains("non-finite")
        );
        assert!(
            cel_to_json(
                &CelValue::map([(1i64, CelValue::Int(2))]),
                &ValueType::Map(Box::new(ValueType::Int64)),
                &mut Budget::default(),
                "",
                1
            )
            .unwrap_err()
            .to_string()
            .contains("map key must be a string")
        );
        assert!(
            cel_to_json(
                &CelValue::Bool(true),
                &ValueType::Int64,
                &mut Budget::default(),
                "",
                1
            )
            .unwrap_err()
            .to_string()
            .contains("expected int64")
        );
    }

    #[test]
    fn bounds_collection_entries_and_nested_depth() {
        let oversized = json!(vec![1; MAX_COLLECTION_ENTRIES + 1]);
        let value_type = ValueType::List(Box::new(ValueType::Int64));
        assert!(
            json_to_cel(
                &ValueRef::from(oversized),
                &value_type,
                &mut Budget::default(),
                "",
                1
            )
            .unwrap_err()
            .to_string()
            .contains("collection entry limit")
        );
        let mut nested_type = ValueType::Int64;
        let mut nested_value = json!(1);
        for _ in 0..ValueType::MAX_DEPTH {
            nested_type = ValueType::List(Box::new(nested_type));
            nested_value = json!([nested_value]);
        }
        assert!(
            json_to_cel(
                &ValueRef::from(nested_value),
                &nested_type,
                &mut Budget::default(),
                "",
                1
            )
            .unwrap_err()
            .to_string()
            .contains("nesting depth")
        );
    }
}
