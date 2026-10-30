//! Output shaping and printing. Stdout carries only JSON.

use std::io::{IsTerminal, Write};

use serde_json::{Map, Value};

use crate::time;

/// Prunes nulls, rounds floats to 4 dp and adds `<key>_iso` next to unix timestamps.
#[must_use]
pub fn shape(v: Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(shape_object(o)),
        Value::Array(a) => Value::Array(a.into_iter().map(shape).collect()),
        Value::Number(n) if !n.is_i64() && !n.is_u64() => n
            .as_f64()
            .and_then(|f| serde_json::Number::from_f64((f * 1e4).round() / 1e4))
            .map_or(Value::Null, Value::Number),
        other => other,
    }
}

fn shape_object(o: Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    let mut isos = Vec::new();
    for (k, v) in o {
        if v.is_null() {
            continue;
        }
        if is_timestamp_key(&k)
            && let Some(f) = v.as_f64()
            && let Some(iso) = time::iso(f)
        {
            isos.push((format!("{k}_iso"), iso));
            out.insert(k, v);
            continue;
        }
        out.insert(k, shape(v));
    }
    for (k, iso) in isos {
        out.entry(k).or_insert(Value::String(iso));
    }
    out
}

fn is_timestamp_key(k: &str) -> bool {
    k.ends_with("_at") || matches!(k, "last_access" | "last_seen")
}

/// Pretty or compact JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Pretty,
    Compact,
}

impl Format {
    /// `--pretty` / `--compact` win; otherwise pretty on a terminal, compact when piped.
    #[must_use]
    pub fn detect(pretty: bool, compact: bool) -> Self {
        if pretty {
            Self::Pretty
        } else if compact || !std::io::stdout().is_terminal() {
            Self::Compact
        } else {
            Self::Pretty
        }
    }

    #[must_use]
    pub fn render(self, v: &Value) -> String {
        match self {
            Self::Pretty => serde_json::to_string_pretty(v),
            Self::Compact => serde_json::to_string(v),
        }
        .unwrap_or_else(|_| "null".into())
    }
}

/// Writes one JSON document and a newline to stdout.
pub fn print(v: &Value, format: Format) {
    print_raw(&format.render(v));
}

/// Writes a line to stdout, ignoring a closed pipe.
pub fn print_raw(s: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}
