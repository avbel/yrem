# yantrikdb-server HTTP API notes (captured from a real run)

Fixtures re-captured 2026-10-31 from the NAS scratch build: yantrikdb-server 0.20.0 (`/v1/health/deep` -> `version.build_id: "0.20.0"`, wire 1.2), release build,
single-node, embedding strategy `builtin` (fastembed MiniLM, dim 384). All responses below are real, replayed in one run against a fresh scratch database
(ops/temporal/triggers/claims first, then the payments flow, then sessions and digests). Fixtures: this directory, one JSON per case
`{method,path,request,status,response}` (`auth: "none"|"bad"` marks unauthenticated cases). Same synthetic requests as the earlier potion-base-2M (dim 64) capture;
the response shapes did not change, only values did: scores/similarities (MiniLM), rids, timestamps, counts.

## Conventions
- Auth: `Authorization: Bearer ydb_...` (per-database token; `yantrikdb token --data-dir D create --db NAME`). The token selects the database; there is no db in the URL.
- Request bodies are free-form JSON read field by field: unknown fields are silently ignored; wrong-typed optional fields silently fall back to defaults. `Content-Type: application/json` is REQUIRED on POST bodies (else 415, plain-text body). Invalid JSON -> 400 plain-string body. Body limit 65536 bytes (health/deep `admission.max_request_body_bytes`).
- Error envelope (JSON): `{"error":{"code":"<id>","message":"...","hint"?:"..."}}`. Most handlers emit `code:"generic"`. Specific codes seen: `unauthenticated`, `memory_not_found`, `invalid_query_parameter`. Exceptions with different shapes: recall top_k cap (`{"error":"<string>","reason":"top_k_cap","hard_top_k_cap":1000}`), the 501 deferred body (below), and axum extractor errors (JSON string body, e.g. 400 "Failed to deserialize query string: missing field `namespace`", 422 "Failed to deserialize the JSON body ... missing field `summary`", 415).
- Auth failures: 401. Legacy handlers: `{"error":{"code":"generic","message":"missing Bearer token"}}` / `"invalid or revoked token"`. Handlers on the principal middleware (/v1/memories, /v1/memory/{rid}): `code:"unauthenticated"`, messages `"missing Bearer token"` / `"invalid or unknown token"`.
- Unknown route: 404 with empty body. Wrong method: 405 with empty body.
- "namespace" is a row-level TAG within the token's database (not a tenant). Omitted on remember -> stored as `"default"`.
- Timestamps are float epoch seconds; ids are UUIDv7 strings.
- /v1/health and /v1/health/deep need NO auth (SERVER_README says all endpoints do).
- Writes may also return 429 (quota: max_memories 1,000,000, max_batch_size 10,000), 503 (`{"error":"read-only: not the leader ...","leader_node_id",...}` in cluster mode), 307/409/426 (cluster commit errors). Not exercised (single node).

## GET /v1/health (no auth) -> 200
`{"status":"ok","engines_loaded":int,"capabilities":["idempotency_key"]}`; adds `cluster{}` when clustered; `status` may be `"quarantined"`/`"engine_backfilling"` in yrp mode. Fixture: health.json, health_unauth.json

## GET /v1/health/deep (no auth) -> 200 | 503
200: `{"status":"healthy","encryption":{"enabled":bool,"algorithm":null|"AES-256-GCM"},"checks":[{"check":"engine_lock","pass":bool,"latency_ms":float},{"check":"control_db","pass":bool,"latency_ms":float,"databases":int}],"admission":{"hard_top_k_cap":1000,"max_request_body_bytes":65536,"in_flight_recall":{"max","in_use"},"expanded_recall":{"max","in_use"}},"runtime":{"control_runtime_isolated":bool},"version":{"build_id":"0.20.0","wire":{"major","minor"},"min_supported_wire":{...},"table_schema_versions":[[name,int],...]}}`.
503: same shape with `status:"degraded"` and failing check(s) carrying `error` (not captured; cluster_quorum check appears only when clustered). NOTE: engine_lock probes the database named "default" (auto-created at startup), not the token's db. Fixture: health_deep.json

