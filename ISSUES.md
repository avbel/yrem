# yrem issues (2026-11-14)

What to change in yrem so that memory stops injecting noise, duplicating itself and leaking
secrets. Every issue below is backed by a measurement. Priorities: **P0** leaks data, **P1**
degrades every session, **P2** wrong but contained. "Fix in" says where the change belongs:
yrem (this repo), server (yantrikdb-server, upstream) or data (the store itself).

## Data sources

| Source | What it covers |
|---|---|
| Claude Code transcripts, 2026-11-07..14 | 97 interactive sessions, 593 prompts, every hook injection and MCP call |
| Codex rollouts, same week | 16 interactive sessions, 11 `codex exec` runs, 27 guardian subagents |
| Server DB snapshot, 2026-11-14 06:24 UTC | `yantrik.db` copied with the container stopped: memories, `recall_impressions`, `recall_demand`, `trigger_log`, `sessions`, `oplog` |
| Live replay | 26 real prompts from this week re-run against the server with `skip_reinforce: true`, comparing the current gate with the proposals below |

Store at the time: 528 active memories, 496 consolidated (merge sources, hidden), 183 tombstoned.

## 1. Secrets and privacy

### 1.1 The digest carried a PIN into every session (P0, yrem + data)

The briefing had one "live decision" holding the parent-control offline-unlock PIN in plain
digits. It was injected 188 times this week, into about 100 sessions across Claude Code and
Codex, including `codex exec` runs, so the PIN reached both model providers. The secret guard let
it through: "PIN as of 2026-10-18 22:15: <8 digits>" has no `key=value` shape, no known prefix
and too little entropy.

Fix in yrem:
- A `pin_code` detector: a PIN-like keyword (`pin`, `pincode`, `passcode`, `*_pin`, `*-pin`)
  followed within a few tokens by a 4–12 digit run that is not a year. Running it over the 528
  active memories must flag only the PIN rows (check port numbers, "SPKI pin <hex>" and dates).
- A `sensitive` flag on `remember` / `correct` (stored as `metadata.sensitive = true`). The digest
  and the prompt hook never inject sensitive rows. Explicit `recall` and `memory_get` still
  return them.

Data, done 2026-11-14: both memories that held the digits were corrected with the digits
replaced by `<redacted>` and flagged `sensitive`. Their two consolidation sources (still holding
the digits, hidden from `memory list`) were forgotten. See 1.2 for what forgetting leaves behind.

### 1.2 `forget` and `correct` are soft deletes (P0, server; yrem documents and cascades)

After the API fix, the three PIN values were still in `yantrik.db`: tombstoned `memories.text`
(4 rows) and the FTS index over them, `oplog.payload` (44 entries), `trigger_log.reason/context`
(30), and `commit_log.sqlite` (6). They were removed by hand: container stopped, data backed
up, SQL scrub with FTS `optimize` and `VACUUM`, integrity checks, restart. Nothing in yrem can
do this.

- yrem: `forget --cascade` (and the same for `correct`). When the rid is a merge
  (`metadata.consolidated_from`), also forget its sources. The PIN survived in exactly such
  sources.
- yrem: say in `forget --help`, the README and the secret-guard refusal that forgotten text stays
  on the server.
- Server (upstream): a purge operation that removes the text from every table and from the
  replication log.

### 1.3 Recall queries are stored verbatim on the server (P0, yrem)

`recall_demand` keeps every query: 4,296 rows. 109 distinct queries contain credential words
(`password`, `token`, `secret`, `credentials.json`). One came from an IDE selection inside
`.sessions/saleroom.credentials.json`. The prompt hook sends the raw prompt, including
`<ide_selection>` / `<ide_opened_file>` blocks.

Fix in yrem, prompt hook:
- Strip every `<ide_*>…</ide_*>` block. If a block has no closing tag, drop the whole query.
- Run `secrets::scan` over what is left and replace each finding with `<redacted>` before sending.
- The scan would not catch a short password typed without a keyword in front, so stripping the
  IDE blocks is the main defence.

## 2. False and irrelevant injections

### 2.1 Measured precision (P1)

This week the prompt hook injected 33 times (46 hits) over 593 prompts. Rated by hand:

| Rating | Injections | Where |
|---|---|---|
| Clearly useful | about 9 | auction-bot Saleroom work, Fluidd valuation, NAS cleanup, "Done closes the issue" |
| Marginal | about 12 | mostly the same Fluidd board rule repeated after subagent hand-backs |
| Noise | about 12 | every vaultkeeper injection (`commit` → Dokploy vault-provider, move2ts CI, dbhub) and off-project imported "Context:" summaries |

Codex: one injection in 60 interactive prompts.

### 2.2 Harness text is treated as a user prompt (P1, yrem)

