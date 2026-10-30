//! Secret guard for write requests: a hand-written scanner over every string in the JSON body.
//!
//! Findings carry kind and location only, never the matched text.

mod detectors;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// Pointers whose values are identifiers: exempt from the entropy detector only.
const ENTROPY_EXEMPT: &[&str] = &[
    "/idempotency_key",
    "/rid",
    "/source_memory_rid",
    "/metadata/corrects",
];

const MAX_LISTED: usize = 5;

/// One secret-like match. `start` / `end` are char offsets inside the string at `path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub kind: &'static str,
    pub path: String,
    pub start: usize,
    pub end: usize,
}

/// Scans every string leaf of `value`, and object keys that name a credential.
#[must_use]
pub fn scan(value: &Value) -> Vec<Finding> {
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

fn walk(value: &Value, path: &str, out: &mut Vec<Finding>) {
    match value {
        Value::String(s) => {
            let skip_entropy = ENTROPY_EXEMPT.contains(&path);
            out.extend(
                detectors::scan_text(s, skip_entropy)
                    .into_iter()
                    .map(|h| Finding {
                        kind: h.kind,
                        path: path.to_owned(),
                        start: h.start,
                        end: h.end,
                    }),
            );
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{path}/{i}"), out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                let before = out.len();
                walk(item, &child, out);
                if let Value::String(s) = item
                    && out.len() == before
                    && detectors::is_credential_name(key)
                    && detectors::is_credential_value(s)
                {
                    out.push(Finding {
                        kind: "credential_field",
                        path: child,
                        start: 0,
                        end: s.chars().count(),
                    });
                }
            }
        }
        _ => {}
    }
}

/// Refuses a write whose serialized `request` looks like it holds a secret.
///
/// `force` waives every finding: the caller takes responsibility for the data.
pub fn guard(op: &str, force: bool, request: &impl Serialize) -> Result<()> {
    if force {
        return Ok(());
    }
    // Request structs only have string keys, so serialization cannot fail.
    let value = serde_json::to_value(request).unwrap_or_default();
    let findings = scan(&value);
    if findings.is_empty() {
        Ok(())
    } else {
        Err(Error::Sensitive {
            op: op.to_owned(),
            findings,
        })
    }
}

/// Single-line refusal text, at most five findings listed.
#[must_use]
pub fn refusal(op: &str, findings: &[Finding]) -> String {
    let mut parts: Vec<String> = findings
        .iter()
        .take(MAX_LISTED)
        .map(|f| format!("{} at {}[{}..{}]", f.kind, f.path, f.start, f.end))
        .collect();
    if findings.len() > MAX_LISTED {
        parts.push(format!("+{} more", findings.len() - MAX_LISTED));
    }
    format!(
        "{op} refused: possible secret(s), nothing stored. {}. Redact them (e.g. \"<redacted>\") \
         and retry; if this is not secret data, retry with force=true.",
        parts.join("; ")
    )
}