## GET /v1/stats -> 200
`{"active_memories","consolidated_memories","tombstoned_memories","edges","entities","open_conflicts","pending_triggers","operations"}` all ints. Fixtures: stats.json (empty db), stats_after.json, stats_final.json

## POST /v1/remember -> 200
Request: `text` string REQUIRED (400 `missing 'text'`); optional: `memory_type` string (default "semantic"; also "episodic" etc.), `importance` f64 (0.5), `valence` f64 (0.0), `half_life` f64 hours (168.0), `metadata` object ({}; `metadata.kind`, `confidence_basis`, `override_kind` feed the provenance gate), `namespace` string ("" -> shows as "default"), `certainty` f64 (1.0), `domain` string (""), `source` string ("user"), `emotional_state` string|null, `embedding` [f32] (client vector; else server embeds), `created_at` f64 epoch secs (historical import), `idempotency_key` string.
Response: `{"rid":string,"log_index":int}`. With `idempotency_key`: `{"rid":string}` only (no log_index); same key + same text returns the ORIGINAL rid; same key + different text returns 200 `{"stored":false,"idempotency_conflict":true,"rid":<existing>}` (from handler code, not captured); invalid key -> 400. Provenance gate violation -> 400 `provenance_inconsistent: ...`. 500 if embedder fails.
Fixtures: remember.json, remember_minimal.json, remember_idem.json, remember_idem_again.json, remember_conflict_a.json, remember_missing_text.json, unauthorized_remember.json

## POST /v1/remember/batch -> 200
Request: `{"memories":[<same item fields as remember, text required each>], "idempotency_key"?:string}` (item keys derive `"{key}:{index}"`; per-item `idempotency_key` also allowed). Missing/non-array `memories` -> 400; item w/o text -> 400 `memories[i]: missing 'text'`; empty array -> 200 `{"rids":[],"count":0}`; > max_batch_size -> 429; > 10000 -> 400.
Response: `{"rids":[string],"count":int,"log_index":int}`. Fixture: remember_batch.json

## POST /v1/recall -> 200
Request: `query` string REQUIRED (400 `missing 'query'`); optional `top_k` u64 (10; > hard cap 1000 -> 400 top_k_cap shape), `memory_type`, `namespace`, `domain`, `source` (string filters), `include_consolidated` bool (false), `expand_entities` bool (TRUE by default), `include_superseded` bool (false), `certainty_min` f64, `skip_reinforce` bool (false; otherwise recall mutates access_count), `query_embedding` [f32]. There is NO `time_window` field (ignored).
Response: `{"results":[{"rid","text","memory_type","namespace","domain","source","importance","valence","certainty","created_at","metadata":{},"score":f64,"why_retrieved":[string]}],"total":int,"fallback":null|"fts5_keyword"}`. `fallback` is "fts5_keyword" whenever any hit has "keyword_match" in why_retrieved. why_retrieved strings are free text (e.g. "semantically similar (0.89)", "recent", "graph-connected via PostgreSQL", "unresolved high conflict (preference) with <rid> - verify before relying" with a leading warning sign). Admission rejection (capacity) -> `{"error":str,"reason":str,"retry_after_ms":200}` with 429/503.
Fixtures: recall.json, recall_minimal.json, recall_filtered.json, recall_empty.json, recall_missing_query.json, recall_topk_cap.json, recall_after_forget.json

## POST /v1/forget -> 200
Request: `rid` string REQUIRED (400 `missing 'rid'`); optional `namespace` string, `reason` string.
Response: `{"rid","found":true,"log_index":int}`. `found` is ALWAYS true even for a nonexistent rid (forget_unknown.json; no 404). Tombstoned memories remain readable via GET /v1/memory/{rid} with `consolidation_status:"tombstoned"` (tombstone_reason came back null) and are excluded from recall.
Fixtures: forget.json, forget_unknown.json, forget_missing_rid.json, memory_get_after_forget.json

