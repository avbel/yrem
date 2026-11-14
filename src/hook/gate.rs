//! Relevance gate for recalled rows, ported from the yantrikdb hook plugins and made
//! project-aware.
//!
//! Measured on a real store (2026-10): most injected rows were auto-captured prompts or
//! consolidation merges of them, and blended scores sat in 0.3-0.65 for relevant and
//! irrelevant hits alike. The semantic-lane cosine from `why_retrieved` separates them:
//! with MiniLM-384 a 0.50 floor kept 13/18 known answers and silenced 12/12 irrelevant
//! prompts.
//!
//! That floor alone dropped relevant keyword hits ("what is the saleroom OTP flow" found 4
//! Saleroom memories at score 0.30-0.35 and injected none) and kept off-project semantic
//! ones (cosine 0.51-0.56). Replaying 26 real prompts (2026-11, `skip_reinforce`):
//!
//! | Variant                                                | Added useful | Added noise | Removed noise |
//! |--------------------------------------------------------|--------------|-------------|---------------|
//! | keyword + 2 lanes + score 0.30, anywhere               | 6 prompts    | 9 hits      | 0             |
//! | the same, only for hits of the current project         | 6 prompts    | 0           | 0             |
//! | plus cosine 0.60 for hits that do not name the project | -            | 0           | 7             |
//!
//! The last row is what this gate does. It loses one marginal hit (cosine 0.54, a Vaultwarden
//! note for a vaultkeeper prompt).
//!
//! Re-run on the live store over 26 prompts (23 real, 3 probes), top 3 injected, counting lanes
//! the way the gate does (graph lane left out): the 0.50 floor injected for 16 prompts (34
//! hits, 7 off-project), this gate for 19 (47 hits, 0 off-project). Counting the graph lane as
//! the server's "multi-lane agreement" does would have added one hit (keyword plus graph
//! only). About 67% of top-10 hits carry a graph reason, mostly through generic words (`API`,
//! `POST`), so that lane says little about agreement.

use serde_json::Value;

const AUTO_CAPTURE: &str = "session_auto_capture";
/// Merges of curated facts were never shorter than 825 chars; merges of prompts had a
/// median of 194.
const MERGE_MIN_CHARS: usize = 600;
const MERGE_MIN_SEGMENT: usize = 160;
const MIN_SCORE: f64 = 0.10;
/// A project hit with no semantic match needs this score and agreement from two lanes.
const KEYWORD_MIN_SCORE: f64 = 0.30;
const KEYWORD_MIN_LANES: u32 = 2;
/// A shorter project name ("sui", "yrem") is an ordinary word too often to be recognised in text.
const MIN_NAME_CHARS: usize = 5;
const KEYWORD: &str = "keyword_match";
const GRAPH: &str = "graph-connected";

/// Gate settings.
#[derive(Debug, Clone, Default)]
pub struct Gate {
    pub min_similarity: f64,
    /// Floor for hits outside the project, when there is one.
    pub min_similarity_other: f64,
    pub include_captured: bool,
    pub excluded_namespaces: Vec<String>,
    pub project: Option<String>,
}

impl Gate {
    #[must_use]
    pub fn with_project(self, project: Option<String>) -> Self {
        Self { project, ..self }
    }

