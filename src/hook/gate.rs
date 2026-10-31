//! Relevance gate for recalled rows, ported from the yantrikdb hook plugins.
//!
//! Measured on a real store (2026-10): most injected rows were auto-captured prompts or
//! consolidation merges of them, and blended scores sat in 0.3-0.65 for relevant and
//! irrelevant hits alike. The semantic-lane cosine from `why_retrieved` separates them:
//! with MiniLM-384 a 0.50 floor kept 13/18 known answers and silenced 12/12 irrelevant
//! prompts.

use serde_json::Value;

const AUTO_CAPTURE: &str = "session_auto_capture";
/// Merges of curated facts were never shorter than 825 chars; merges of prompts had a
/// median of 194.
const MERGE_MIN_CHARS: usize = 600;
const MERGE_MIN_SEGMENT: usize = 160;
const MIN_SCORE: f64 = 0.10;

/// Gate settings.
#[derive(Debug, Clone)]
pub struct Gate {
    pub min_similarity: f64,
    pub include_captured: bool,
    pub excluded_namespaces: Vec<String>,
}

impl Gate {
    /// Why `hit` must not be injected, or `None` when it may be.
    #[must_use]
    pub fn reject(&self, hit: &Value) -> Option<&'static str> {
        let namespace = hit.get("namespace").and_then(Value::as_str).unwrap_or("");
        if self
            .excluded_namespaces
            .iter()
            .any(|p| namespace.starts_with(p.as_str()))
        {
            return Some("namespace");
        }
        if hit.get("score").and_then(Value::as_f64).unwrap_or(0.0) < MIN_SCORE {
            return Some("score");
        }
        if !self.include_captured {
            let meta = metadata(hit);
            if hit.get("source").and_then(Value::as_str) == Some(AUTO_CAPTURE)
                || meta.get("kind").and_then(Value::as_str) == Some(AUTO_CAPTURE)
            {
                return Some("auto-capture");
            }
            let text = hit.get("text").and_then(Value::as_str).unwrap_or("");
            if is_prompt_merge(text, &meta) {
                return Some("prompt-merge");
            }
        }
        if self.min_similarity > 0.0 && similarity(hit).is_none_or(|s| s < self.min_similarity) {
            return Some("similarity");
        }
        None
    }
}

/// Cosine of the semantic lane, or `None` when only keyword or graph lanes matched.
#[must_use]
pub fn similarity(hit: &Value) -> Option<f64> {
    hit.get("why_retrieved")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find_map(|s| {
            s.strip_prefix("semantically similar (")?
                .split(')')
                .next()?
                .parse()
                .ok()
        })
}

fn metadata(hit: &Value) -> Value {
    match hit.get("metadata") {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    }
}

/// Consolidation relabels merges of captured prompts as `source=user`; their shape gives
/// them away.
fn is_prompt_merge(text: &str, meta: &Value) -> bool {
    let merged = meta
        .get("consolidated_from")
        .is_some_and(|v| !v.is_null() && v != &Value::Bool(false) && v != &Value::Array(vec![]));
    if !merged {
        return false;
    }
    let mut lens: Vec<usize> = text
        .split(" | ")
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.chars().count())
        .collect();
    if lens.is_empty() {
        lens.push(text.chars().count());
    }
    lens.sort_unstable();
    let mid = lens.len() / 2;
    let median2 = if lens.len().is_multiple_of(2) {
        lens[mid - 1] + lens[mid]
    } else {
        2 * lens[mid]
    };
    text.chars().count() < MERGE_MIN_CHARS || median2 < 2 * MERGE_MIN_SEGMENT
}
