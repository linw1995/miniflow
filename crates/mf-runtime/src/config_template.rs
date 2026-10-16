use crate::{ContextReference, ContextValue, ExecutionContext, ValueRef};
use serde_json::Value;
use snafu::{OptionExt, ResultExt, Snafu, ensure};

/// An explicitly selected configuration expression, independent of node contracts.
#[derive(Debug)]
pub enum ConfigTemplate {
    Literal(ValueRef),
    Reference { output: String, path: String },
    Interpolation(Vec<ConfigTemplate>),
}

#[derive(Debug, Snafu)]
pub enum ConfigTemplateError {
    #[snafu(display("invalid configuration template: {requirement}"))]
    Syntax { requirement: &'static str },
    #[snafu(display("configuration reference `{output}` is unavailable: {source}"))]
    Context {
        output: String,
        source: crate::NodeExecutionError,
    },
    #[snafu(display("configuration reference `{output}` was skipped"))]
    Skipped { output: String },
    #[snafu(display("configuration reference `{output}` has no value at `{path}`"))]
    MissingPath { output: String, path: String },
    #[snafu(display("configuration interpolation requires string values"))]
    InterpolationType,
}

impl ConfigTemplate {
    pub fn parse(value: Value) -> Result<Self, ConfigTemplateError> {
        let Some(object) = value.as_object() else {
            return Ok(Self::Literal(value.into()));
        };
        if object.contains_key("$ref") {
            ensure!(
                object
                    .keys()
                    .all(|key| matches!(key.as_str(), "$ref" | "path")),
                SyntaxSnafu {
                    requirement: "references accept only $ref and path"
                }
            );
            let output = object["$ref"].as_str().context(SyntaxSnafu {
                requirement: "$ref must be a nonempty output identifier",
            })?;
            let path = object.get("path").map_or(Ok(""), |value| {
                value.as_str().context(SyntaxSnafu {
                    requirement: "path must be a JSON Pointer string",
                })
            })?;
            return Self::reference(output, path);
        }
        if let Some(template) = object.get("$template") {
            ensure!(
                object.len() == 1,
                SyntaxSnafu {
                    requirement: "interpolation accepts only $template",
                }
            );
            let template = template.as_str().context(SyntaxSnafu {
                requirement: "$template must be a string",
            })?;
            let mut parts = Vec::new();
            let mut rest = template;
            while let Some(start) = rest.find("{{") {
                parts.push(Self::Literal(rest[..start].into()));
                rest = &rest[start + 2..];
                let end = rest.find("}}").context(SyntaxSnafu {
                    requirement: "interpolation references require closing braces",
                })?;
                let reference = rest[..end].trim();
                ensure!(
                    !reference.contains(['{', '}']),
                    SyntaxSnafu {
                        requirement: "interpolation references cannot contain braces"
                    }
                );
                let (output, path) = reference.split_once('#').unwrap_or((reference, ""));
                parts.push(Self::reference(output, path)?);
                rest = &rest[end + 2..];
            }
            parts.push(Self::Literal(rest.into()));
            return Ok(Self::Interpolation(parts));
        }
        Ok(Self::Literal(value.into()))
    }

    fn reference(output: &str, path: &str) -> Result<Self, ConfigTemplateError> {
        ensure!(
            !output.is_empty(),
            SyntaxSnafu {
                requirement: "references require a nonempty output identifier",
            }
        );
        ensure!(
            path.is_empty() || path.starts_with('/'),
            SyntaxSnafu {
                requirement: "path must be an empty string or start with /",
            }
        );
        let mut bytes = path.bytes();
        while let Some(byte) = bytes.next() {
            ensure!(
                byte != b'~' || matches!(bytes.next(), Some(b'0' | b'1')),
                SyntaxSnafu {
                    requirement: "JSON Pointer escapes must be ~0 or ~1",
                }
            );
        }
        Ok(Self::Reference {
            output: output.into(),
            path: path.into(),
        })
    }

    pub fn literal(&self) -> Option<&ValueRef> {
        match self {
            Self::Literal(value) => Some(value),
            _ => None,
        }
    }

    pub fn references(&self, label: &str) -> Vec<ContextReference> {
        match self {
            Self::Literal(_) => Vec::new(),
            Self::Reference { output, .. } => vec![ContextReference::new(output, label)],
            Self::Interpolation(parts) => parts
                .iter()
                .flat_map(|part| part.references(label))
                .collect(),
        }
    }

    pub fn resolve(&self, context: &ExecutionContext) -> Result<ValueRef, ConfigTemplateError> {
        self.evaluate(
            &|output| match context.output(output).context(ContextSnafu { output })? {
                ContextValue::Value(value) => Ok(value.clone()),
                ContextValue::Skipped => SkippedSnafu { output }.fail(),
            },
        )
    }