## POST /v1/correct -> 501 (always)
Body ignored (any/no JSON). Response: `{"error":"not_yet_available_over_http","operation":"correct","reason":"This is a cluster-global write with no replication path yet; exposing it as a direct engine call would silently diverge across the cluster. Available in embedded (single-node) mode; replicated HTTP support is tracked in RFC 032.","deferred":true}`. NOTE `error` is a plain string here, not the envelope object. Same shape for task/category/procedure deferred writes. Fixture: correct.json

## POST /v1/think -> 200
Request (all optional; send `{}` - JSON content-type still required): `run_consolidation` bool (true), `run_conflict_scan` bool (true; alias `run_conflicts`), `run_pattern_mining` bool (FALSE; alias `run_patterns`), `run_personality` bool (true), `consolidation_limit` u64 (50), `consolidation_time_window_days` f64, `min_active_memories` i64, `max_triggers` u64.
Response: `{"consolidation_count":int,"conflicts_found":int,"patterns_new":int,"patterns_updated":int,"personality_updated":bool,"duration_ms":int,"triggers":[{"trigger_type":"redundancy"|"potential_conflict"|"surprise_connection"|...,"reason":string,"urgency":f64,"source_rids":[string],"suggested_action":string}]}`.
With MiniLM `consolidation_count` can be > 0: near-duplicate memories are consolidated (think.json: 2; stats then show `consolidated_memories` 3). Triggers already persisted by an earlier think (decay_review, temporal_drift) are listed again. Fixtures: think.json (found the MySQL-vs-PostgreSQL conflict, conflicts_found 1, consolidation_count 2, 4 triggers), think_full.json (pattern mining on: patterns_new 8, patterns_updated 1; conflicts_found 0 because already recorded)

## POST /v1/relate -> 200 (DEPRECATED)
Request: `entity` string REQUIRED, `target` string REQUIRED, `relationship` string REQUIRED (each missing -> 400 `missing '<field>'`); optional `weight` f64 (1.0), `namespace` string (""). Response: `{"edge_id":string,"log_index":int}` with response headers `deprecation: true` and `link: </v1/claim>; rel="successor-version"`. Fixtures: relate.json, relate2.json, relate_missing.json

## POST /v1/graph -> DOES NOT EXIST (404, empty body); GET /v1/graph also 404
No HTTP route for graph/edge queries in 0.20.0 (the wire protocol has `Command::Edges`, not exposed over HTTP). Related routes in the router (NOT exercised): POST /v1/claim, GET /v1/claims, POST /v1/alias. Fixtures: graph_post.json, graph_get.json, graph_edges.json (all 404).

## GET /v1/conflicts -> 200
Query (all optional): `status` ("open"|"resolved"|...), `conflict_type`, `entity`, `namespace`, `limit` (default 50). Response: `{"conflicts":[{"conflict_id","conflict_type" ("preference" seen),"priority" ("high" seen),"status":"open"|"resolved","memory_a":rid,"memory_b":rid,"entity":string|null,"detected_at":f64,"detection_reason":string}]}`.
Fixtures: conflicts.json (open), conflicts_filtered.json, conflicts_after.json (status=resolved), conflicts_open_after.json (empty list)

## POST /v1/conflicts/{id}/resolve -> 200
Request: `strategy` string REQUIRED (400 `missing 'strategy'`; values per repo docs: keep_a, keep_b, keep_both, merge - only keep_b exercised); optional `winner_rid` string, `new_text` string (for merge), `resolution_note` string. Response: `{"conflict_id","strategy"}`. Unknown id -> 500 (not 404) `memory not found: conflict: <id>`; already resolved -> 500 `sync error: conflict <id> is already resolved`. The losing memory is tombstoned.
Fixtures: conflict_resolve.json, conflict_resolve_notfound.json, conflict_resolve_again.json, conflict_resolve_missing_strategy.json