- Server-wide, 848 of 4,515 recall calls (19%) were harness text. Top offenders: `Reply with the
  single word OK.` (×17 plus variants), Codex's `# Overview Generate 0 to 3 hyperpersonalized
  suggestions…`, `# Code Review Brief…`, `<system-reminder> … session started from another
  session`.
- This week, 10 of the 33 injections were triggered by `<task-notification>` or `Another Claude
  session sent a message: <agent-message …>` (subagent hand-backs).
- Headless sessions get the full treatment. The digest reached all 11 `codex exec` runs while
  missing 10 of 16 interactive Codex sessions, and 3 of the 4 Codex recall injections were in
  `exec` runs.

Fix in yrem:
- Run the prompt through the same harness filter as capture (`transcript::META_PREFIXES`,
  extended with `Another Claude session sent a message`, `<agent-message` and the Codex
  suggestion prompt) before the length check.
- Skip digest and recall when the session is headless: `CLAUDE_CODE_ENTRYPOINT` starting with
  `sdk`, or a transcript head whose `session_meta` says `codex_exec` or `subagent`.
- Check that a Codex SessionStart sees `session_meta` already written.

### 2.3 The gate drops relevant keyword hits and keeps off-project semantic ones (P1, yrem)

The gate needs a semantic-lane cosine of at least 0.50. Hits matched by keyword and graph only
are always dropped. Example: "what is the saleroom OTP flow" finds 4 Saleroom memories (score
0.30–0.35) and injects none.

The replay of 26 real prompts compared variants:

| Variant | Added useful hits | Added noise | Removed noise |
|---|---|---|---|
| Accept keyword + ≥2 lanes + score ≥ 0.30 anywhere | 6 prompts (Fluidd, auction-bot) | 9 hits (vaultkeeper `commit` → 3 Sui notes, etc.) | 0 |
| **Same, but only for hits in the current project's namespace** | same 6 prompts | **0** | 0 |
| **Plus a 0.60 semantic floor for hits outside the project that do not name it** | — | 0 | 7 (move2ts, dbhub, ai-notes, Dokploy vault-provider ×2, rpc-node, codepilot) |

The last variant loses one marginal hit (Vaultwarden SSH-agent, cosine 0.54, for a vaultkeeper
prompt). Off-project noise sat at cosine 0.51–0.56. Useful default-namespace hits scored about
the same but always named the project ("Fluidd …", "auction-bot …").

Fix in yrem: work out the current project from `cwd` (see 3.1), then gate like this:
- A hit in the project's namespace, or whose text names the project, passes at cosine ≥ 0.50,
  or by keyword + ≥2 lanes + score ≥ 0.30.
- Any other hit needs cosine ≥ 0.60.
- Sort project hits first.
- Update the calibration note in `gate.rs`.

### 2.4 Recall reinforces everything it returns (P1, yrem)

Every recall impression is a `reinforce` op (43,733 of each). `access_count` follows impressions
exactly (correlation 1.00), and `half_life` grows with access (0.55). The prompt hook asks for 10
candidates and injects at most 3, usually 0, so every prompt strengthens up to 10 memories the
gate just rejected. This week: 10,937 reinforce ops from 1,116 recall calls.

The result is hub memories. "Token analysis of dev-* skill sessions" was in the top 3 for 365 of
this week's queries. The server documents `skip_reinforce` (`tests/fixtures/README.md`), but
yrem never sends it.

Fix in yrem:
- `skip_reinforce: true` on the prompt-hook recall and on the capture duplicate check.
- Expose `skip_reinforce` on MCP/CLI `recall`.

### 2.5 The entity graph links unrelated memories through generic words (P2, server; yrem works around it)

- Active memories average 24.2 entity links. 34 entities sit on at least 10% of them (`sui` 211,
  `node` 156, `api` 156, `json` 99, `project` 92, `docker` 88, `url` 82, `http` 77, `status` 66,
  `none` 61).
- 2,550 of the 2,551 edges are `co_occurs_with`.
- In the replay, 67% of top-10 hits carried a "graph-connected via" reason, mostly through `API`
  (44), `POST` (15), `AI` (14), `Users` (10), `Account` (9), `Amount` (6).
- Turning `expand_entities` off changes only 5 of 260 hits, but these links inflate
  "multi-lane agreement".

Fix:
- yrem: do not count graph lanes when the gate checks lane agreement.
- Server: an entity stoplist or IDF weighting.

### 2.6 Imported Hindsight summaries dominate off-project noise (P1, data)

- 252 of 528 active memories are `Context: …` session summaries, 204 of them with
  `source = hindsight` (the importer from the memory system removed 2026-10-31).
- They took 35% of this week's top-3 recall slots.
- Every off-project noise hit in 2.3 was one of them.

Fix:
- yrem: a `YREM_HOOK_EXCLUDE_SOURCES` setting, or a higher floor for `source=hindsight`.
- Data: review the 204 rows. 45 of them have never been in a top 3.

### 2.7 One memory is re-injected in session after session (P2, yrem)

Dedup is per session only. "Setting a Fluidd task to Done closes the issue" was injected in 9
sessions this week, 6 of them after subagent hand-backs.

Fix:
- The harness filter (2.2) removes most of these.
- Also a per-project cooldown: do not re-inject a rid shown in the last 24 h in the same project
  unless its cosine is ≥ 0.65.

## 3. Briefing (digest)

### 3.1 The digest is the same everywhere (P1, yrem)

One digest text served all 188 injections, whatever the project. The server's `top_decisions` is
the store's all-time highest importance (about 0.99): 4 September byzantion notes plus the PIN
note. `namespace` only selects the narrative head ("Decisions and conflicts are not limited to
it").

Not a cause: tested 2026-11-14, `session_digest` and `memory list` do not raise `access_count`;
only `recall` does.

Fix in yrem:
- Build the decision list client-side from `memory list` (newest first, no reinforcement):
  - up to `YREM_HOOK_DECISIONS` rows from the current project's namespace;
  - plus recent rows (last 7 days) from any namespace;
  - skipping sensitive, auto-captured and excluded rows.
- Project from `cwd`: the main worktree's directory name (worktrees such as `fluidd-wt-306` and
  `.claude/worktrees/agent-*` map to `fluidd`), plus `YREM_HOOK_NAMESPACE_ALIASES`
  (`bw-lite-desktop=vaultkeeper`).
- Check that ranking by importance inside a project does not bring back a static list. Run it
  against the live server for fluidd, auction-bot and vaultkeeper and read the output.

### 3.2 Maintenance is hidden, and most of it is noise (P2, yrem + server)

- Triggers: 12,646 in `trigger_log`, none ever acknowledged or acted on. 2,607 were created this
  week (`temporal_drift` 1,796, `consolidation_ready` 537, `session_awareness` 147).
- Conflicts: all 6 open conflicts were "roles substitution" false positives: unrelated Fluidd
  notes mentioning `data`/`ml`/`backend`/`mobile`/`platform`. 3 pointed at rows that no longer
  exist. All closed 2026-11-14 with `keep_both`.

Fix:
- yrem: `YREM_HOOK_MAINTENANCE=daily` as the default, at most once a day across sessions. Show
  open conflicts only, not trigger counts nobody can act on, and skip conflicts whose rows are
  gone.
- Server: the roles-substitution heuristic.

### 3.3 Codex gaps (P2, yrem)

4 interactive `codex-tui` sessions with prompts got no digest. No re-injection after Codex
compaction (known: no `compact` source).

## 4. Writes and duplicates

### 4.1 Writing has stopped (P1, yrem + config)

- The last memory was written 2026-11-08 09:46 UTC, against 25–33 a week in October.
- Agents called `remember` twice in about 650 interactive prompts, and capture is off.
- vaultkeeper (57 sessions this week, formerly bw-lite-desktop) has 0 memories. Its facts went to
  Claude's `MEMORY.md`, which Codex cannot read.

Fix:
- Turn capture on, but only after project namespaces ship (4.2, 4.3).
- Decision 2026-11-14: project facts live in yrem; `MEMORY.md` keeps only Claude-specific working
  preferences.

### 4.2 Captures go to the server default namespace (P1, yrem)

Capture, the duplicate check and the tracked session all use `YREM_HOOK_NAMESPACE`, which is
unset, so everything lands in `default`. Fix: when unset, use the project namespace from 3.1.

### 4.3 Turning capture on also injects captured rows (P1, yrem)

`include_captured` is tied to `capture`. The `gate.rs` measurements name auto-captured prompts as
the main source of noise. Fix: separate `YREM_HOOK_INJECT_CAPTURED` setting, default off.

### 4.4 Memories are too long, and MCP recall returns all of it (P1, yrem + data)

- 227 of 528 active memories exceed 1,500 chars (770k chars in total). 156 of them are server
  merges: `A | B` concatenations, clusters of up to 10, median 1,805 chars, maximum 20,149.
- MCP `recall` returns full text and metadata: a median of 16.5k chars (about 4k tokens) per
  call, 88.7k chars for 6 calls this week. That is 4.7 times all the prompt-hook injections
  combined.

Fix:
- yrem: MCP `recall` returns `rid`, `namespace`, `score`, date, `why_retrieved` and a 300-char
  snippet unless `full: true`; `memory_get` gives the whole text. CLI output is unchanged.
- yrem: `remember` adds a warning when a single text exceeds 1,500 chars.
- Data: split the 30 most-recalled long memories, keeping their original `created_at`.
- `think` runs at every session end and may merge the pieces again.

### 4.5 Where the duplication actually is (P2, yrem)

Active near-duplicates are rare: 0 exact, 6 pairs at cosine ≥ 0.85, 1 at ≥ 0.88 (two
`omp-smoke` test rows). The duplication is structural: 227 merges repeat the text of 499 hidden
sources, and forgetting a merge leaves its sources (1.2).

New writes will duplicate once they resume. `remember` has no similarity check; capture has one
at 0.92. Fix: `remember` returns a warning with the matching rid when a ≥ 0.92 match exists
(checked with `skip_reinforce`), suggesting `correct`.

### 4.6 Tracked sessions are left open (P2, yrem)

134 tracked sessions were opened this week. 16 were never closed and 2 received memories. 54
are abandoned all-time. Fix: close the previous slot on resume (the 409 path) and on the next
session start of the same harness session.

## 5. Client and server drift

| Issue | Measured | Fix in yrem |
|---|---|---|
| `conflict resolve --strategy dismiss` | HTTP 500 "unknown resolution strategy: dismiss" on server 0.20.0 | Drop it, or map it to `keep_both` with a note |
| `correct` | Server answers 501. Emulated as get → remember → forget: not atomic, `created_at` becomes now | Pass the old `created_at` on the emulated remember, so a redaction does not look new to a recency digest |
| `skip_reinforce`, `expand_entities`, `include_consolidated`, `certainty_min` | Supported by the server, never sent | Expose them; the hooks use `skip_reinforce` (2.4) |
| `time_window` | Ignored by the server; yrem filters after the `top_k` cut | Over-fetch when `--since` / `--until` is set |
| `"fallback": "fts5_keyword"` | Present on every recall, even with a semantic lane | Drop the field, or document that it names the keyword engine |

## 6. Plan and status

| # | Work | Issues | Status |
|---|---|---|---|
| 1 | Remove the secret; `pin_code` detector; `sensitive` flag | 1.1, 1.2 | Data done 2026-11-14; code done 2026-11-14 (`pin_code` flags 0 of the 528 active rows; `--sensitive` on `remember` / `correct`, never injected by the hooks). A merge made by `think` does not inherit the flag, so the hooks read a merge's sources, however deep, before showing it (briefing, narrative head and prompt recall) and drop it when one is sensitive or unreadable |
| 2 | Prompt filter, IDE stripping, query redaction, headless skip | 1.3, 2.2, 2.7 | Done 2026-11-14, per-project cooldown included; a prompt whose IDE block cannot be told from its text (never closed, or a stray `<ide_` left over) sends no query; `Reply with the single word` and `# Code Review Brief` join the harness prefixes; headless sessions also open no tracked session and run no `think`. Codex `session_meta`: in the sampled rollouts it is line 0, written 1.2-1.7 s before the briefing record (3 `codex exec`, 1 desktop), so a SessionStart hook that reads the rollout head sees it; not exercised end to end |
| 3 | Project namespace from cwd plus aliases; client-side digest | 3.1, 4.2 | Done 2026-11-14 (checked on the live store for fluidd, auction-bot, vaultkeeper). The narrative head is shown only when it names a memory that passes the same checks; the digest and the listings are fetched at once within 8 s |
| 4 | Project-aware gate; `skip_reinforce`; graph lanes not counted | 2.3, 2.4, 2.5 | Done 2026-11-14 (server entity stoplist still upstream). A name of 5+ characters matches in text as a whole word not followed by `-`; shorter names match by namespace only |
| 5 | Capture on after release; separate inject-captured switch; facts into yrem | 4.1, 4.3 | `YREM_HOOK_INJECT_CAPTURED` done 2026-11-14; decision made, config after release |
| 6 | MCP recall snippets; remember length and duplicate warnings; split the top 30 | 4.4, 4.5 | Code done 2026-11-14 (snippets in the MCP tool only; `yrem call recall` keeps the CLI shape); data split pending |
| 7 | Daily maintenance (conflicts only); close the 6 conflicts | 3.2 | Conflicts closed 2026-11-14; `YREM_HOOK_MAINTENANCE=daily` done 2026-11-14 (the slot is taken only for a briefing that shows conflicts; conflicts whose rows are gone are not skipped yet) |
| — | `dismiss` 500, `correct` dates, `forget --cascade`, open tracked sessions | 1.2, 4.6, 5 | `dismiss` (sent as `keep_both`), `correct` dates (the emulated record drops the consolidation keys), `forget --cascade` and `correct --cascade`, and the `--since` / `--until` over-fetch done 2026-11-14; open tracked sessions pending |
| — | Hindsight import review | 2.6 | needs a decision (`YREM_HOOK_EXCLUDE_SOURCES` not built) |
