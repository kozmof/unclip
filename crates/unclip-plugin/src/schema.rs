//! Validation of plugin parameters against a descriptor's declared schema.
//!
//! Every descriptor carries a `params_schema` string. Until it was checked, that
//! string was documentation: each plugin separately re-stated its parameter
//! contract as a serde struct, and nothing held the two together. A schema that
//! claimed a field was required while serde defaulted it, or listed a field the
//! struct had since renamed, was indistinguishable from a correct one.
//!
//! [`validate_params`] makes the schema authoritative for the part it can
//! decide: shape, required keys, unknown keys, and the value constraints the
//! descriptors actually use. A plugin still deserializes its own parameters, so
//! this does not replace that step — it fails first, and names the offending
//! key, which a serde error for a nested rename often cannot.
//!
//! # Supported subset
//!
//! `type` (`object`/`array`/`string`/`number`/`integer`/`boolean`/`null`),
//! `properties`, `required`, `additionalProperties` (boolean form), `items`,
//! `enum`, `minLength`, `minimum`, `maximum`, `exclusiveMinimum`, and `oneOf`.
//! Anything else in a schema is ignored rather than rejected, so an unsupported
//! keyword weakens the check instead of failing a valid plugin. [`check_schema`]
//! reports a schema that is malformed or that relies on nothing this understands.

use serde_json::Value;

/// A parameter value that does not satisfy the declared schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct SchemaViolation {
    /// JSON-pointer-ish location, e.g. `sequence/0/position`.
    pub path: String,
    pub reason: String,
}

fn violation(path: &str, reason: impl Into<String>) -> SchemaViolation {
    SchemaViolation {
        path: if path.is_empty() { "params" } else { path }.to_owned(),
        reason: reason.into(),
    }
}

/// Check `params` against `schema`, which is the descriptor's `params_schema`.
///
/// A schema that does not parse is reported as a violation at the root rather
/// than silently accepting everything: a descriptor that cannot state its
/// contract should not be treated as having no constraints.
pub fn validate_params(schema: &str, params: &Value) -> Result<(), SchemaViolation> {
    let parsed: Value = serde_json::from_str(schema)
        .map_err(|error| violation("", format!("declared schema is not valid JSON: {error}")))?;
    validate(&parsed, params, "")
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn matches_type(expected: &str, value: &Value) -> bool {
    match expected {
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        other => other == type_name(value),
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}/{key}")
    }
}

fn validate(schema: &Value, value: &Value, path: &str) -> Result<(), SchemaViolation> {
    let Some(schema) = schema.as_object() else {
        // `true`/`{}` accept anything, which is what an absent constraint means.
        return Ok(());
    };

    // `oneOf` succeeds when exactly one branch does, so it is checked on its own
    // and its branches' failures are not reported individually: which branch was
    // "meant" is not knowable here.
    if let Some(Value::Array(branches)) = schema.get("oneOf") {
        let matched = branches
            .iter()
            .filter(|branch| validate(branch, value, path).is_ok())
            .count();
        if matched != 1 {
            return Err(violation(
                path,
                format!(
                    "value matches {matched} of {} alternatives, expected exactly 1",
                    branches.len()
                ),
            ));
        }
    }

    if let Some(Value::String(expected)) = schema.get("type") {
        if !matches_type(expected, value) {
            return Err(violation(
                path,
                format!("expected {expected}, found {}", type_name(value)),
            ));
        }
    }

    if let Some(Value::Array(allowed)) = schema.get("enum") {
        if !allowed.contains(value) {
            let names = allowed
                .iter()
                .map(|option| option.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(violation(path, format!("must be one of [{names}]")));
        }
    }

    if let Some(text) = value.as_str() {
        if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64) {
            if (text.chars().count() as u64) < minimum {
                return Err(violation(
                    path,
                    format!("must be at least {minimum} character(s) long"),
                ));
            }
        }
    }

    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                return Err(violation(path, format!("must be at least {minimum}")));
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                return Err(violation(path, format!("must be at most {maximum}")));
            }
        }
        if let Some(limit) = schema.get("exclusiveMinimum").and_then(Value::as_f64) {
            if number <= limit {
                return Err(violation(path, format!("must be greater than {limit}")));
            }
        }
    }

    if let Some(items) = schema.get("items") {
        if let Some(values) = value.as_array() {
            for (index, item) in values.iter().enumerate() {
                validate(items, item, &join(path, &index.to_string()))?;
            }
        }
    }

    let Some(object) = value.as_object() else {
        return Ok(());
    };

    if let Some(Value::Array(required)) = schema.get("required") {
        for name in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(name) {
                return Err(violation(path, format!("missing required key `{name}`")));
            }
        }
    }

    let properties = schema.get("properties").and_then(Value::as_object);
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        for name in object.keys() {
            if !properties.is_some_and(|declared| declared.contains_key(name)) {
                return Err(violation(path, format!("unknown key `{name}`")));
            }
        }
    }
    if let Some(properties) = properties {
        for (name, subschema) in properties {
            if let Some(present) = object.get(name) {
                validate(subschema, present, &join(path, name))?;
            }
        }
    }

    Ok(())
}