## GET /v1/session/digest -> 200
Query (all optional): `namespace` (narrative chain), `scope` (isolates content aggregates to a namespace), `max_decisions` (8), `max_conflicts` (5), `max_triggers` (5), `snippet_chars` (240), `include_gaps` bool (false), `max_gaps` (5), `include_superseded` bool (false).
Response: `{"narrative_head":null|{..},"top_decisions":[{"rid","namespace","snippet","importance","created_at","current_status","disputed":bool,"superseded_by":null|rid}],"open_conflicts":[..],"open_conflict_count":int,"pending_triggers":[{"trigger_id","trigger_type","reason","urgency"}],"pending_trigger_count":int,"last_maintenance":null|{..},"knowledge_gaps"?:[gap]}` (knowledge_gaps only with include_gaps=true).
Fixtures: session_digest.json (scope=payments&include_gaps=true), session_digest_default.json

## POST /v1/session/end (end-of-session capture; NOT the same as DELETE /v1/sessions/{id})
Request: `summary` string REQUIRED (field missing -> 422 plain JSON string; blank -> 400 `summary must not be empty`); optional `namespace` ("default"), `domain` ("general"). Response: `{"drafted":[rid],"count":int}` - the summary is split into atomic provisional memories (`source:"session_auto_capture"`, `metadata:{kind:"session_auto_capture",provisional:true}`, certainty 0.7). 409 on a non-writable cluster node.
Fixtures: session_end.json (same as session_end_capture.json), session_end_empty.json, session_end_missing.json

## POST /v1/sessions -> 200 (start)
Request (all optional; body must still be JSON e.g. `{}`): `namespace` ("default"), `client_id` (""), `metadata` object ({}). Response: `{"session_id":uuid7}`. Fixtures: session_start.json, session_start_default.json

## DELETE /v1/sessions/{id} -> 200 (end)
Optional JSON body `{"summary":string}`. Response: `{"session_id","duration_secs":f64,"memory_count":int,"topics":[string]}`. Unknown id -> 500 `{"error":{"code":"generic","message":"database error: Query returned no rows"}}` (not 404).
Fixtures: session_delete.json, session_delete_default.json, session_delete_unknown.json

## GET /v1/memory/{rid} -> 200 | 404
Query: `namespace` (ignored), `min_seq` u64 (412 `replica_behind` if node behind). Response (flat object): `rid, type, text, created_at, created_at_iso, updated_at(null), updated_at_iso(null), importance, half_life, last_access, access_count, valence, certainty, domain, source, namespace, emotional_state, consolidation_status ("active"|"tombstoned"|...), consolidated_into, storage_tier, session_id, due_at, temporal_kind, tombstone_reason, embedding_bytes, embedding_model, metadata_json (object), consolidation_sources [], entities [], claims []` (last three always empty). Note: memory type key is `type` and metadata key is `metadata_json` here, vs `memory_type`/`metadata` in recall. 404: `{"error":{"code":"memory_not_found","message":"memory not found: <rid>"}}`.
Fixtures: memory_get.json, memory_get_404.json, memory_get_after_forget.json, memory_get_unauth.json, memory_get_bad_token.json

## GET /v1/memories -> 200
Query: `namespace` (row tag filter), `domain`, `memory_type`, `limit` (1..=200, default 50; else 400 invalid_query_parameter), `offset` (0), `sort` (created_at|importance|last_access; updated_at/access_count/certainty -> 400 "not implemented"), `status` (only "active"), structural params `kind`, `drive_id`, `since_rid`, `order` ("asc"|"desc"). UNSUPPORTED (400): `q`, `source`.
Two response shapes. Legacy (none of kind/drive_id/since_rid/order=desc): `{"items":[<row as in memory_get>],"total":int,"limit":int,"offset":int}`. Structural (any of kind, drive_id, since_rid, or order=desc): `{"records":[{rid,text,memory_type,namespace,domain,source,importance,certainty,created_at,created_at_iso,metadata}],"next_cursor":rid|null,"limit":int}`.
Fixtures: memories.json (legacy), memories_desc.json (structural), memories_bad_param.json, memories_unauth.json

## GET /v1/current?namespace=X -> 200 | 404
`namespace` REQUIRED (missing -> 400 JSON string; empty -> 400). Returns the newest active record in that namespace (greatest UUIDv7 rid, not a supersession lookup): `{"found":true,"namespace":X,"record":{rid,text,memory_type,namespace,domain,source,importance,certainty,created_at,created_at_iso,metadata}}`. Empty namespace -> 404 `{"error":{"code":"generic","message":"no chain head for namespace 'X'"}}`.
Fixtures: current.json, current_404.json, current_missing_ns.json

