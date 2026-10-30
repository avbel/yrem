#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use yrem::mcp::{compact, compact_schema};
use yrem::ops::RecallArgs;

fn compacted(mut v: Value) -> Value {
    compact(&mut v);
    v
}

#[test]
fn removes_schema_and_title_recursively() {
    let v = compacted(json!({
        "$schema": "x", "title": "T",
        "properties": {"a": {"title": "A", "type": "string"}},
        "items": [{"title": "I"}]
    }));
    assert_eq!(
        v,
        json!({"properties": {"a": {"type": "string"}}, "items": [{}]})
    );
}

#[test]
fn nullable_type_collapses() {
    assert_eq!(
        compacted(json!({"type": ["string", "null"]})),
        json!({"type": "string"})
    );
    assert_eq!(
        compacted(json!({"type": ["null", "array"]})),
        json!({"type": "array"})
    );
    // Several non-null types stay as they are.
    assert_eq!(
        compacted(json!({"type": ["string", "integer", "null"]})),
        json!({"type": ["string", "integer", "null"]})
    );
}

#[test]
fn numeric_format_removed() {
    assert_eq!(
        compacted(json!({"type": "integer", "format": "uint32", "minimum": 0})),
        json!({"type": "integer", "minimum": 0})
    );
    assert_eq!(
        compacted(json!({"type": ["number", "null"], "format": "double"})),
        json!({"type": "number"})
    );
    // Non-numeric formats are kept.
    assert_eq!(
        compacted(json!({"type": "string", "format": "date-time"})),
        json!({"type": "string", "format": "date-time"})
    );
}

#[test]
fn null_default_removed_other_defaults_kept() {
    assert_eq!(
        compacted(json!({"type": "string", "default": null})),
        json!({"type": "string"})
    );
    assert_eq!(
        compacted(json!({"type": "integer", "default": 0})),
        json!({"type": "integer", "default": 0})
    );
}

#[test]
fn any_of_ref_or_null_collapses() {
    assert_eq!(
        compacted(json!({"anyOf": [{"$ref": "#/$defs/Order"}, {"type": "null"}]})),
        json!({"$ref": "#/$defs/Order"})
    );
    assert_eq!(
        compacted(json!({"description": "d", "anyOf": [{"type": "null"}, {"$ref": "#/$defs/X"}]})),
        json!({"description": "d", "$ref": "#/$defs/X"})
    );
}

#[test]
fn other_any_of_untouched() {
    let v = json!({"anyOf": [{"type": "string"}, {"type": "integer"}]});
    assert_eq!(compacted(v.clone()), v);
    let w = json!({"anyOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}]});
    assert_eq!(compacted(w.clone()), w);
}

#[test]
fn recall_schema_is_compact() {
    let schema = compact_schema::<RecallArgs>();
    let text = serde_json::to_string(&*schema).unwrap();
    assert!(!text.contains("\"title\""), "{text}");
    assert!(!text.contains("$schema"), "{text}");
    assert!(!text.contains("null"), "{text}");
    assert_eq!(schema["required"], json!(["query"]));
    assert_eq!(schema["type"], "object");
}
