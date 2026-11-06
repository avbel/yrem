//! Operations. MCP and CLI both call these; neither contains logic of its own.

mod args;

use std::collections::HashSet;
use std::io::Read;

use chrono::Utc;
use serde_json::{Map, Value, json};

pub use args::*;

use crate::client::{
    AsOfRequest, CaptureRequest, ClaimRequest, ClaimsQuery, Client, ConflictsQuery, CorrectRequest,
    DigestQuery, ForgetRequest, GapsQuery, MemoriesQuery, RecallRequest, RelateRequest,
    RememberRequest, ResolveRequest, SessionEndRequest, SessionStartRequest, SurfaceRequest,
    TasksQuery, TemporalQuery, ThinkRequest, TriggersQuery,
};
use crate::error::{Error, Result};
use crate::output::shape;
use crate::secrets::guard;
use crate::time::{self, TimeWindow};

/// MCP tool names, in `tools/list` order. Each has a CLI counterpart.
///
/// `graph_edges` is CLI-only: `yantrikdb-server` 0.20 has no HTTP edge query, and an MCP
/// tool that always fails costs tokens and misleads agents.
pub const TOOLS: [&str; 12] = [
    "remember",
    "recall",
    "forget",
    "correct",
    "think",
    "graph_relate",
    "conflict_list",
    "conflict_resolve",
    "session_digest",
    "session_capture",
    "memory_get",
    "memory_current",
];

