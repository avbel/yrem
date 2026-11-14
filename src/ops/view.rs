//! What operations add around the server's answer: recall snippets, `remember` warnings and
//! the `sensitive` metadata flag.

use serde_json::{Map, Value, json};

use crate::client::RememberRequest;
use crate::hook::gate::similarity;
use crate::hook::render::clip;

const SNIPPET_CHARS: usize = 300;
const LONG_TEXT_CHARS: usize = 1_500;
/// A stored memory at least this similar is the same fact, written again.
const DUPLICATE_SIMILARITY: f64 = 0.92;
pub(super) const DUPLICATE_QUERY_CHARS: usize = 1_000;
const SNIPPET_FIELDS: [&str; 6] = [
    "rid",
    "namespace",
    "score",
    "created_at_iso",
    "memory_type",
    "why_retrieved",
];

/// Replaces each hit of a recall answer with its identity, score and a text snippet.
pub(super) fn snippets(mut out: Value) -> Value {
    if let Some(Value::Array(hits)) = out.get_mut("results") {
        for hit in hits {
            *hit = snippet_hit(hit);
        }
    }
    out
}

fn snippet_hit(hit: &Value) -> Value {
    let mut slim: Map<String, Value> = SNIPPET_FIELDS
        .iter()
        .filter_map(|k| Some(((*k).to_owned(), hit.get(*k)?.clone())))
        .collect();
    let text = hit.get("text").and_then(Value::as_str).unwrap_or_default();
    let snippet = clip(text, SNIPPET_CHARS);
    slim.insert(
        "truncated".into(),
        (snippet != clip(text, usize::MAX)).into(),
    );
    slim.insert("text".into(), snippet.into());
    Value::Object(slim)
}

/// One warning per text over the length where recall stops being useful.
pub(super) fn length_warnings(texts: &[&str], batch: bool) -> Vec<String> {
    texts
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let chars = t.chars().count();
            let at = if batch {
                format!("memories[{i}]: ")
            } else {
                String::new()
            };
            (chars > LONG_TEXT_CHARS).then(|| {
                format!("{at}text is {chars} characters (over {LONG_TEXT_CHARS}): split into separate facts")
            })
        })
        .collect()
}

/// Warning for a recall hit that is nearly the text being written.
pub(super) fn duplicate_warning(hit: &Value) -> Option<String> {
    let cosine = similarity(hit).filter(|s| *s >= DUPLICATE_SIMILARITY)?;
    let rid = hit.get("rid")?.as_str().filter(|r| !r.is_empty())?;
    Some(format!(
        "similar memory {rid} is already stored (cosine {cosine:.2}): use correct to update it \
         instead of adding a duplicate"
    ))
}

pub(super) fn with_warnings(mut out: Value, warnings: &[String]) -> Value {
    if !warnings.is_empty()
        && let Value::Object(o) = &mut out
    {
        o.insert("warnings".into(), json!(warnings));
    }
    out
}

pub(super) fn is_sensitive(req: &RememberRequest) -> bool {
    req.metadata.as_ref().and_then(|m| m.get("sensitive")) == Some(&Value::Bool(true))
}

/// Adds `sensitive: true` to the metadata when asked; an explicit `false` leaves it alone.
pub(super) fn flag_sensitive(
    metadata: Option<Map<String, Value>>,
    sensitive: Option<bool>,
) -> Option<Map<String, Value>> {
    if sensitive != Some(true) {
        return metadata;
    }
    let mut metadata = metadata.unwrap_or_default();
    metadata.insert("sensitive".into(), true.into());
    Some(metadata)
}

/// Memory metadata as an object: the server sends it as an object or as a JSON string.
pub(super) fn metadata_object(metadata: Value) -> Map<String, Value> {
    match metadata {
        Value::Object(m) => m,
        Value::String(s) => serde_json::from_str(&s).unwrap_or_default(),
        _ => Map::new(),
    }
}

/// The keys `think` writes on a merge. A correction is a new fact, not a merge of the old ones.
const CONSOLIDATION_KEYS: [&str; 4] = [
    "consolidated_from",
    "evidence_ids",
    "cluster_size",
    "consolidation_time",
];
const CONSOLIDATION_PREFIXES: [&str; 2] = ["evidence_span_", "first_mention_"];

/// `metadata` without what a consolidation left on it.
pub(super) fn without_consolidation(mut metadata: Map<String, Value>) -> Map<String, Value> {
    metadata.retain(|k, _| {
        !CONSOLIDATION_KEYS.contains(&k.as_str())
            && !CONSOLIDATION_PREFIXES.iter().any(|p| k.starts_with(p))
    });
    metadata
}
