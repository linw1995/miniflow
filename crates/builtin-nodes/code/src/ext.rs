use crate::value::cel_to_json;
use cel_core::{CelType, EvalError, FunctionDecl, OverloadDecl, Value};
use mf_runtime::ValueType;
use std::{collections::HashSet, sync::Arc};

pub fn list_extension() -> Vec<FunctionDecl> {
    vec![
        slice_decl(),
        flatten_decl(),
        distinct_decl(),
        sort_decl(),
        sum_decl(),
    ]
}

pub fn json_extension() -> Vec<FunctionDecl> {
    vec![FunctionDecl::new("json.encode").with_overload(
        OverloadDecl::function("json_encode", vec![CelType::Dyn], CelType::String).with_impl(
            |args| match cel_to_json(&args[0], &ValueType::Any, "", 1) {
                Ok(value) => match serde_json::to_string(&value) {
                    Ok(encoded) => Value::String(Arc::from(encoded)),
                    Err(error) => Value::error(EvalError::invalid_argument(error.to_string())),
                },
                Err(error) => Value::error(EvalError::invalid_argument(error.to_string())),
            },
        ),
    )]
}

fn slice_decl() -> FunctionDecl {
    let list_t = CelType::list(CelType::type_param("T"));
    FunctionDecl::new("slice").with_overload(
        OverloadDecl::method(
            "list_slice_int_int",
            vec![list_t.clone(), CelType::Int, CelType::Int],
            list_t,
        )
        .with_type_params(vec!["T".to_owned()])
        .with_impl(|args| {
            let Value::List(list) = &args[0] else {
                return Value::error(EvalError::invalid_argument("expected list"));
            };
            let Value::Int(start) = &args[1] else {
                return Value::error(EvalError::invalid_argument("expected int start index"));
            };
            let Value::Int(end) = &args[2] else {
                return Value::error(EvalError::invalid_argument("expected int end index"));
            };
            let len = list.len() as i64;
            if *start < 0 || *start > len {
                return Value::error(EvalError::range_error(format!(
                    "slice start index out of range: {start}"
                )));
            }
            if *end < 0 || *end > len {
                return Value::error(EvalError::range_error(format!(
                    "slice end index out of range: {end}"
                )));
            }
            if *start > *end {
                return Value::error(EvalError::range_error(format!(
                    "invalid slice range: start {start} > end {end}"
                )));
            }
            Value::list(list[*start as usize..*end as usize].iter().cloned())
        }),
    )
}

fn flatten_decl() -> FunctionDecl {
    let list_dyn = CelType::list(CelType::Dyn);
    FunctionDecl::new("flatten").with_overload(
        OverloadDecl::method("list_flatten", vec![list_dyn.clone()], list_dyn).with_impl(|args| {
            let Value::List(outer) = &args[0] else {
                return Value::error(EvalError::invalid_argument("expected list"));
            };
            let mut total = 0usize;
            for item in outer.iter() {
                let Value::List(inner) = item else {
                    return Value::error(EvalError::invalid_argument(
                        "flatten() requires list elements to be lists",
                    ));
                };
                total += inner.len();
            }
            let mut flat = Vec::with_capacity(total);
            for item in outer.iter() {
                if let Value::List(inner) = item {
                    flat.extend(inner.iter().cloned());
                }
            }
            Value::list(flat)
        }),
    )
}

fn distinct_decl() -> FunctionDecl {
    let list_t = CelType::list(CelType::type_param("T"));
    FunctionDecl::new("distinct").with_overload(
        OverloadDecl::method("list_distinct", vec![list_t.clone()], list_t)
            .with_type_params(vec!["T".to_owned()])
            .with_impl(|args| {
                let Value::List(list) = &args[0] else {
                    return Value::error(EvalError::invalid_argument("expected list"));
                };
                if list.iter().all(|item| matches!(item, Value::String(_))) {
                    let mut seen = HashSet::with_capacity(list.len());
                    let mut unique = Vec::with_capacity(list.len());
                    for item in list.iter() {
                        if let Value::String(text) = item
                            && seen.insert(text.as_ref())
                        {
                            unique.push(item.clone());
                        }
                    }
                    return Value::list(unique);
                }
                if list.iter().all(|item| matches!(item, Value::Int(_))) {
                    let mut seen = HashSet::with_capacity(list.len());
                    let mut unique = Vec::with_capacity(list.len());
                    for item in list.iter() {
                        if let Value::Int(number) = item
                            && seen.insert(*number)
                        {
                            unique.push(item.clone());
                        }
                    }
                    return Value::list(unique);
                }
                let mut unique: Vec<Value> = Vec::with_capacity(list.len());
                for item in list.iter() {
                    if !unique.iter().any(|existing| existing == item) {
                        unique.push(item.clone());
                    }
                }
                Value::list(unique)
            }),
    )
}

