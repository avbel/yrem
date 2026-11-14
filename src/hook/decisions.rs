//! The briefing's "Live decisions", picked client-side.
//!
//! The server's `top_decisions` is the store's all-time highest importance, the same few
//! rows in every project. `memory list` is ordered by insertion and does not reinforce, so
//! the newest rows of the project, plus whatever is recent anywhere, make a list that
//! follows the work.

use serde_json::Value;

use super::gate::{self, Gate};

/// Rows fetched when listing every namespace, and the project's own namespace.
pub const FETCH_RECENT: u32 = 100;
pub const FETCH_PROJECT: u32 = 50;
/// Rows from other namespaces that still make the list, when this recent.
const RECENT_SECS: f64 = 7.0 * 86_400.0;
const RECENT_OTHER: usize = 2;

/// One decision line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub rid: String,
    pub date: Option<String>,
    pub text: String,
    /// What a server merge was made from; empty for any other row.
    pub sources: Vec<String>,
}

/// `records` of a `memory list` answer.
#[must_use]
pub fn records(answer: &Value) -> Vec<Value> {
    answer
        .get("records")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Picks up to `limit` decisions of the project, then the recent rows of other namespaces.
///
/// `recent` is a listing of every namespace, `in_project` one of the project's namespace.
/// A project row is one whose namespace or text names the project. Without a project every
/// pick is a recent row. Rows are ordered by `created_at`: corrected and imported rows keep
/// an old date under a new rid. `excluded` holds rids the caller found unfit to show.
#[must_use]
pub fn select(
    recent: &[Value],
    in_project: &[Value],
    gate: &Gate,
    limit: usize,
    now: f64,
    excluded: &[String],
) -> Vec<Decision> {
    let recent = usable(recent, gate, excluded);
    let mut mine: Vec<&Value> = recent
        .iter()
        .copied()
        .filter(|r| gate.is_project_hit(r))
        .chain(usable(in_project, gate, excluded))
        .collect();
    mine.sort_by(|a, b| created(b).total_cmp(&created(a)));
    let mut chosen: Vec<&Value> = Vec::new();
    for row in mine {
        if chosen.len() < limit && !chosen.iter().any(|c| rid(c) == rid(row)) {
            chosen.push(row);
        }
    }
    let others = if gate.project.is_some() {
        RECENT_OTHER
    } else {
        limit
    };
    let extra: Vec<&Value> = recent
        .iter()
        .copied()
        .filter(|r| {
            now - created(r) <= RECENT_SECS
                && !gate.is_project_hit(r)
                && !chosen.iter().any(|c| rid(c) == rid(r))
        })
        .take(others)
        .collect();
    chosen.into_iter().chain(extra).map(decision).collect()
}

/// Rows a briefing may show, newest first.
fn usable<'a>(rows: &'a [Value], gate: &Gate, excluded: &[String]) -> Vec<&'a Value> {
    let mut rows: Vec<&Value> = rows
        .iter()
        .filter(|r| {
            !rid(r).is_empty()
                && text(r).is_some()
                && gate.eligible(r).is_none()
                && !excluded.iter().any(|e| e == rid(r))
        })
        .collect();
    rows.sort_by(|a, b| created(b).total_cmp(&created(a)));
    rows
}

fn decision(row: &Value) -> Decision {
    Decision {
        rid: rid(row).to_owned(),
        date: row
            .get("created_at_iso")
            .and_then(Value::as_str)
            .map(|s| s.get(..10).unwrap_or(s).to_owned()),
        text: text(row).unwrap_or_default().to_owned(),
        sources: gate::merge_sources(row),
    }
}

fn rid(row: &Value) -> &str {
    row.get("rid").and_then(Value::as_str).unwrap_or_default()
}

fn text(row: &Value) -> Option<&str> {
    row.get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
}

fn created(row: &Value) -> f64 {
    row.get("created_at").and_then(Value::as_f64).unwrap_or(0.0)
}
