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
    #[arg(required_unless_present = "stdin")]
    pub text: Option<String>,
    #[arg(long, value_enum)]
    pub memory_type: Option<MemoryType>,
    /// Higher decays slower.
    #[arg(long)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub importance: Option<f64>,
    /// Emotional tone.
    #[arg(long, allow_negative_numbers = true)]
    #[schemars(range(min = -1.0, max = 1.0))]
    pub valence: Option<f64>,
    #[arg(long)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub certainty: Option<f64>,
    /// e.g. work, people, architecture.
    #[arg(long)]
    pub domain: Option<String>,
    /// e.g. user, document, inference.
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(skip)]
    pub metadata: Option<Map<String, Value>>,
    /// When the fact happened, if not now.
    #[arg(long)]
    pub created_at: Option<String>,
    /// Retries with the same key store once.
    #[arg(long)]
    pub idempotency_key: Option<String>,
    /// Batch, instead of text.
    #[arg(skip)]
    pub memories: Option<Vec<MemoryInput>>,
    /// Read the text from stdin.
    #[arg(long, conflicts_with = "text")]
    #[serde(skip)]
    #[schemars(skip)]
    pub stdin: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecallArgs {
    #[arg(default_value = "", hide_default_value = true)]
    pub query: String,
    #[arg(long)]
    #[schemars(range(min = 1, max = 100))]
    pub top_k: Option<u32>,
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long)]
    pub domain: Option<String>,
    #[arg(long, value_enum)]
    pub memory_type: Option<MemoryType>,
    /// Time or span back from now (7d).
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long)]
    pub until: Option<String>,
    /// Drop hits below this fraction of the top score.
    #[arg(long)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub min_score_ratio: Option<f64>,
    #[arg(long, value_enum)]
    pub order: Option<Order>,
    /// Rids to drop from results.
    #[arg(long, num_args = 1..)]
    pub refine_exclude: Option<Vec<String>>,
    /// Read the query from stdin.
    #[arg(long)]
    #[serde(skip)]
    #[schemars(skip)]
    pub stdin: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForgetArgs {
    pub rid: String,
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CorrectArgs {
    pub rid: String,
    /// Why the fact changed.
    #[arg(long)]
    pub reason: String,
    #[arg(long)]
    pub new_text: Option<String>,
    #[arg(long)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub new_importance: Option<f64>,
    #[arg(long, allow_negative_numbers = true)]
    #[schemars(range(min = -1.0, max = 1.0))]
    pub new_valence: Option<f64>,
    #[arg(skip)]
    pub metadata_merge: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThinkArgs {}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphRelateArgs {
    #[arg(long)]
    pub src: String,
    #[arg(long)]
    pub dst: String,
    /// e.g. uses, owns.
    #[arg(long)]
    pub rel_type: String,
    #[arg(long)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub weight: Option<f64>,
    #[arg(long)]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphEdgesArgs {
    #[arg(long)]
    pub entity: String,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConflictListArgs {
    /// open, resolved or dismissed.
    #[arg(long)]
    pub status: Option<String>,
    #[arg(long)]
    pub entity: Option<String>,
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConflictResolveArgs {
    pub id: String,
    #[arg(long, value_enum)]
    pub strategy: Strategy,
    /// Merged text, for merge.
    #[arg(long)]
    pub new_text: Option<String>,
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConflictDismissArgs {
    pub id: String,
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionDigestArgs {
    #[arg(long)]
    pub namespace: Option<String>,
    /// Add unanswered recall topics.
    #[arg(long)]
    #[serde(default)]
    pub include_gaps: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCaptureArgs {
    #[arg(default_value = "", hide_default_value = true)]
    pub summary: String,
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long)]
    pub domain: Option<String>,
    /// Read the summary from stdin.
    #[arg(long)]
    #[serde(skip)]
    #[schemars(skip)]
    pub stdin: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionStartArgs {
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long)]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionEndArgs {
    pub session_id: String,
    #[arg(long)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryGetArgs {
    pub rid: String,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryCurrentArgs {
    #[arg(long)]
    pub namespace: String,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryListArgs {
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long)]
    pub domain: Option<String>,
    #[arg(long, value_enum)]
    pub memory_type: Option<MemoryType>,
    #[arg(long)]
    pub limit: Option<u32>,
    #[arg(long)]
    pub offset: Option<u32>,
    /// Newest first.
    #[arg(long)]
    #[serde(default)]
    pub desc: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryGapsArgs {
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryStatsArgs {}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemporalListArgs {
    /// Window in days.
    #[arg(long)]
    pub days: Option<u32>,
    #[arg(long)]
    pub limit: Option<u32>,
    #[arg(long)]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemporalAsOfArgs {
    #[arg(default_value = "", hide_default_value = true)]
    pub query: String,
    /// Time or span back from now (7d).
    #[arg(long)]
    pub at: String,
    #[arg(long)]
    #[schemars(range(min = 1, max = 1000))]
    pub top_k: Option<u32>,
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long, value_enum)]
    pub memory_type: Option<MemoryType>,
    /// Read the query from stdin.
    #[arg(long)]
    #[serde(skip)]
    #[schemars(skip)]
    pub stdin: bool,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerListArgs {
    #[arg(long)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerHistoryArgs {
    #[arg(long)]
    pub limit: Option<u32>,
    /// e.g. `decay_review`.
    #[arg(long)]
    pub trigger_type: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriggerActionArgs {
    pub id: String,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimAddArgs {
    pub src: String,
    /// e.g. uses, owns.
    pub rel_type: String,
    pub dst: String,
    #[arg(long)]
    pub namespace: Option<String>,
    /// 1 asserts, -1 negates.
    #[arg(long, allow_negative_numbers = true)]
    pub polarity: Option<i32>,
    /// e.g. asserted.
    #[arg(long)]
    pub modality: Option<String>,
    /// Time or span back from now (7d).
    #[arg(long)]
    pub valid_from: Option<String>,
    #[arg(long)]
    pub valid_to: Option<String>,
    /// e.g. low, medium, high.
    #[arg(long)]
    pub confidence_band: Option<String>,
    #[arg(long)]
    pub source_memory_rid: Option<String>,
    #[arg(long)]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimListArgs {
    pub entity: String,
    #[arg(long)]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcedureSurfaceArgs {
    pub query: String,
    #[arg(long)]
    #[schemars(range(min = 1, max = 1000))]
    pub top_k: Option<u32>,
    #[arg(long)]
    pub domain: Option<String>,
    #[arg(long)]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskListArgs {
    #[arg(long)]
    pub namespace: Option<String>,
    #[arg(long)]
    pub status: Option<String>,
}
