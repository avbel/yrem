//! One struct per operation: clap arguments, serde input and MCP JSON Schema at once.
//!
//! Doc comments are agent-facing text (schema `description` and `--help`): keep them terse.

use clap::{Args, ValueEnum};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Semantic,
    Episodic,
    Procedural,
}

impl MemoryType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Episodic => "episodic",
            Self::Procedural => "procedural",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum Order {
    Relevance,
    Recency,
    Certainty,
    FirstMention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum Strategy {
    KeepA,
    KeepB,
    KeepBoth,
    Merge,
    Dismiss,
}

impl Strategy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KeepA => "keep_a",
            Self::KeepB => "keep_b",
            Self::KeepBoth => "keep_both",
            Self::Merge => "merge",
            Self::Dismiss => "dismiss",
        }
    }
}

/// One memory of a batch `remember`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryInput {
    pub text: String,
    pub memory_type: Option<MemoryType>,
    pub importance: Option<f64>,
    pub valence: Option<f64>,
    pub certainty: Option<f64>,
    pub domain: Option<String>,
    pub source: Option<String>,
    pub namespace: Option<String>,
    pub metadata: Option<Map<String, Value>>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RememberArgs {
    #[arg(
        long_help = "Fact to store: one specific, self-contained statement. Omit when using --stdin.",
        required_unless_present = "stdin"
    )]
    pub text: Option<String>,
    #[arg(
        long_help = "Kind of memory; the server defaults to semantic when omitted.",
        long,
        value_enum
    )]
    pub memory_type: Option<MemoryType>,
    /// Higher decays slower.
    #[arg(
        long_help = "0.0 to 1.0; higher decays slower (server default 0.5).",
        long
    )]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub importance: Option<f64>,
    /// Emotional tone.
    #[arg(
        long_help = "-1.0 (negative) to 1.0 (positive) emotional tone (server default 0).",
        long,
        allow_negative_numbers = true
    )]
    #[schemars(range(min = -1.0, max = 1.0))]
    pub valence: Option<f64>,
    #[arg(
        long_help = "0.0 to 1.0 confidence in the fact (server default 1.0).",
        long
    )]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub certainty: Option<f64>,
    /// e.g. work, people, architecture.
    #[arg(
        long_help = "Free-form topic label such as work or people; filters on exact match.",
        long
    )]
    pub domain: Option<String>,
    /// e.g. user, document, inference.
    #[arg(
        long_help = "Where the fact came from, e.g. user, document, inference (server default user).",
        long
    )]
    pub source: Option<String>,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    #[arg(skip)]
    pub metadata: Option<Map<String, Value>>,
    /// When the fact happened, if not now.
    #[arg(
        long_help = "When the fact happened, if not now. RFC 3339 with offset, YYYY-MM-DD (UTC), unix seconds, or a span back from now (7d, 12h, 30m, 2w).",
        long
    )]
    pub created_at: Option<String>,
    /// Retries with the same key store once.
    #[arg(
        long_help = "Retrying with the same key stores once and returns the original rid. Single memories only.",
        long
    )]
    pub idempotency_key: Option<String>,
    /// Batch, instead of text.
    #[arg(skip)]
    pub memories: Option<Vec<MemoryInput>>,
    /// Read the text from stdin.
    #[arg(
        long_help = "Read the text from stdin instead of the TEXT argument.",
        long,
        conflicts_with = "text"
    )]
    #[serde(skip)]
    #[schemars(skip)]
    pub stdin: bool,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecallArgs {
    #[arg(
        long_help = "Natural-language search text. Omit when using --stdin.",
        default_value = "",
        hide_default_value = true
    )]
    pub query: String,
    #[arg(long_help = "Maximum results, 1 to 100 (server default 10).", long)]
    #[schemars(range(min = 1, max = 100))]
    pub top_k: Option<u32>,
    #[arg(
        long_help = "Only memories in this namespace (a row-level tag within the token's database).",
        long
    )]
    pub namespace: Option<String>,
    #[arg(long_help = "Only memories with this domain label.", long)]
    pub domain: Option<String>,
    #[arg(long_help = "Only memories of this type.", long, value_enum)]
    pub memory_type: Option<MemoryType>,
    /// Time or span back from now (7d).
    #[arg(
        long_help = "Only memories created at or after this time, filtered client-side. RFC 3339 with offset, YYYY-MM-DD (UTC), unix seconds, or a span back from now (7d, 12h, 30m, 2w).",
        long
    )]
    pub since: Option<String>,
    #[arg(
        long_help = "Only memories created at or before this time, filtered client-side. Same formats as --since.",
        long
    )]
    pub until: Option<String>,
    /// Drop hits below this fraction of the top score.
    #[arg(
        long_help = "Above 0 up to 1.0; drop hits scoring below this fraction of the top hit.",
        long
    )]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub min_score_ratio: Option<f64>,
    #[arg(
        long_help = "Client-side sort; relevance keeps the server order.",
        long,
        value_enum
    )]
    pub order: Option<Order>,
    /// Rids to drop from results.
    #[arg(long_help = "Rids to leave out, e.g. hits you already saw in a previous recall.", long, num_args = 1..)]
    pub refine_exclude: Option<Vec<String>>,
    /// Read the query from stdin.
    #[arg(
        long_help = "Read the query from stdin instead of the QUERY argument.",
        long
    )]
    #[serde(skip)]
    #[schemars(skip)]
    pub stdin: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForgetArgs {
    #[arg(
        long_help = "Id of the memory to delete (tombstone), as returned by remember or recall."
    )]
    pub rid: String,
    #[arg(long_help = "Why it is forgotten; stored on the tombstone.", long)]
    pub reason: Option<String>,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CorrectArgs {
    #[arg(long_help = "Id of the memory to replace.")]
    pub rid: String,
    /// Why the fact changed.
    #[arg(long)]
    pub reason: String,
    #[arg(
        long_help = "Replacement text; omit to keep the old text and change only the other fields.",
        long
    )]
    pub new_text: Option<String>,
    #[arg(
        long_help = "New importance, 0.0 to 1.0; omit to keep the old value.",
        long
    )]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub new_importance: Option<f64>,
    #[arg(
        long_help = "New valence, -1.0 to 1.0; omit to keep the old value.",
        long,
        allow_negative_numbers = true
    )]
    #[schemars(range(min = -1.0, max = 1.0))]
    pub new_valence: Option<f64>,
    #[arg(skip)]
    pub metadata_merge: Option<Map<String, Value>>,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThinkArgs {}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphRelateArgs {
    #[arg(long_help = "Source entity name.", long)]
    pub src: String,
    #[arg(long_help = "Target entity name.", long)]
    pub dst: String,
    /// e.g. uses, owns.
    #[arg(long)]
    pub rel_type: String,
    #[arg(long_help = "Edge strength, 0.0 to 1.0 (server default 1.0).", long)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub weight: Option<f64>,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphEdgesArgs {
    #[arg(
        long_help = "Entity whose relations to list. The 0.20 server has no edge query, so this exits 4 there.",
        long
    )]
    pub entity: String,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConflictListArgs {
    /// open, resolved or dismissed.
    #[arg(long_help = "Filter by status: open, resolved or dismissed.", long)]
    pub status: Option<String>,
    #[arg(long_help = "Only conflicts involving this entity.", long)]
    pub entity: Option<String>,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    #[arg(long_help = "Maximum conflicts to return (server default 50).", long)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConflictResolveArgs {
    #[arg(long_help = "Conflict id from conflict list.")]
    pub id: String,
    #[arg(
        long_help = "How to resolve; keep_a/keep_b tombstone the loser, merge needs --new-text, dismiss closes it unchanged.",
        long,
        value_enum
    )]
    pub strategy: Strategy,
    /// Merged text, for merge.
    #[arg(long_help = "Merged text; required for the merge strategy.", long)]
    pub new_text: Option<String>,
    #[arg(long_help = "Free-text note stored with the resolution.", long)]
    pub note: Option<String>,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConflictDismissArgs {
    #[arg(long_help = "Conflict id from conflict list.")]
    pub id: String,
    #[arg(long_help = "Free-text note stored with the resolution.", long)]
    pub note: Option<String>,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionDigestArgs {
    #[arg(
        long_help = "Scope the briefing to this namespace (a row-level tag within the token's database).",
        long
    )]
    pub namespace: Option<String>,
    /// Add unanswered recall topics.
    #[arg(
        long_help = "Also list recall topics that returned weak results.",
        long
    )]
    #[serde(default)]
    pub include_gaps: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCaptureArgs {
    #[arg(
        long_help = "Session summary; the server splits it into separate provisional facts. Omit when using --stdin.",
        default_value = "",
        hide_default_value = true
    )]
    pub summary: String,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    #[arg(long_help = "Domain label given to every captured fact.", long)]
    pub domain: Option<String>,
    /// Read the summary from stdin.
    #[arg(
        long_help = "Read the summary from stdin instead of the SUMMARY argument.",
        long
    )]
    #[serde(skip)]
    #[schemars(skip)]
    pub stdin: bool,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionStartArgs {
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    #[arg(
        long_help = "Name of the calling client, stored with the session.",
        long
    )]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionEndArgs {
    #[arg(long_help = "Id printed by session start.")]
    pub session_id: String,
    #[arg(long_help = "Optional closing summary stored with the session.", long)]
    pub summary: Option<String>,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryGetArgs {
    #[arg(long_help = "Memory id, as returned by remember or recall.")]
    pub rid: String,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryCurrentArgs {
    #[arg(
        long_help = "Namespace to read (required); the server returns the latest value per fact in it.",
        long
    )]
    pub namespace: String,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryListArgs {
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    #[arg(
        long_help = "Free-form topic label such as work or people; filters on exact match.",
        long
    )]
    pub domain: Option<String>,
    #[arg(
        long_help = "Kind of memory; the server defaults to semantic when omitted.",
        long,
        value_enum
    )]
    pub memory_type: Option<MemoryType>,
    #[arg(long_help = "Page size.", long)]
    pub limit: Option<u32>,
    #[arg(long_help = "Rows to skip, for paging.", long)]
    pub offset: Option<u32>,
    /// Newest first.
    #[arg(long_help = "Sort newest first instead of oldest first.", long)]
    #[serde(default)]
    pub desc: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryGapsArgs {
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    #[arg(long_help = "Maximum topics to return.", long)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryStatsArgs {}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemporalListArgs {
    /// Window in days.
    #[arg(
        long_help = "Window length in days, counted back (stale) or forward (upcoming) from now.",
        long
    )]
    pub days: Option<u32>,
    #[arg(long_help = "Maximum memories to return.", long)]
    pub limit: Option<u32>,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemporalAsOfArgs {
    #[arg(
        long_help = "Natural-language search text. Omit when using --stdin.",
        default_value = "",
        hide_default_value = true
    )]
    pub query: String,
    /// Time or span back from now (7d).
    #[arg(
        long_help = "Point in time to recall as of. RFC 3339 with offset, YYYY-MM-DD (UTC), unix seconds, or a span back from now (7d, 12h, 30m, 2w).",
        long
    )]
    pub at: String,
    #[arg(long_help = "Maximum results, 1 to 1000.", long)]
    #[schemars(range(min = 1, max = 1000))]
    pub top_k: Option<u32>,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    #[arg(
        long_help = "Kind of memory; the server defaults to semantic when omitted.",
        long,
        value_enum
    )]
    pub memory_type: Option<MemoryType>,
    /// Read the query from stdin.
    #[arg(
        long_help = "Read the query from stdin instead of the QUERY argument.",
        long
    )]
    #[serde(skip)]
    #[schemars(skip)]
    pub stdin: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerListArgs {
    #[arg(long_help = "Maximum triggers to return (server default 20).", long)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerHistoryArgs {
    #[arg(long_help = "Maximum triggers to return (server default 50).", long)]
    pub limit: Option<u32>,
    /// e.g. `decay_review`.
    #[arg(
        long_help = "Only triggers of this type, e.g. decay_review or temporal_drift.",
        long
    )]
    pub trigger_type: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerActionArgs {
    #[arg(
        long_help = "Trigger id from trigger list or trigger history. Unknown ids and transitions the current state forbids exit 1 (not found)."
    )]
    pub id: String,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimAddArgs {
    #[arg(long_help = "Subject entity.")]
    pub src: String,
    /// e.g. uses, owns.
    #[arg(long_help = "Relation, e.g. uses, owns, reviews.")]
    pub rel_type: String,
    #[arg(long_help = "Object entity.")]
    pub dst: String,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    /// 1 asserts, -1 negates.
    #[arg(
        long_help = "1 asserts the claim (default), -1 negates it.",
        long,
        allow_negative_numbers = true
    )]
    pub polarity: Option<i32>,
    /// e.g. asserted.
    #[arg(long_help = "How the claim is held, e.g. asserted.", long)]
    pub modality: Option<String>,
    /// Time or span back from now (7d).
    #[arg(
        long_help = "Start of validity. RFC 3339 with offset, YYYY-MM-DD (UTC), unix seconds, or a span back from now (7d, 12h, 30m, 2w).",
        long
    )]
    pub valid_from: Option<String>,
    #[arg(
        long_help = "End of validity; same formats as --valid-from. Must not precede it.",
        long
    )]
    pub valid_to: Option<String>,
    /// e.g. low, medium, high.
    #[arg(long_help = "Confidence label: low, medium or high.", long)]
    pub confidence_band: Option<String>,
    #[arg(long_help = "Rid of the memory that supports this claim.", long)]
    pub source_memory_rid: Option<String>,
    #[arg(long_help = "Claim strength, 0.0 to 1.0.", long)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub weight: Option<f64>,
    /// Store despite a secret-like match.
    #[arg(
        long_help = "Waive the secret-like data check (exit 7) and send the data as given. Use only when it is not a secret.",
        long
    )]
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimListArgs {
    #[arg(long_help = "Entity whose claims to list (as subject or object).")]
    pub entity: String,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcedureSurfaceArgs {
    #[arg(long_help = "Task description; returns stored procedural memories that match it.")]
    pub query: String,
    #[arg(long_help = "Maximum procedures, 1 to 1000.", long)]
    #[schemars(range(min = 1, max = 1000))]
    pub top_k: Option<u32>,
    #[arg(long_help = "Only procedures with this domain label.", long)]
    pub domain: Option<String>,
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskListArgs {
    #[arg(
        long_help = "Row-level tag within the token's database, not a separate database. Omit for the default namespace.",
        long
    )]
    pub namespace: Option<String>,
    #[arg(long_help = "Filter by task status, e.g. pending.", long)]
    pub status: Option<String>,
}