/// A declared schema that cannot serve as a parameter contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MalformedSchema(pub String);

/// Check that a declared schema is well formed enough to constrain anything.
///
/// This is what keeps a descriptor's schema from decaying into a comment. It
/// requires an object schema to say whether it accepts unknown keys and to list
/// every `required` name among its `properties` — a `required` name that is not
/// a declared property can never be satisfied by a value the schema also
/// accepts, so it is always a mistake rather than a strict contract.
pub fn check_schema(schema: &str) -> Result<(), MalformedSchema> {
    let parsed: Value = serde_json::from_str(schema)
        .map_err(|error| MalformedSchema(format!("schema is not valid JSON: {error}")))?;
    check(&parsed, "")
}

fn check(schema: &Value, path: &str) -> Result<(), MalformedSchema> {
    let Some(object) = schema.as_object() else {
        return Ok(());
    };
    if let Some(Value::Array(branches)) = object.get("oneOf") {
        for (index, branch) in branches.iter().enumerate() {
            check(branch, &join(path, &index.to_string()))?;
        }
        return Ok(());
    }
    let declares_object = object.get("type") == Some(&Value::String("object".into()))
        || object.contains_key("properties");
    let properties = object.get("properties").and_then(Value::as_object);
    if declares_object && !object.contains_key("additionalProperties") {
        return Err(MalformedSchema(format!(
            "{}: object schema must state `additionalProperties`",
            if path.is_empty() { "params" } else { path }
        )));
    }
    if let Some(Value::Array(required)) = object.get("required") {
        for name in required.iter().filter_map(Value::as_str) {
            if !properties.is_some_and(|declared| declared.contains_key(name)) {
                return Err(MalformedSchema(format!(
                    "{}: `{name}` is required but not a declared property",
                    if path.is_empty() { "params" } else { path }
                )));
            }
        }
    }
    if let Some(properties) = properties {
        for (name, subschema) in properties {
            check(subschema, &join(path, name))?;
        }
    }
    if let Some(items) = object.get("items") {
        check(items, &join(path, "items"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SENSOR: &str = r#"{"type":"object","additionalProperties":false,
        "required":["left","right"],
        "properties":{
            "left":{"type":"string","minLength":1},
            "right":{"type":"string","minLength":1},
            "foreground_rank":{"type":"integer","minimum":1},
            "conditioning_variables":{"type":"array","items":{"type":"string","minLength":1}}
        }}"#;

    fn reject(params: Value) -> SchemaViolation {
        validate_params(SENSOR, &params).expect_err("params should violate the schema")
    }

    #[test]
    fn accepts_parameters_that_satisfy_the_schema() {
        assert!(validate_params(SENSOR, &json!({"left": "a", "right": "b"})).is_ok());
        assert!(validate_params(
            SENSOR,
            &json!({"left":"a","right":"b","foreground_rank":3,
                    "conditioning_variables":["c"]})
        )
        .is_ok());
    }

    #[test]
    fn names_the_offending_key_rather_than_only_failing() {
        assert_eq!(
            reject(json!({"left": "a"})).reason,
            "missing required key `right`"
        );
        assert_eq!(
            reject(json!({"left":"a","right":"b","typo":1})).reason,
            "unknown key `typo`"
        );
        let deep = reject(json!({"left":"a","right":"b","conditioning_variables":["ok", ""]}));
        assert_eq!(deep.path, "conditioning_variables/1");
        assert_eq!(deep.reason, "must be at least 1 character(s) long");
    }

    #[test]
    fn enforces_numeric_and_type_constraints() {
        assert_eq!(
            reject(json!({"left":"a","right":"b","foreground_rank":0})).reason,
            "must be at least 1"
        );
        assert_eq!(
            reject(json!({"left":"a","right":"b","foreground_rank":"3"})).reason,
            "expected integer, found string"
        );
        assert_eq!(reject(json!({"left": 1, "right": "b"})).path, "left");
    }

    #[test]
    fn an_empty_object_schema_constrains_nothing() {
        assert!(validate_params("{}", &json!({"anything": 1})).is_ok());
    }

    #[test]
    fn a_schema_that_does_not_parse_is_a_violation_not_a_free_pass() {
        let error = validate_params("{not json", &json!({})).expect_err("must not accept");
        assert_eq!(error.path, "params");
        assert!(
            error.reason.contains("not valid JSON"),
            "got: {}",
            error.reason
        );
    }

    #[test]
    fn exclusive_minimum_and_enum_are_honored() {
        let schema = r#"{"type":"object","additionalProperties":false,
            "properties":{"tolerance":{"type":"number","exclusiveMinimum":0},
                          "method":{"enum":["communities","spectral"]}}}"#;
        assert!(validate_params(schema, &json!({"tolerance": 0.5})).is_ok());
        assert_eq!(
            validate_params(schema, &json!({"tolerance": 0.0}))
                .unwrap_err()
                .reason,
            "must be greater than 0"
        );
        assert!(validate_params(schema, &json!({"method": "spectral"})).is_ok());
        assert!(validate_params(schema, &json!({"method": "other"})).is_err());
    }

    #[test]
    fn one_of_requires_exactly_one_matching_alternative() {
        let schema = r#"{"oneOf":[{"type":"string"},{"type":"integer"}]}"#;
        assert!(validate_params(schema, &json!("a")).is_ok());
        assert!(validate_params(schema, &json!(1)).is_ok());
        assert!(validate_params(schema, &json!(true)).is_err());
    }

    #[test]
    fn check_schema_rejects_contracts_that_cannot_constrain() {
        assert!(check_schema(SENSOR).is_ok());
        // An object schema that does not say whether unknown keys are allowed.
        assert!(check_schema(r#"{"type":"object","properties":{"a":{"type":"string"}}}"#).is_err());
        // A required name that is not a declared property can never be satisfied.
        let error = check_schema(
            r#"{"type":"object","additionalProperties":false,"required":["b"],
                "properties":{"a":{"type":"string"}}}"#,
        )
        .expect_err("unsatisfiable requirement must be rejected");
        assert!(error.0.contains("`b` is required"), "got: {}", error.0);
        assert!(check_schema("{oops").is_err());
    }

    #[test]
    fn a_free_form_object_declares_itself_free_form() {
        // `additionalProperties: true` is how a pass-through payload states that
        // its keys belong to someone else's vocabulary.
        let schema = r#"{"type":"object","additionalProperties":true}"#;
        assert!(check_schema(schema).is_ok());
        assert!(validate_params(schema, &json!({"temperature": 0})).is_ok());
    }
}