/// Drops the offending value from a serde error, which may be a secret the caller pasted.
///
/// Quoted strings are elided, and so is the name in `unknown field` / `unknown variant`; the
/// expected types and the list of valid names stay.
fn without_values(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut chars = msg.chars();
    let hide_first_tick =
        msg.starts_with("unknown field `") || msg.starts_with("unknown variant `");
    let mut ticks = 0;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push_str("\"…\"");
                while let Some(q) = chars.next() {
                    match q {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '`' if hide_first_tick && ticks == 0 => {
                ticks += 1;
                out.push_str("`…`");
                for q in chars.by_ref() {
                    if q == '`' {
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Shared application state: one HTTP client, cloned cheaply per call.
#[derive(Debug, Clone)]
pub struct App {
    pub client: Client,
}

impl App {
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Runs an MCP tool by name with raw JSON arguments (`yrem call`).
    ///
    /// # Errors
    /// Unknown tool, arguments that don't match the tool's schema, or the tool's own error.
    pub async fn call(&self, tool: &str, args: Value) -> Result<Value> {
        fn parse<T: serde::de::DeserializeOwned>(tool: &str, v: Value) -> Result<T> {
            serde_json::from_value(v)
                .map_err(|e| Error::Invalid(format!("{tool}: {}", without_values(&e.to_string()))))
        }
        let args = if args.is_null() { json!({}) } else { args };
        match tool {
            "remember" => self.remember(parse(tool, args)?).await,
            "recall" => self.recall(parse(tool, args)?).await,
            "forget" => self.forget(parse(tool, args)?).await,
            "correct" => self.correct(parse(tool, args)?).await,
            "think" => self.think(parse(tool, args)?).await,
            "graph_relate" => self.graph_relate(parse(tool, args)?).await,
            "graph_edges" => self.graph_edges(parse(tool, args)?).await,
            "conflict_list" => self.conflict_list(parse(tool, args)?).await,
            "conflict_resolve" => self.conflict_resolve(parse(tool, args)?).await,
            "session_digest" => self.session_digest(parse(tool, args)?).await,
            "session_capture" => self.session_capture(parse(tool, args)?).await,
            "memory_get" => self.memory_get(parse(tool, args)?).await,
            "memory_current" => self.memory_current(parse(tool, args)?).await,
            other => Err(Error::Invalid(format!(
                "unknown tool `{other}`; one of: {}",
                TOOLS.join(", ")
            ))),
        }
    }

    pub async fn remember(&self, mut args: RememberArgs) -> Result<Value> {
        if args.stdin {
            args.text = Some(read_stdin()?);
        }
        let now = Utc::now();
        let defaults = MemoryInput {
            text: String::new(),
            memory_type: args.memory_type,
            importance: args.importance,
            valence: args.valence,
            certainty: args.certainty,
            domain: args.domain,
            source: args.source,
            namespace: args.namespace,
            metadata: args.metadata,
            created_at: args.created_at,
        };
        match (args.text.take(), args.memories.take()) {
            (Some(_), Some(_)) => Err(Error::Invalid("give `text` or `memories`, not both".into())),
            (None, None) => Err(Error::Invalid("`text` is required".into())),
            (Some(text), None) => {
                let input = MemoryInput { text, ..defaults };
                let mut req = remember_request(input, now)?;
                req.idempotency_key = args.idempotency_key;
                guard("remember", args.force, &req)?;
                Ok(shape(self.client.remember(&req).await?))
            }
            (None, Some(memories)) => {
                if args.idempotency_key.is_some() {
                    return Err(Error::Invalid(
                        "`idempotency_key` applies to a single memory, not a batch".into(),
                    ));
                }
                if memories.is_empty() {
                    return Err(Error::Invalid("`memories` is empty".into()));
                }
                let reqs = memories
                    .into_iter()
                    .map(|m| remember_request(m.or(&defaults), now))
                    .collect::<Result<Vec<_>>>()?;
                guard("remember", args.force, &json!({ "memories": &reqs }))?;
                Ok(shape(self.client.remember_batch(&reqs).await?))
            }
        }
    }

    pub async fn recall(&self, mut args: RecallArgs) -> Result<Value> {
        if args.stdin {
            args.query = read_stdin()?;
        }
        let query = non_empty("query", args.query)?;
        if let Some(r) = args.min_score_ratio
            && !(r > 0.0 && r <= 1.0)
        {
            return Err(Error::Invalid("`min_score_ratio` must be in (0, 1]".into()));
        }
        if args.top_k == Some(0) {
            return Err(Error::Invalid("`top_k` must be at least 1".into()));
        }
        let window =
            TimeWindow::from_args(args.since.as_deref(), args.until.as_deref(), Utc::now())?;
        let req = RecallRequest {
            query,
            top_k: args.top_k,
            memory_type: args.memory_type.map(|t| t.as_str().to_owned()),
            namespace: args.namespace,
            domain: args.domain,
            source: None,
            include_superseded: None,
            time_window: window.map(|w| w.as_unix()),
        };
        let mut resp = self.client.recall(&req).await?;
        let hits = &mut resp.results;
        let mut counts = Map::new();
        let mut count = |key: &str, n: usize| {
            if n > 0 {
                counts.insert(key.into(), n.into());
            }
        };
        if let Some(w) = window {
            count(
                "filtered_by_time_window",
                hits.extract_if(.., |h| !h.created_at.is_some_and(|t| w.contains(t)))
                    .count(),
            );
        }
        if let Some(exclude) = &args.refine_exclude {
            let exclude: HashSet<&str> = exclude.iter().map(String::as_str).collect();
            count(
                "filtered_by_refine_exclude",
                hits.extract_if(.., |h| exclude.contains(h.rid.as_str()))
                    .count(),
            );
        }
        if let Some(ratio) = args.min_score_ratio
            && let Some(top) = hits.iter().filter_map(|h| h.score).reduce(f64::max)
        {
            count(
                "filtered_by_min_score_ratio",
                hits.extract_if(.., |h| h.score.unwrap_or(0.0) < ratio * top)
                    .count(),
            );
        }
        let key = |v: Option<f64>| v.unwrap_or(f64::NEG_INFINITY);
        match args.order {
            None | Some(Order::Relevance) => {}
            Some(Order::Recency) => {
                hits.sort_by(|a, b| key(b.created_at).total_cmp(&key(a.created_at)));
            }
            Some(Order::FirstMention) => {
                hits.sort_by(|a, b| key(a.created_at).total_cmp(&key(b.created_at)));
            }
            Some(Order::Certainty) => {
                hits.sort_by(|a, b| key(b.certainty).total_cmp(&key(a.certainty)));
            }
        }
        let mut out = serde_json::to_value(&resp).map_err(|e| Error::Decode(e.to_string()))?;
        if let Value::Object(o) = &mut out {
            o.extend(counts);
        }
        Ok(shape(out))
    }

    pub async fn forget(&self, args: ForgetArgs) -> Result<Value> {
        let req = ForgetRequest {
            rid: non_empty("rid", args.rid)?,
            namespace: None,
            reason: args.reason,
        };
        guard("forget", args.force, &req)?;
        Ok(shape(self.client.forget(&req).await?))
    }

    /// Native `POST /v1/correct`; on 501, emulated as get → remember → forget.
    pub async fn correct(&self, args: CorrectArgs) -> Result<Value> {
        let rid = non_empty("rid", args.rid)?;
        let reason = non_empty("reason", args.reason)?;
        if let Some(t) = &args.new_text
            && t.trim().is_empty()
        {
            return Err(Error::Invalid("`new_text` must not be empty".into()));
        }
        let req = CorrectRequest {
            rid: rid.clone(),
            reason: reason.clone(),
            new_text: args.new_text,
            new_importance: args
                .new_importance
                .map(|v| clamp("new_importance", v, 0.0, 1.0))
                .transpose()?,
            new_valence: args
                .new_valence
                .map(|v| clamp("new_valence", v, -1.0, 1.0))
                .transpose()?,
            metadata_merge: args.metadata_merge,
        };
        guard("correct", args.force, &req)?;
        match self.client.correct(&req).await {
            Ok(v) => {
                let mut v = shape(v);
                if let Value::Object(o) = &mut v {
                    o.insert("emulated".into(), false.into());
                }
                Ok(v)
            }
            Err(Error::Unsupported { .. }) => self.correct_emulated(req).await,
            Err(e) => Err(e),
        }
    }

    async fn correct_emulated(&self, req: CorrectRequest) -> Result<Value> {
        let old = self.client.memory_get(&req.rid).await?;
        let mut metadata = match old.metadata {
            Value::Object(m) => m,
            Value::String(s) => serde_json::from_str(&s).unwrap_or_default(),
            _ => Map::new(),
        };
        metadata.extend(req.metadata_merge.unwrap_or_default());
        metadata.insert("corrects".into(), req.rid.clone().into());
        metadata.insert("correct_reason".into(), req.reason.into());
        metadata.insert(
            "corrected_at".into(),
            Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                .into(),
        );
        let new = RememberRequest {
            text: req.new_text.unwrap_or(old.text),
            memory_type: old.memory_type,
            importance: req.new_importance.or(old.importance),
            valence: req.new_valence.or(old.valence),
            certainty: old.certainty,
            domain: old.domain.filter(|s| !s.is_empty()),
            source: old.source.filter(|s| !s.is_empty()),
            namespace: old.namespace.filter(|s| !s.is_empty()),
            emotional_state: old.emotional_state,
            metadata: Some(metadata),
            created_at: None,
            idempotency_key: None,
        };
        let written = self.client.remember(&new).await?;
        let new_rid = written["rid"]
            .as_str()
            .ok_or_else(|| Error::Decode("remember: no `rid` in response".into()))?
            .to_owned();
        let forget = ForgetRequest {
            rid: req.rid.clone(),
            namespace: None,
            reason: Some(format!("corrected by {new_rid}")),
        };
        if let Err(e) = self.client.forget(&forget).await {
            return Err(Error::PartialCorrect {
                old_rid: req.rid,
                new_rid,
                source: Box::new(e),
            });
        }
        Ok(json!({ "old_rid": req.rid, "new_rid": new_rid, "emulated": true }))
    }

    pub async fn think(&self, _args: ThinkArgs) -> Result<Value> {
        Ok(shape(self.client.think(&ThinkRequest::default()).await?))
    }

    pub async fn graph_relate(&self, args: GraphRelateArgs) -> Result<Value> {
        let req = RelateRequest {
            entity: non_empty("src", args.src)?,
            target: non_empty("dst", args.dst)?,
            relationship: non_empty("rel_type", args.rel_type)?,
            weight: args
                .weight
                .map(|w| clamp("weight", w, 0.0, 1.0))
                .transpose()?,
            namespace: args.namespace,
        };
        guard("graph_relate", args.force, &req)?;
        Ok(shape(self.client.relate(&req).await?))
    }

    pub async fn graph_edges(&self, args: GraphEdgesArgs) -> Result<Value> {
        let entity = non_empty("entity", args.entity)?;
        Ok(shape(self.client.graph_edges(&entity).await?))
    }

    pub async fn conflict_list(&self, args: ConflictListArgs) -> Result<Value> {
        let q = ConflictsQuery {
            status: args.status,
            conflict_type: None,
            entity: args.entity,
            namespace: args.namespace,
            limit: args.limit,
        };
        Ok(shape(self.client.conflicts(&q).await?))
    }

    pub async fn conflict_resolve(&self, args: ConflictResolveArgs) -> Result<Value> {
        let id = non_empty("id", args.id)?;
        if args.strategy == Strategy::Merge
            && args.new_text.as_deref().is_none_or(|t| t.trim().is_empty())
        {
            return Err(Error::Invalid("strategy `merge` needs `new_text`".into()));
        }
        let req = ResolveRequest {
            strategy: args.strategy.as_str().to_owned(),
            winner_rid: None,
            new_text: args.new_text,
            resolution_note: args.note,
        };
        guard("conflict_resolve", args.force, &req)?;
        Ok(shape(self.client.resolve_conflict(&id, &req).await?))
    }

    pub async fn conflict_dismiss(&self, args: ConflictDismissArgs) -> Result<Value> {
        self.conflict_resolve(ConflictResolveArgs {
            id: args.id,
            strategy: Strategy::Dismiss,
            new_text: None,
            note: args.note,
            force: args.force,
        })
        .await
    }

    pub async fn session_digest(&self, args: SessionDigestArgs) -> Result<Value> {
        let q = DigestQuery {
            namespace: args.namespace,
            include_gaps: args.include_gaps.then_some(true),
            ..DigestQuery::default()
        };
        Ok(shape(decode_maintenance(
            self.client.session_digest(&q).await?,
        )))
    }

    pub async fn session_capture(&self, mut args: SessionCaptureArgs) -> Result<Value> {
        if args.stdin {
            args.summary = read_stdin()?;
        }
        let req = CaptureRequest {
            summary: non_empty("summary", args.summary)?,
            namespace: args.namespace,
            domain: args.domain,
        };
        guard("session_capture", args.force, &req)?;
        Ok(shape(self.client.session_capture(&req).await?))
    }

    pub async fn session_start(&self, args: SessionStartArgs) -> Result<Value> {
        let req = SessionStartRequest {
            namespace: args.namespace,
            client_id: args.client_id,
        };
        guard("session_start", args.force, &req)?;
        Ok(shape(self.client.session_start(&req).await?))
    }

    pub async fn session_end(&self, args: SessionEndArgs) -> Result<Value> {
        let id = non_empty("session_id", args.session_id)?;
        let req = SessionEndRequest {
            summary: args.summary,
        };
        guard("session_end", args.force, &req)?;
        Ok(shape(self.client.session_end(&id, &req).await?))
    }

    pub async fn memory_get(&self, args: MemoryGetArgs) -> Result<Value> {
        let rid = non_empty("rid", args.rid)?;
        let memory = self.client.memory_get(&rid).await?;
        Ok(shape(
            serde_json::to_value(memory).map_err(|e| Error::Decode(e.to_string()))?,
        ))
    }

    pub async fn memory_current(&self, args: MemoryCurrentArgs) -> Result<Value> {
        let ns = non_empty("namespace", args.namespace)?;
        Ok(shape(self.client.current(&ns).await?))
    }

    pub async fn memory_list(&self, args: MemoryListArgs) -> Result<Value> {
        let q = MemoriesQuery {
            namespace: args.namespace,
            domain: args.domain,
            memory_type: args.memory_type.map(|t| t.as_str().to_owned()),
            source: None,
            q: None,
            limit: args.limit,
            offset: args.offset,
            order: args.desc.then(|| "desc".to_owned()),
        };
        Ok(shape(self.client.memories(&q).await?))
    }

    pub async fn memory_gaps(&self, args: MemoryGapsArgs) -> Result<Value> {
        let q = GapsQuery {
            namespace: args.namespace,
            min_count: None,
            limit: args.limit,
        };
        Ok(shape(self.client.gaps(&q).await?))
    }

    pub async fn memory_stats(&self, _args: MemoryStatsArgs) -> Result<Value> {
        Ok(shape(self.client.stats().await?))
    }

    pub async fn temporal_stale(&self, args: TemporalListArgs) -> Result<Value> {
        Ok(shape(
            self.client.temporal_stale(&temporal_query(args)).await?,
        ))
    }

    pub async fn temporal_upcoming(&self, args: TemporalListArgs) -> Result<Value> {
        Ok(shape(
            self.client.temporal_upcoming(&temporal_query(args)).await?,
        ))
    }

    pub async fn temporal_as_of(&self, mut args: TemporalAsOfArgs) -> Result<Value> {
        if args.stdin {
            args.query = read_stdin()?;
        }
        let query = non_empty("query", args.query)?;
        if args.top_k == Some(0) {
            return Err(Error::Invalid("`top_k` must be at least 1".into()));
        }
        let req = AsOfRequest {
            query,
            as_of: time::unix_secs(time::parse(&args.at, Utc::now())?),
            top_k: args.top_k,
            namespace: args.namespace,
            memory_type: args.memory_type.map(|t| t.as_str().to_owned()),
        };
        Ok(shape(self.client.temporal_as_of(&req).await?))
    }

    pub async fn trigger_list(&self, args: TriggerListArgs) -> Result<Value> {
        let q = TriggersQuery {
            limit: args.limit,
            trigger_type: None,
        };
        Ok(shape(self.client.triggers(&q).await?))
    }

    pub async fn trigger_history(&self, args: TriggerHistoryArgs) -> Result<Value> {
        let q = TriggersQuery {
            limit: args.limit,
            trigger_type: args.trigger_type,
        };
        Ok(shape(self.client.triggers_history(&q).await?))
    }

    pub async fn trigger_deliver(&self, args: TriggerActionArgs) -> Result<Value> {
        self.trigger_action("deliver", args).await
    }

    pub async fn trigger_acknowledge(&self, args: TriggerActionArgs) -> Result<Value> {
        self.trigger_action("acknowledge", args).await
    }

    pub async fn trigger_act(&self, args: TriggerActionArgs) -> Result<Value> {
        self.trigger_action("act", args).await
    }

    pub async fn trigger_dismiss(&self, args: TriggerActionArgs) -> Result<Value> {
        self.trigger_action("dismiss", args).await
    }

    async fn trigger_action(&self, action: &str, args: TriggerActionArgs) -> Result<Value> {
        let id = non_empty("id", args.id)?;
        Ok(shape(self.client.trigger_action(&id, action).await?))
    }

    pub async fn claim_add(&self, args: ClaimAddArgs) -> Result<Value> {
        let now = Utc::now();
        let unix = |t: Option<String>| {
            t.map(|t| time::parse(&t, now).map(time::unix_secs))
                .transpose()
        };
        let (valid_from, valid_to) = (unix(args.valid_from)?, unix(args.valid_to)?);
        if let (Some(from), Some(to)) = (valid_from, valid_to)
            && from > to
        {
            return Err(Error::Invalid(
                "`valid_from` is later than `valid_to`".into(),
            ));
        }
        let req = ClaimRequest {
            src: non_empty("src", args.src)?,
            rel_type: non_empty("rel_type", args.rel_type)?,
            dst: non_empty("dst", args.dst)?,
            namespace: args.namespace,
            polarity: args.polarity,
            modality: args.modality,
            valid_from,
            valid_to,
            confidence_band: args.confidence_band,
            source_memory_rid: args.source_memory_rid,
            weight: args
                .weight
                .map(|w| clamp("weight", w, 0.0, 1.0))
                .transpose()?,
        };
        guard("claim_add", args.force, &req)?;
        Ok(shape(self.client.claim(&req).await?))
    }

    pub async fn claim_list(&self, args: ClaimListArgs) -> Result<Value> {
        let q = ClaimsQuery {
            entity: non_empty("entity", args.entity)?,
            namespace: args.namespace,
        };
        Ok(shape(self.client.claims(&q).await?))
    }

    pub async fn procedure_surface(&self, args: ProcedureSurfaceArgs) -> Result<Value> {
        let query = non_empty("query", args.query)?;
        if args.top_k == Some(0) {
            return Err(Error::Invalid("`top_k` must be at least 1".into()));
        }
        let req = SurfaceRequest {
            query,
            top_k: args.top_k,
            domain: args.domain,
            namespace: args.namespace,
        };
        Ok(shape(self.client.procedures_surface(&req).await?))
    }

    pub async fn task_list(&self, args: TaskListArgs) -> Result<Value> {
        let q = TasksQuery {
            namespace: args.namespace,
            status: args.status,
        };
        Ok(shape(self.client.tasks(&q).await?))
    }
}

/// Server 0.20 sends `last_maintenance` as a JSON-encoded string; decoding it lets
/// `shape` round its floats and add ISO timestamps like everywhere else.
fn decode_maintenance(mut digest: Value) -> Value {
    let decoded = digest
        .get("last_maintenance")
        .and_then(Value::as_str)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(Value::is_object);
    if let Some(v) = decoded {
        digest["last_maintenance"] = v;
    }
    digest
}

fn temporal_query(args: TemporalListArgs) -> TemporalQuery {
    TemporalQuery {
        days: args.days,
        limit: args.limit,
        namespace: args.namespace,
    }
}

fn remember_request(m: MemoryInput, now: chrono::DateTime<Utc>) -> Result<RememberRequest> {
    Ok(RememberRequest {
        text: non_empty("text", m.text)?,
        memory_type: m.memory_type.map(|t| t.as_str().to_owned()),
        importance: m
            .importance
            .map(|v| clamp("importance", v, 0.0, 1.0))
            .transpose()?,
        valence: m
            .valence
            .map(|v| clamp("valence", v, -1.0, 1.0))
            .transpose()?,
        certainty: m
            .certainty
            .map(|v| clamp("certainty", v, 0.0, 1.0))
            .transpose()?,
        domain: m.domain,
        source: m.source,
        namespace: m.namespace,
        emotional_state: None,
        metadata: m.metadata,
        created_at: m
            .created_at
            .map(|t| time::parse(&t, now).map(time::unix_secs))
            .transpose()?,
        idempotency_key: None,
    })
}

fn non_empty(name: &str, s: String) -> Result<String> {
    if s.trim().is_empty() {
        Err(Error::Invalid(format!("`{name}` must not be empty")))
    } else {
        Ok(s)
    }
}

fn clamp(name: &str, v: f64, min: f64, max: f64) -> Result<f64> {
    if v.is_finite() {
        Ok(v.clamp(min, max))
    } else {
        Err(Error::Invalid(format!("`{name}` must be a finite number")))
    }
}

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| Error::Invalid(format!("reading stdin: {e}")))?;
    Ok(s.trim_end().to_owned())
}
