//! Tool arguments: the accepted fields of a tool, the JSON Schema advertised
//! for them, and validation of what a client actually sent.
//!
//! Only the small JSON-Schema subset this repo authors is understood: objects
//! with string, number, boolean, and string-array properties.

use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Kind {
    String,
    Number,
    Boolean,
    StringArray,
    Integer { min: i64, max: i64 },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Field {
    name: String,
    kind: Kind,
    description: Option<String>,
    required: bool,
    default: Option<Value>,
}

impl Field {
    pub(super) fn optional(name: &str, kind: Kind, description: &str) -> Self {
        Self {
            name: name.to_string(),
            kind,
            description: Some(description.to_string()),
            required: false,
            default: None,
        }
    }

    pub(super) fn with_default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }

    fn schema(&self) -> Value {
        let mut schema = match self.kind {
            Kind::String => json!({ "type": "string" }),
            Kind::Number => json!({ "type": "number" }),
            Kind::Boolean => json!({ "type": "boolean" }),
            Kind::StringArray => json!({ "type": "array", "items": { "type": "string" } }),
            Kind::Integer { min, max } => {
                json!({ "type": "integer", "minimum": min, "maximum": max })
            }
        };
        if let Some(description) = &self.description {
            schema["description"] = json!(description);
        }
        if let Some(default) = &self.default {
            schema["default"] = default.clone();
        }
        schema
    }

    fn check(&self, value: &Value) -> Result<(), String> {
        let expected = match self.kind {
            Kind::String if value.is_string() => return Ok(()),
            Kind::Number if value.is_number() => return Ok(()),
            Kind::Boolean if value.is_boolean() => return Ok(()),
            Kind::StringArray
                if value
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string)) =>
            {
                return Ok(())
            }
            Kind::Integer { min, max } => match value.as_f64() {
                Some(number)
                    if number.fract() == 0.0 && number >= min as f64 && number <= max as f64 =>
                {
                    return Ok(())
                }
                _ => {
                    return Err(format!(
                        "{}: expected an integer from {min} to {max}",
                        self.name
                    ))
                }
            },
            Kind::String => "a string",
            Kind::Number => "a number",
            Kind::Boolean => "a boolean",
            Kind::StringArray => "an array of strings",
        };
        Err(format!("{}: expected {expected}", self.name))
    }
}

/// The fields a manifest request schema accepts.
pub(super) fn fields(schema: &Value) -> Vec<Field> {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema["properties"].as_object() else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, property)| Field {
            name: name.clone(),
            kind: match property["type"].as_str() {
                Some("array") => Kind::StringArray,
                Some("number") => Kind::Number,
                Some("boolean") => Kind::Boolean,
                _ => Kind::String,
            },
            description: property["description"].as_str().map(str::to_string),
            required: required.contains(&name.as_str()),
            default: property.get("default").cloned(),
        })
        .collect()
}

/// The JSON Schema a client is shown. `None` is a tool that takes no
/// arguments; a tool with fields rejects nothing it does not name, but says
/// so by closing the object.
pub(super) fn json_schema(fields: Option<&[Field]>) -> Map<String, Value> {
    let Some(fields) = fields else {
        return object(json!({ "type": "object", "properties": {} }));
    };
    let properties: Map<String, Value> = fields
        .iter()
        .map(|field| (field.name.clone(), field.schema()))
        .collect();
    // A field with a default is never required: the default fills it in.
    let required: Vec<&str> = fields
        .iter()
        .filter(|field| field.required && field.default.is_none())
        .map(|field| field.name.as_str())
        .collect();
    let mut schema = json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
        "$schema": "http://json-schema.org/draft-07/schema#",
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    object(schema)
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Checks `arguments` against `fields` and returns the values a tool may
/// use: known fields only, with defaults filled in. Arguments the tool does
/// not name are dropped, never forwarded.
pub(super) fn parse(
    fields: &[Field],
    arguments: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let mut values = Map::new();
    let mut problems = Vec::new();
    for field in fields {
        match (arguments.get(&field.name), &field.default) {
            (Some(value), _) => match field.check(value) {
                Ok(()) => {
                    // Normalize integral floats so integer query parameters remain parseable.
                    let value = if matches!(field.kind, Kind::Integer { .. }) {
                        json!(value.as_f64().expect("validated number") as i64)
                    } else {
                        value.clone()
                    };
                    values.insert(field.name.clone(), value);
                }
                Err(problem) => problems.push(problem),
            },
            (None, Some(default)) => {
                values.insert(field.name.clone(), default.clone());
            }
            (None, None) if field.required => problems.push(format!("{}: required", field.name)),
            (None, None) => {}
        }
    }
    if problems.is_empty() {
        Ok(values)
    } else {
        Err(problems.join("; "))
    }
}

/// The validated arguments of one tool call.
pub(super) struct Args<'a> {
    values: Map<String, Value>,
    default_profile: &'a str,
}