## GET /v1/insights/gaps -> 200
Query: `namespace`, `min_count` (2), `max_avg_top_score` (0.5), `limit` (10). Response: `{"count":int,"gaps":[{"query","count","avg_top_score","avg_results","last_seen","last_seen_iso"}]}`. Gaps = queries recalled >= min_count times with low mean top score (fixture built by recalling an unrelated query 3x).
Fixtures: gaps.json, gaps_loose.json

## GET /v1/temporal/stale and GET /v1/temporal/upcoming -> 200
Query (all optional): `days` (stale 30 / upcoming 7; clamped to 0..=3650, unparseable -> default), `limit` (20, capped 1000), `namespace`. Response: `{"memories":[row],"count":int}`; row = `rid, text, memory_type, namespace, domain, source, importance, certainty, valence, half_life, created_at, last_access, access_count, consolidation_status, consolidated_into, storage_tier, session_id, emotional_state, due_at, temporal_kind, metadata` (no `_iso` fields; `memory_type`/`metadata` naming as in recall). Stale = not accessed within `days` (fixture seeded with a record created 90 days back via `created_at`); `days=0` returns everything.
`due_at` / `temporal_kind` are NOT settable via /v1/remember (ignored; a `due_at` inside `metadata` is just metadata, text dates are not parsed), so `upcoming` is always empty over HTTP in 0.20.0.
Fixtures: temporal_stale.json, temporal_stale_params.json (days=0&limit=2), temporal_upcoming.json (empty)

## POST /v1/temporal/as_of -> 200
Request: `query` string REQUIRED (400 `missing 'query'`), `as_of` f64 unix seconds REQUIRED (400 `missing 'as_of' (unix seconds)`); optional `top_k` (10; > hard cap 1000 -> 400 `{"error","reason":"top_k_cap","hard_top_k_cap":1000}`, a flat error shape), `namespace`, `memory_type`, `query_embedding` [f32]. Response: `{"results":[<recall row>],"count":int,"as_of":f64}`; result rows have the same shape as /v1/recall results (`rid,text,score,scores{..},why_retrieved,current_status,superseded_by,...`). Memories created after `as_of` are excluded (as_of in the past -> `count:0`); the `as_of` is echoed back.
Fixtures: temporal_as_of.json, temporal_as_of_empty.json, temporal_as_of_missing_query.json, temporal_as_of_missing_as_of.json, temporal_as_of_topk_cap.json

## GET /v1/triggers and GET /v1/triggers/history -> 200
Query: `limit` (20 / 50, capped 1000); history also `trigger_type`. Response: `{"triggers":[trigger],"count":int}`; trigger = `trigger_id, trigger_type ("decay_review"|"temporal_drift"|...), reason, urgency f64, suggested_action, source_rids [rid], context {..type specific}, status ("pending"|"delivered"|"acknowledged"|"acted"|"dismissed"), created_at, expires_at (+7d), delivered_at, acknowledged_at, acted_at` (timestamps null until set). /v1/triggers = pending only; history = all statuses (pending included), with `trigger_type` filter. Triggers come from POST /v1/think (`{"triggers":[{trigger_type,reason,urgency,suggested_action,source_rids}],...}` in its response; no trigger_id there); with fresh recent data think yields none.
Fixtures: triggers.json (2 pending), triggers_limit.json, triggers_after.json (1 pending decay_review of a second identical-text stale note; every other pending trigger was dismissed first), triggers_history_before.json, triggers_history.json, triggers_history_filtered.json