fn sort_list(args: &[Value]) -> Value {
    let Value::List(list) = &args[0] else {
        return Value::error(EvalError::invalid_argument("expected list"));
    };
    let Some(first) = list.first() else {
        return Value::list(Vec::<Value>::new());
    };
    match first {
        Value::Int(_) => {
            let mut values = Vec::with_capacity(list.len());
            for item in list.iter() {
                let Value::Int(number) = item else {
                    return Value::error(EvalError::invalid_argument(
                        "sort() requires homogeneous element types",
                    ));
                };
                values.push(*number);
            }
            values.sort_unstable();
            Value::list(values.into_iter().map(Value::Int))
        }
        Value::UInt(_) => {
            let mut values = Vec::with_capacity(list.len());
            for item in list.iter() {
                let Value::UInt(number) = item else {
                    return Value::error(EvalError::invalid_argument(
                        "sort() requires homogeneous element types",
                    ));
                };
                values.push(*number);
            }
            values.sort_unstable();
            Value::list(values.into_iter().map(Value::UInt))
        }
        Value::Double(_) => {
            let mut values = Vec::with_capacity(list.len());
            for item in list.iter() {
                let Value::Double(number) = item else {
                    return Value::error(EvalError::invalid_argument(
                        "sort() requires homogeneous element types",
                    ));
                };
                if number.is_nan() {
                    return Value::error(EvalError::invalid_argument("cannot sort NaN"));
                }
                values.push(*number);
            }
            values.sort_by(f64::total_cmp);
            Value::list(values.into_iter().map(Value::Double))
        }
        Value::String(_) => {
            let mut values = Vec::with_capacity(list.len());
            for item in list.iter() {
                let Value::String(text) = item else {
                    return Value::error(EvalError::invalid_argument(
                        "sort() requires homogeneous element types",
                    ));
                };
                values.push(Arc::clone(text));
            }
            values.sort();
            Value::list(values.into_iter().map(Value::String))
        }
        Value::Bool(_) => {
            let mut values = Vec::with_capacity(list.len());
            for item in list.iter() {
                let Value::Bool(flag) = item else {
                    return Value::error(EvalError::invalid_argument(
                        "sort() requires homogeneous element types",
                    ));
                };
                values.push(*flag);
            }
            values.sort_unstable();
            Value::list(values.into_iter().map(Value::Bool))
        }
        _ => Value::error(EvalError::invalid_argument(
            "sort() supports lists of int, uint, double, string, or bool",
        )),
    }
}

fn sort_decl() -> FunctionDecl {
    FunctionDecl::new("sort")
        .with_overload(
            OverloadDecl::method(
                "list_int_sort",
                vec![CelType::list(CelType::Int)],
                CelType::list(CelType::Int),
            )
            .with_impl(sort_list),
        )
        .with_overload(
            OverloadDecl::method(
                "list_uint_sort",
                vec![CelType::list(CelType::UInt)],
                CelType::list(CelType::UInt),
            )
            .with_impl(sort_list),
        )
        .with_overload(
            OverloadDecl::method(
                "list_double_sort",
                vec![CelType::list(CelType::Double)],
                CelType::list(CelType::Double),
            )
            .with_impl(sort_list),
        )
        .with_overload(
            OverloadDecl::method(
                "list_string_sort",
                vec![CelType::list(CelType::String)],
                CelType::list(CelType::String),
            )
            .with_impl(sort_list),
        )
        .with_overload(
            OverloadDecl::method(
                "list_bool_sort",
                vec![CelType::list(CelType::Bool)],
                CelType::list(CelType::Bool),
            )
            .with_impl(sort_list),
        )
}

fn sum_decl() -> FunctionDecl {
    FunctionDecl::new("sum").with_overload(
        OverloadDecl::method(
            "list_int_sum",
            vec![CelType::list(CelType::Int)],
            CelType::Int,
        )
        .with_impl(|args| {
            let Value::List(list) = &args[0] else {
                return Value::error(EvalError::invalid_argument("expected list"));
            };
            let mut total = 0i64;
            for item in list.iter() {
                let Value::Int(number) = item else {
                    return Value::error(EvalError::invalid_argument("sum() requires list of int"));
                };
                let Some(next) = total.checked_add(*number) else {
                    return Value::error(EvalError::overflow("integer overflow in sum()"));
                };
                total = next;
            }
            Value::Int(total)
        }),
    )
}