    fn evaluate(
        &self,
        lookup: &impl Fn(&str) -> Result<ValueRef, ConfigTemplateError>,
    ) -> Result<ValueRef, ConfigTemplateError> {
        match self {
            Self::Literal(value) => Ok(value.clone()),
            Self::Reference { output, path } => {
                let value = lookup(output)?;
                value
                    .pointer(path)
                    .cloned()
                    .context(MissingPathSnafu { output, path })
            }
            Self::Interpolation(parts) => {
                let values = parts
                    .iter()
                    .map(|part| part.evaluate(lookup))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut result = String::new();
                for value in &values {
                    result.push_str(value.as_str().context(InterpolationTypeSnafu)?);
                }
                Ok(ValueRef::from(result).inherit_secrets(&values))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn references_preserve_types_subtrees_and_sensitivity() {
        let source = ValueRef::object([
            (
                "a/b".into(),
                ValueRef::array([ValueRef::from(42).into_secret()]),
            ),
            ("public".into(), ValueRef::from("visible")),
        ]);
        let template =
            ConfigTemplate::parse(json!({"$ref": "params.settings", "path": "/a~1b/0"})).unwrap();
        let value = template.evaluate(&|_| Ok(source.clone())).unwrap();
        assert_eq!(value.as_i64(), Some(42));
        assert!(value.is_secret());
        assert!(value.ptr_eq(&source["a/b"][0]));
        assert_eq!(
            template.references("max_tokens"),
            vec![ContextReference::new("params.settings", "max_tokens")]
        );
        let whole = ConfigTemplate::parse(json!({"$ref": "params.settings"})).unwrap();
        assert!(
            whole
                .evaluate(&|_| Ok(source.clone()))
                .unwrap()
                .ptr_eq(&source)
        );
        let public =
            ConfigTemplate::parse(json!({"$ref": "params.settings", "path": "/public"})).unwrap();
        assert!(
            !public
                .evaluate(&|_| Ok(source.clone()))
                .unwrap()
                .contains_secret()
        );
    }

    #[test]
    fn interpolation_inherits_secrets_without_implicit_string_conversion() {
        let template =
            ConfigTemplate::parse(json!({"$template": "Bearer {{ params.token }}!"})).unwrap();
        let value = template
            .evaluate(&|_| Ok(ValueRef::from("synthetic-key").into_secret()))
            .unwrap();
        assert_eq!(value.as_str(), Some("Bearer synthetic-key!"));
        assert!(value.is_secret());
        assert!(!format!("{value:?}").contains("synthetic-key"));
        assert!(matches!(
            template.evaluate(&|_| Ok(42.into())),
            Err(ConfigTemplateError::InterpolationType)
        ));
        let nested =
            ConfigTemplate::parse(json!({"$template": "{{ params.object#/name }}"})).unwrap();
        assert_eq!(
            nested
                .evaluate(&|_| Ok(json!({"name":"chosen"}).into()))
                .unwrap()
                .as_str(),
            Some("chosen")
        );
        let literal = ConfigTemplate::parse(json!("{{ params.token }}")).unwrap();
        assert_eq!(
            literal.literal().unwrap().as_str(),
            Some("{{ params.token }}")
        );
        let object = json!({"label": "{{ params.token }}", "count": 42});
        let literal = ConfigTemplate::parse(object.clone()).unwrap();
        assert_eq!(
            literal.resolve(&ExecutionContext::default()).unwrap(),
            object
        );
    }

    #[test]
    fn malformed_or_unavailable_references_fail_without_resolved_payloads() {
        for value in [
            json!({"$ref": ""}),
            json!({"$ref": 1}),
            json!({"$ref": "x", "other": true}),
            json!({"$ref": "x", "path": null}),
            json!({"$ref": "x", "path": "key"}),
            json!({"$ref": "x", "path": "/~2"}),
            json!({"$ref": "x", "path": "/~"}),
            json!({"$template": 1}),
            json!({"$template": "{{ x"}),
            json!({"$template": "{{ }}"}),
            json!({"$template": "{{ invalid{name }}"}),
            json!({"$template": "x", "extra": 1}),
        ] {
            assert!(ConfigTemplate::parse(value.clone()).is_err(), "{value}");
        }
        let template =
            ConfigTemplate::parse(json!({"$ref": "params.key", "path": "/missing"})).unwrap();
        let error = template
            .evaluate(&|_| Ok(ValueRef::from("synthetic-key").into_secret()))
            .unwrap_err();
        assert!(matches!(error, ConfigTemplateError::MissingPath { .. }));
        assert!(!format!("{error:?}").contains("synthetic-key"));
        assert!(matches!(
            template.resolve(&ExecutionContext::default()),
            Err(ConfigTemplateError::Context { .. })
        ));
        let mut context = ExecutionContext::default();
        let source = crate::FlowNode {
            definition_id: "params".into(),
            node: (),
            metadata: crate::NodePorts {
                inputs: vec![],
                outputs: vec![crate::PortSpec::new("key", crate::ValueType::String, false)],
            }
            .into(),
        };
        crate::execute_generated_node_in_context(&source, [], &mut context, &[], |_, _| {
            Ok(crate::GeneratedNodeResult::Dynamic(crate::NodeResult {
                skipped: ["key".into()].into(),
                ..Default::default()
            }))
        })
        .unwrap();
        assert!(matches!(
            template.resolve(&context),
            Err(ConfigTemplateError::Skipped { .. })
        ));
    }
}
