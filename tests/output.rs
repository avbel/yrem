#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use serde_json::{Value, json};
use yrem::output::{Format, shape};

#[test]
fn prunes_nulls_recursively() {
    let v = json!({
        "a": null,
        "b": {"c": null, "d": 1},
        "e": [{"f": null, "g": "x"}, {"h": null}],
        "i": [null, 1]
    });
    assert_eq!(
        shape(v),
        json!({"b": {"d": 1}, "e": [{"g": "x"}, {}], "i": [null, 1]})
    );
}

#[test]
fn rounds_floats_to_four_places() {
    let out = shape(json!({"score": 0.123_456_789, "n": [1.99999, 2.5]}));
    assert_eq!(out, json!({"score": 0.1235, "n": [2.0, 2.5]}));
}

#[test]
fn integers_untouched() {
    let out = shape(json!({"a": 5, "b": -3, "c": u64::MAX, "d": i64::MIN}));
    assert_eq!(out, json!({"a": 5, "b": -3, "c": u64::MAX, "d": i64::MIN}));
}

#[test]
fn adds_iso_for_timestamp_keys() {
    let out = shape(json!({
        "created_at": 1_767_323_045.0,
        "last_access": 0,
        "last_seen": 86_400.5,
        "other": 1_767_323_045.0,
        "nested": [{"updated_at": 1}],
    }));
    assert_eq!(out["created_at_iso"], "2026-01-02T03:04:05Z");
    assert_eq!(out["last_access_iso"], "1970-01-01T00:00:00Z");
    assert_eq!(out["last_seen_iso"], "1970-01-02T00:00:00Z");
    assert!(out.get("other_iso").is_none());
    assert_eq!(out["nested"][0]["updated_at_iso"], "1970-01-01T00:00:01Z");
}

#[test]
fn timestamp_keys_with_non_numeric_values_get_no_iso() {
    let out = shape(json!({"created_at": "yesterday", "seen_at": true}));
    assert!(out.get("created_at_iso").is_none());
    assert!(out.get("seen_at_iso").is_none());
    assert_eq!(out["created_at"], "yesterday");
}

#[test]
fn existing_iso_not_overwritten() {
    let out = shape(json!({"created_at": 0, "created_at_iso": "custom"}));
    assert_eq!(out["created_at_iso"], "custom");
}

#[test]
fn scalars_pass_through() {
    assert_eq!(shape(json!("s")), json!("s"));
    assert_eq!(shape(json!(true)), json!(true));
    assert_eq!(shape(Value::Null), Value::Null);
}

#[test]
fn render_pretty_vs_compact() {
    let v = json!({"a": [1, 2]});
    assert_eq!(Format::Compact.render(&v), r#"{"a":[1,2]}"#);
    let pretty = Format::Pretty.render(&v);
    assert!(pretty.contains('\n'));
    assert!(pretty.contains("  \"a\""));
    assert_eq!(serde_json::from_str::<Value>(&pretty).unwrap(), v);
}

#[test]
fn detect_flags_and_pipe() {
    assert_eq!(Format::detect(true, false), Format::Pretty);
    assert_eq!(Format::detect(true, true), Format::Pretty);
    assert_eq!(Format::detect(false, true), Format::Compact);
    // Under `cargo test` stdout is captured, never a terminal.
    assert_eq!(Format::detect(false, false), Format::Compact);
}