impl<'a> Args<'a> {
    pub(super) fn new(values: Map<String, Value>, default_profile: &'a str) -> Self {
        Self {
            values,
            default_profile,
        }
    }

    pub(super) fn values(&self) -> &Map<String, Value> {
        &self.values
    }

    pub(super) fn value(&self, name: &str) -> Option<Value> {
        self.values.get(name).cloned()
    }

    /// The argument as it appears in a URL.
    pub(super) fn text(&self, name: &str) -> Option<String> {
        match self.values.get(name)? {
            Value::String(text) => Some(text.clone()),
            other => Some(other.to_string()),
        }
    }

    /// The named profile, or the configured default when the call names none.
    pub(super) fn profile(&self) -> String {
        self.text("profile")
            .unwrap_or_else(|| self.default_profile.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Field> {
        fields(&json!({
            "type": "object",
            "properties": {
                "work_id": { "type": "string", "description": "Work item." },
                "wait": { "type": "boolean", "default": true },
                "tags": { "type": "array", "items": { "type": "string" } },
            },
            "required": ["work_id"],
        }))
    }

    #[test]
    fn parse_fills_defaults_and_drops_unknown_arguments() {
        let arguments = object(json!({ "work_id": "#1", "extra": 1 }));
        assert_eq!(
            parse(&sample(), &arguments).unwrap(),
            object(json!({ "work_id": "#1", "wait": true }))
        );
    }

    #[test]
    fn parse_reports_missing_and_mistyped_arguments() {
        let arguments = object(json!({ "tags": ["a", 1], "wait": "yes" }));
        assert_eq!(
            parse(&sample(), &arguments).unwrap_err(),
            "tags: expected an array of strings; wait: expected a boolean; work_id: required"
        );
    }

    #[test]
    fn integer_arguments_accept_integral_numbers_and_reject_fractions() {
        let fields = vec![Field::optional(
            "days",
            Kind::Integer { min: 1, max: 90 },
            "",
        )];
        for days in [json!(30), json!(30.0), json!(1.0), json!(90.0)] {
            let parsed = parse(&fields, &object(json!({ "days": days }))).unwrap();
            assert!(parsed["days"].as_i64().is_some());
        }
        for days in [
            json!(30.5),
            json!(0.5),
            json!(90.5),
            json!(0.0),
            json!(91.0),
            json!("30"),
        ] {
            assert!(parse(&fields, &object(json!({ "days": days }))).is_err());
        }
    }

    #[test]
    fn json_schema_closes_the_object_and_lists_required_fields() {
        let schema = Value::Object(json_schema(Some(&sample())));
        assert_eq!(schema["required"], json!(["work_id"]));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(
            schema["properties"]["wait"],
            json!({ "type": "boolean", "default": true })
        );
        assert_eq!(
            Value::Object(json_schema(None)),
            json!({ "type": "object", "properties": {} })
        );
    }
}
