//! Small helpers for writing JSON Schema by hand.
//!
//! Tool schemas are the contract the model codes against, and a wrong one
//! shows up as a confidently malformed tool call rather than a type error.
//! These builders keep the shapes consistent and the call sites readable.

use serde_json::{json, Map, Value};

/// Build an object schema.
///
/// `properties` are `(name, schema)` pairs; `required` names the mandatory
/// ones. `additionalProperties` is always false so a model that invents an
/// argument gets a validation failure it can see and correct, rather than
/// having the argument silently dropped.
pub fn object(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut map = Map::new();
    for (name, schema) in properties {
        map.insert(name.to_string(), schema);
    }
    json!({
        "type": "object",
        "properties": Value::Object(map),
        "required": required,
        "additionalProperties": false,
    })
}

/// A string field.
pub fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

/// A whole-number field.
///
/// Worth reaching for whenever the handler will do arithmetic with the value.
/// Declaring a number as a string invites the model to send one, and a parser
/// that only accepts numbers then drops it silently.
pub fn integer(description: &str) -> Value {
    json!({"type": "integer", "description": description, "minimum": 1})
}

/// A string field constrained to a fixed set of values.
pub fn enumerated(description: &str, values: &[&str]) -> Value {
    json!({"type": "string", "description": description, "enum": values})
}

/// A boolean field.
pub fn boolean(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

/// An array of strings.
pub fn string_array(description: &str) -> Value {
    json!({
        "type": "array",
        "description": description,
        "items": {"type": "string"},
    })
}

/// An array of objects.
pub fn object_array(description: &str, items: Value) -> Value {
    json!({"type": "array", "description": description, "items": items})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_reject_unknown_properties() {
        let schema = object(vec![("task", string("the task id"))], &["task"]);
        assert_eq!(
            schema["additionalProperties"], false,
            "a hallucinated argument must fail loudly, not be dropped"
        );
        assert_eq!(schema["required"][0], "task");
        assert_eq!(schema["properties"]["task"]["type"], "string");
    }

    #[test]
    fn enumerated_fields_carry_their_allowed_values() {
        let schema = enumerated("how urgent", &["low", "normal"]);
        assert_eq!(schema["enum"][1], "normal");
    }

    #[test]
    fn arrays_describe_their_item_type() {
        assert_eq!(string_array("names")["items"]["type"], "string");
    }
}