    /// Why a row must not be injected whatever its relevance, or `None` when it may be:
    /// excluded or sensitive, superseded, auto-captured.
    #[must_use]
    pub fn eligible(&self, row: &Value) -> Option<&'static str> {
        let namespace = row.get("namespace").and_then(Value::as_str).unwrap_or("");
        if self
            .excluded_namespaces
            .iter()
            .any(|p| namespace.starts_with(p.as_str()))
        {
            return Some("namespace");
        }
        let meta = metadata(row);
        if is_sensitive(&meta) {
            return Some("sensitive");
        }
        if is_superseded(row) {
            return Some("superseded");
        }
        if !self.include_captured {
            if row.get("source").and_then(Value::as_str) == Some(AUTO_CAPTURE)
                || meta.get("kind").and_then(Value::as_str) == Some(AUTO_CAPTURE)
            {
                return Some("auto-capture");
            }
            let text = row.get("text").and_then(Value::as_str).unwrap_or("");
            if is_prompt_merge(text, &meta) {
                return Some("prompt-merge");
            }
        }
        None
    }

    /// Why `hit` must not be injected, or `None` when it may be.
    #[must_use]
    pub fn reject(&self, hit: &Value) -> Option<&'static str> {
        if let Some(why) = self.eligible(hit) {
            return Some(why);
        }
        if hit.get("score").and_then(Value::as_f64).unwrap_or(0.0) < MIN_SCORE {
            return Some("score");
        }
        if self.min_similarity <= 0.0 {
            return None;
        }
        let cosine = similarity(hit);
        let at_least = |floor: f64| cosine.is_some_and(|s| s >= floor);
        match &self.project {
            None => (!at_least(self.min_similarity)).then_some("similarity"),
            Some(_) if self.is_project_hit(hit) => {
                (!(at_least(self.min_similarity) || keyword_agreement(hit))).then_some("similarity")
            }
            Some(_) => (!at_least(self.min_similarity.max(self.min_similarity_other)))
                .then_some("off-project"),
        }
    }

    /// Whether a row belongs to the project: its namespace, or its text, names it.
    #[must_use]
    pub fn is_project_hit(&self, row: &Value) -> bool {
        let Some(project) = self.project.as_deref() else {
            return false;
        };
        row.get("namespace").and_then(Value::as_str) == Some(project)
            || row
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|t| names(t, project))
    }

    /// Puts the project's hits first, keeping the server's order within each group.
    pub fn prioritize(&self, hits: &mut [&Value]) {
        hits.sort_by_key(|h| !self.is_project_hit(h));
    }
}

/// Case-insensitive whole-word match: `fluidd` is in "Fluidd task #3" but not in "fluiddb" or
/// "fluidd-bench" (another project's name often starts with this one's). Short names never
/// match in text.
fn names(text: &str, project: &str) -> bool {
    if project.chars().count() < MIN_NAME_CHARS {
        return false;
    }
    let (text, project) = (text.to_lowercase(), project.to_lowercase());
    text.match_indices(&project).any(|(at, found)| {
        let before = text[..at].chars().next_back();
        let after = text[at + found.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric)
            && !after.is_some_and(|c| c.is_alphanumeric() || c == '-')
    })
}

/// Cosine of the semantic lane, or `None` when only keyword or graph lanes matched.
#[must_use]
pub fn similarity(hit: &Value) -> Option<f64> {
    reasons(hit).find_map(|s| {
        s.strip_prefix("semantically similar (")?
            .split(')')
            .next()?
            .parse()
            .ok()
    })
}

fn reasons(hit: &Value) -> impl Iterator<Item = &str> {
    hit.get("why_retrieved")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// Keyword match backed by a second lane. The server's lane count includes the graph lane,
/// which generic entities (`API`, `POST`) make agree with almost anything, so it is left out.
fn keyword_agreement(hit: &Value) -> bool {
    reasons(hit).any(|r| r == KEYWORD)
        && lanes(hit) >= KEYWORD_MIN_LANES
        && hit.get("score").and_then(Value::as_f64).unwrap_or(0.0) >= KEYWORD_MIN_SCORE
}

/// Lanes behind a hit, not counting the graph: "multi-lane agreement (N lanes)", less one
/// when a "graph-connected via" reason is present.
fn lanes(hit: &Value) -> u32 {
    let reported = reasons(hit)
        .find_map(|r| {
            r.strip_prefix("multi-lane agreement (")?
                .split(' ')
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(1_u32);
    let graph = u32::from(reasons(hit).any(|r| r.starts_with(GRAPH)));
    reported.saturating_sub(graph)
}

fn is_superseded(row: &Value) -> bool {
    row.get("current_status").and_then(Value::as_str) == Some("superseded")
        || row
            .get("superseded_by")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
}

fn metadata(hit: &Value) -> Value {
    parse_metadata(hit.get("metadata").unwrap_or(&Value::Null))
}

/// Memory metadata as a value: the server sends an object or a JSON string.
#[must_use]
pub fn parse_metadata(v: &Value) -> Value {
    match v {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        other => other.clone(),
    }
}

/// Whether `meta` marks a memory as sensitive.
#[must_use]
pub fn is_sensitive(meta: &Value) -> bool {
    meta.get("sensitive") == Some(&Value::Bool(true))
}

/// The rids a server merge was made from, from the row's own metadata.
#[must_use]
pub fn merge_sources(row: &Value) -> Vec<String> {
    sources_in(&metadata(row))
}

/// `metadata.consolidated_from` as rids.
#[must_use]
pub fn sources_in(meta: &Value) -> Vec<String> {
    meta.get("consolidated_from")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
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