## POST /v1/triggers/{id}/{action}
Actions: `acknowledge|deliver|act|dismiss`; body ignored (none needed). Node-local, no replication. 200 `{"trigger_id","action","changed":true,"node_local":true}`. Unknown action -> 400 `{"error":{"code":"generic","message":"unknown trigger action 'x' (acknowledge|deliver|act|dismiss)"}}`. Unknown id OR a transition the current state does not permit -> 404 `trigger '<id>' not found or not in a state that permits '<action>'`. Observed state machine: pending -deliver-> delivered -acknowledge-> acknowledged -act-> acted; dismiss works from pending; acknowledge/act from pending -> 404; repeating a transition -> 404; dismiss after act -> 404.
Fixtures: trigger_deliver.json, trigger_acknowledge.json, trigger_act.json, trigger_dismiss.json, trigger_deliver_again.json, trigger_acknowledge_from_pending.json, trigger_dismiss_after_act.json, trigger_act_404.json (unknown id), trigger_act_bad_action.json

## POST /v1/claim -> 200
Request: `src`, `rel_type`, `dst` strings REQUIRED (each 400 `missing '<field>'`); optional `namespace` ("default"), `polarity` int (1), `modality` ("asserted"), `valid_from`/`valid_to` f64, `extractor` ("manual"), `extractor_version`, `confidence_band` ("medium"), `source_memory_rid`, `span_start`/`span_end` int, `weight` f64 (1.0). Response: `{"claim_id":uuid7,"namespace"}`. Successor of the deprecated POST /v1/relate (its response carries `deprecation: true` + `link: </v1/claim>; rel="successor-version"` headers).
Fixtures: claim.json (all fields), claim_minimal.json, claim_missing_src.json, claim_missing_rel_type.json, claim_missing_dst.json

## GET /v1/claims -> 200
Query: `entity` REQUIRED (400 `missing 'entity' query parameter`), `namespace` optional (omitted -> not filtered by "ops"; claim in "default" is found with no namespace). Response: `{"claims":[{claim_id,src,rel_type,dst,namespace,polarity,modality,valid_from,valid_to,extractor,confidence_band,source_memory_rid,weight,grounding int,status_suggestion ("active"),created_at}]}`; unknown entity -> `{"claims":[]}`.
Fixtures: claims.json, claims_default_ns.json, claims_empty.json, claims_missing_entity.json

## POST /v1/procedures/surface -> 200
Request: `query` string REQUIRED (400 `missing 'query'`); optional `top_k` (5, hard cap 1000 like as_of), `domain`, `namespace`, `query_embedding`. Response: `{"results":[<recall row>],"count":int}` over `memory_type:"procedural"` memories (seed with /v1/remember `memory_type:"procedural"`). No similarity floor: an unrelated query still returns the procedural memory with a low score.
Fixtures: procedures_surface.json, procedures_surface_unrelated.json, procedures_surface_missing.json

## GET /v1/tasks -> 200 (read-only over HTTP)
Query: `namespace` ("default"), `status`. Response: `{"tasks":[..],"count":int}`; always empty here because tasks cannot be created over HTTP. GET /v1/tasks/{id} unknown -> 404 `task '<id>' not found`.
501 (deferred cluster-global writes, flat body `{"error":"not_yet_available_over_http","operation":"task.add","reason":"...","deferred":true}`): POST /v1/tasks, PATCH/DELETE /v1/tasks/{id}, POST /v1/procedures, POST /v1/procedures/{rid}/reinforce, category learn/reset, correct.
Fixtures: tasks.json, tasks_filtered.json, task_get_404.json, tasks_add_501.json, procedures_learn_501.json

## Other observations
- Extra routes confirmed working: GET /v1/personality (`{"traits":[{"name","score"}]}`), GET /v1/cluster (`{"clustered":false,"message":"single-node mode (no replication)"}`). Others exist (/v1/databases, /v1/claim(s), /v1/alias, /v1/skills/*, /v1/temporal/*, /v1/categories, /v1/tasks, /v1/procedures, /v1/triggers, /v1/jobs, /metrics, /admin).
- Missing Content-Type on POST -> 415 (no_content_type_415.json). Wrong method -> 405 empty body (wrong_method.json). Invalid JSON -> 400 plain string (invalid_json.json). Unauthenticated: unauthorized.json (GET /v1/stats, no token), bad_token.json.
- Fixture data is synthetic (invented) content; the server's timestamps and rids are real from the capture run.
