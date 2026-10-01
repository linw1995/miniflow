use cel_core::{MapKey, Value as CelValue};
use mf_runtime::{ValueKind, ValueRef, ValueType};
use serde_json::Number;
use std::sync::Arc;

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_COLLECTION_ENTRIES: usize = 10_000;

#[derive(Default)]
pub struct Budget {
    entries: usize,
}

impl Budget {
    fn consume(&mut self, count: usize, path: &str) -> Result<(), String> {
        if count > MAX_COLLECTION_ENTRIES - self.entries {
            return Err(format!(
                "path `{path}` exceeds the collection entry limit of {MAX_COLLECTION_ENTRIES}"
            ));
        }
        self.entries += count;
        Ok(())
    }
}

fn child_path(path: &str, segment: &str) -> String {
    format!("{path}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

fn check_depth(depth: usize, path: &str) -> Result<(), String> {
    if depth > ValueType::MAX_DEPTH {
        Err(format!(
            "path `{path}` exceeds nesting depth {}",
            ValueType::MAX_DEPTH
        ))
    } else {
        Ok(())
    }
}

pub fn json_to_cel(
    value: &ValueRef,
    value_type: &ValueType,
    budget: &mut Budget,
    path: &str,
    depth: usize,
) -> Result<CelValue, String> {
    check_depth(depth, path)?;
    match (value_type, value.kind()) {
        (ValueType::Null, ValueKind::Null) => Ok(CelValue::Null),
        (ValueType::Boolean, ValueKind::Bool(value)) => Ok(CelValue::Bool(*value)),
        (ValueType::Int64, ValueKind::Number(value)) => value
            .as_i64()
            .map(CelValue::Int)
            .ok_or_else(|| format!("path `{path}`: expected int64")),
        (ValueType::Float64, ValueKind::Number(value)) if value.is_f64() => value
            .as_f64()
            .filter(|number| number.is_finite())
            .map(CelValue::Double)
            .ok_or_else(|| format!("path `{path}`: expected finite float64")),
        (ValueType::String, ValueKind::String(value)) => Ok(CelValue::String(Arc::clone(value))),
        (ValueType::List(inner), ValueKind::Array(items)) => {
            budget.consume(items.len(), path)?;
            let values = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    json_to_cel(
                        item,
                        inner,
                        budget,
                        &child_path(path, &index.to_string()),
                        depth + 1,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CelValue::list(values))
        }
        (ValueType::Map(inner), ValueKind::Object(entries)) => {
            budget.consume(entries.len(), path)?;
            let values = entries
                .iter()
                .map(|(key, item)| {
                    json_to_cel(item, inner, budget, &child_path(path, key), depth + 1)
                        .map(|converted| (Arc::clone(key), converted))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CelValue::map(values))
        }
        _ => Err(format!("path `{path}`: expected {value_type}")),
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
) -> Result<ValueRef, String> {
    check_depth(depth, path)?;
    match (value_type, value) {
        (ValueType::Null, CelValue::Null) => Ok(ValueRef::null()),
        (ValueType::Boolean, CelValue::Bool(value)) => Ok(ValueRef::from(*value)),
        (ValueType::Int64, CelValue::Int(value)) => Ok(ValueRef::from(*value)),
        (ValueType::Float64, CelValue::Double(value)) => Number::from_f64(*value)
            .map(|number| ValueRef::new(ValueKind::Number(number)))
            .ok_or_else(|| format!("path `{path}`: non-finite float64 result")),
        (ValueType::String, CelValue::String(value)) => {
            Ok(ValueRef::new(ValueKind::String(Arc::clone(value))))
        }
        (ValueType::List(inner), CelValue::List(items)) => {
            budget.consume(items.len(), path)?;
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    cel_to_json(
                        item,
                        inner,
                        budget,
                        &child_path(path, &index.to_string()),
                        depth + 1,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(ValueRef::array)
        }
        (ValueType::Map(inner), CelValue::Map(entries)) => {
            budget.consume(entries.len(), path)?;
            let mut result = Vec::new();
            for (key, item) in entries.iter() {
                let MapKey::String(key) = key else {
                    return Err(format!("path `{path}`: map key must be a string"));
                };
                let path = child_path(path, key);
                result.push((
                    Arc::clone(key),
                    cel_to_json(item, inner, budget, &path, depth + 1)?,
                ));
            }
            Ok(ValueRef::object(result))
        }
        _ => Err(format!(
            "path `{path}`: expected {value_type}, found {}",
            actual_cel_type(value)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            .contains("nesting depth")
        );
    }
}
