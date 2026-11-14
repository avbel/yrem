//! Wire types. Requests are typed; responses are lenient (`#[serde(default)]` plus a
//! flattened `extra` map) so fields the server adds never break decoding.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, Serialize)]
pub struct RememberRequest {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certainty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emotional_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    /// Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BatchRequest<'a> {
    pub memories: &'a [RememberRequest],
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RecallRequest {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_superseded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reinforce: Option<bool>,
    /// `[since, until]` in unix seconds. Servers may ignore it; yrem also filters locally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_window: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecallResponse {
    pub results: Vec<RecallHit>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecallHit {
    pub rid: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certainty: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ForgetRequest {
    pub rid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CorrectRequest {
    pub rid: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_importance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_valence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_merge: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ThinkRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_consolidation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_conflict_scan: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_pattern_mining: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consolidation_limit: Option<u32>,
}

/// `POST /v1/relate`. The server's field names differ from the CLI's `src` / `dst` / `rel_type`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RelateRequest {
    pub entity: String,
    pub target: String,
    pub relationship: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GraphQuery<'a> {
    pub action: &'a str,
    pub entity: &'a str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ConflictsQuery {
    pub status: Option<String>,
    pub conflict_type: Option<String>,
    pub entity: Option<String>,
    pub namespace: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ResolveRequest {
    pub strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner_rid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DigestQuery {
    pub namespace: Option<String>,
    pub scope: Option<String>,
    pub include_gaps: Option<bool>,
    pub max_decisions: Option<u32>,
    pub max_conflicts: Option<u32>,
    pub snippet_chars: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CaptureRequest {
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionStartRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionEndRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MemoriesQuery {
    pub namespace: Option<String>,
    pub domain: Option<String>,
    pub memory_type: Option<String>,
    pub source: Option<String>,
    pub q: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub order: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GapsQuery {
    pub namespace: Option<String>,
    pub min_count: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TemporalQuery {
    pub days: Option<u32>,
    pub limit: Option<u32>,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AsOfRequest {
    pub query: String,
    /// Unix seconds.
    pub as_of: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TriggersQuery {
    pub limit: Option<u32>,
    pub trigger_type: Option<String>,
}

/// `POST /v1/claim`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ClaimRequest {
    pub src: String,
    pub rel_type: String,
    pub dst: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modality: Option<String>,
    /// Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<f64>,
    /// Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_band: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_memory_rid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ClaimsQuery {
    pub entity: String,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SurfaceRequest {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TasksQuery {
    pub namespace: Option<String>,
    pub status: Option<String>,
}

/// One stored memory as `GET /v1/memory/{rid}` returns it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    pub rid: String,
    pub text: String,
    #[serde(alias = "type")]
    pub memory_type: Option<String>,
    pub importance: Option<f64>,
    pub valence: Option<f64>,
    pub certainty: Option<f64>,
    pub domain: Option<String>,
    pub source: Option<String>,
    pub namespace: Option<String>,
    pub emotional_state: Option<String>,
    #[serde(alias = "metadata_json")]
    pub metadata: Value,
    pub created_at: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
