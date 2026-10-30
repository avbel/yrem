# yrem — TODO

Remote-only client for `yantrikdb-server`. One binary, two faces:

- `yrem mcp` — stdio MCP server for Claude Code / Codex (rmcp)
- `yrem <command>` — the same operations as a JSON-emitting CLI, for hooks, skills and scripts (clap)

No engine, no embedder, no Python. The server embeds text itself (`resolve_embedding` in `http_gateway.rs`).

Public repo: `github.com/avbel/yrem`. License: MIT.

## Principles

- One Rust struct per operation is at the same time:
  - the clap arguments
  - the serde input
  - the MCP JSON Schema (schemars)

  CLI and MCP cannot drift.
- Latest stable Rust (1.99.1), edition 2024, every dependency at its latest release. Dependabot bumps weekly; the toolchain is bumped on each Rust release.
- Use modern language and std features (see [Rust conventions](#rust-conventions)).
- Agent-facing text is a token budget, enforced by a test:
  - MCP instructions ≤ 150 tokens (upstream: ~950)
  - full `tools/list` ≤ 2,000 tokens

  Doc comments on args structs are agent text: terse, no examples, no repetition of what the schema already says (types, enums, ranges). Human-only detail goes in `#[arg(long_help = …)]` / `#[command(long_about = …)]`, which schemars ignores.
- Every line of `src/` is exercised by tests. CI enforces the coverage floor (see [M6](#m6--tests-and-coverage)).
- No dev-dependency crates beyond extra features of crates already in the tree.
- Nothing from the real memory store is ever committed: fixtures come from a scratch database.

## Assumptions

- Single-node `yantrikdb-server` v0.20.x on the NAS, plain HTTP on LAN, one bearer token per database.
- Clients: Claude Code + Codex on the laptop (macOS, aarch64).
- Server handlers take untyped `Json<Value>` and there is no OpenAPI spec. The contract is our own types plus contract tests against a real server.

## Decisions

- [x] License: MIT. Single `LICENSE` file. MCP instructions text adapted from yantrikdb-mcp (MIT) is credited in `NOTICE`.
- [x] Stack:
  - rmcp + tokio
  - reqwest
  - clap + schemars
  - thiserror + anyhow
  - toml
  - chrono (already in the tree via rmcp → schemars, so it replaces jiff)
- [x] `correct`: native-first, emulated on 501. Try `POST /v1/correct`; when the server answers 501, emulate it as get → remember → forget (see M2). The revision chain is lost while emulating; README documents this.
- [x] MCP tools are flat: one tool per CLI leaf command (`graph relate` ↔ `graph_relate`). No `action`-tagged enums: they generate a top-level `oneOf`, which several clients and model APIs reject or handle poorly, and it costs more tokens.
- [x] MCP tool set v0.1 (12; `graph_edges` demoted, see below):
  - `remember`, `recall`, `forget`, `correct`, `think`
  - `graph_relate`, `graph_edges`
  - `conflict_list`, `conflict_resolve`
  - `session_digest`, `session_capture`
  - `memory_get`, `memory_current`

  CLI-only: `conflict dismiss`, `session start|end`, `memory list|gaps|stats`, `call`, `doctor`, `mcp`. If `tools/list` exceeds 2k tokens, demote tools to CLI-only.
  - `graph_edges` is CLI-only: `yantrikdb-server` 0.20 has no HTTP edge query (`POST /v1/graph` → 404, verified against a real server), so as an MCP tool it would always fail. `yrem graph edges` reports `Unsupported` (exit 4); move it back to MCP when the server serves edges.
  - Measured: `tools/list` ≈ 1.6k tokens, instructions ≈ 145 tokens (golden test `tests/golden/tools_list.json`).
- [x] Config env names: `YREM_URL` / `YREM_TOKEN`, with fallback to `YANTRIKDB_SERVER_URL` / `YANTRIKDB_TOKEN` for drop-in replacement.
- [x] Logging: rmcp already emits `tracing` events (tracing is in the tree). Add `tracing-subscriber` (fmt to stderr, `RUST_LOG` filter) to surface protocol logs, or keep `eprintln!` for our own diagnostics only. Recommendation: add it; debugging MCP handshakes without it is blind.

## Concurrency model

- `#[tokio::main(flavor = "current_thread")]`. One thread is enough: all work is I/O-bound and a handful of requests at a time.
- **MCP:**
  - rmcp spawns a task per request.
  - Each tool body runs under `tokio::select!` against `RequestContext::ct.cancelled()`. A `notifications/cancelled` drops the reqwest future, which aborts the in-flight HTTP request — a real cancellation.
  - Concurrent tool calls from the client run concurrently.
- **CLI:** the same runtime makes one request and exits.
- Shared state: one `reqwest::Client`, which pools connections internally, held in `Arc<App>`.

## Rust conventions

Toolchain 1.99.1, edition 2024. Use:

- let chains (1.88) and `if let` guards on match arms (1.95):
  - status → error mapping, e.g. `501 if let Some(op) = body["operation"].as_str() => …`
  - config resolution
- async closures (1.85) for the cancellation helper: `cancellable(&ctx.ct, async || app.recall(args).await)`.
- `cfg_select!` (1.95) for the `tls` / plain-HTTP client construction.
- `std::env::home_dir` (undeprecated in 1.87) for the config path; no `dirs` crate.
- `Vec::extract_if` (1.87) for client-side recall filters. It returns the removed rows, so `filtered_by_*` counts come for free.
- `str::floor_char_boundary` (1.91) to truncate the hook digest at the 10k-char cap without splitting a UTF-8 sequence.
- `String::from_utf8_lossy_owned` (1.99) for `token_cmd` stdout.
- `std::io::IsTerminal`: pretty JSON when stdout is a TTY, compact when piped. `--pretty` / `--compact` override.
- `std::process::ExitCode` returned from `main`. The exit code comes from `anyhow::Error::downcast_ref::<yrem::Error>()`; no `process::exit`.
- Errors:
  - `thiserror` for the library error enum: typed, matchable, carries the exit code
  - `anyhow` with `.context(…)` only at the binary edge (config loading, `doctor`, `main`)
- `#[expect(lint, reason = "…")]` instead of `#[allow]` (1.81). An unused expectation is itself a warning.
- `[lints]` table in `Cargo.toml` (shown in M0).
- Cargo `build.warnings = "deny"` in CI (1.97) instead of `RUSTFLAGS=-Dwarnings`, which invalidates the build cache.

## M0 — Scaffold

- [x] `rust-toolchain.toml`:
  ```toml
  [toolchain]
  channel = "1.99.1"
  components = ["rustfmt", "clippy", "llvm-tools-preview"]
  ```
- [x] `Cargo.toml`:
  ```toml
  [package]
  name = "yrem"
  version = "0.1.0"
  edition = "2024"
  rust-version = "1.99"
  license = "MIT"
  description = "Remote-only CLI and MCP server for yantrikdb-server"
  repository = "https://github.com/avbel/yrem"
  readme = "README.md"
  keywords = ["mcp", "yantrikdb", "agent-memory", "cli"]
  categories = ["command-line-utilities"]

  [features]
  default = []
  tls = ["reqwest/rustls"]   # rustls + aws-lc-rs; plain HTTP is enough on LAN

  [dependencies]
  rmcp = { version = "3.8.0", features = ["transport-io"] }   # defaults: server, macros, base64
  tokio = { version = "1.54.1", features = ["rt", "macros"] }
  reqwest = { version = "0.13.5", default-features = false, features = ["json"] }
  clap = { version = "4.6.7", features = ["derive", "env"] }
  schemars = "1.2.3"
  serde = { version = "1.0.229", features = ["derive"] }
  serde_json = "1.0.154"
  thiserror = "2.0.21"
  anyhow = "1.0.104"
  toml = "1.1.8"
  chrono = { version = "0.4.45", default-features = false, features = ["std", "clock", "serde"] }

  [dev-dependencies]
  rmcp = { version = "3.8.0", features = ["client", "transport-child-process"] }
  tokio = { version = "1.54.1", features = ["rt", "macros", "net", "io-util", "time", "process"] }

  [lints.rust]
  unsafe_code = "forbid"
  missing_debug_implementations = "warn"
  unreachable_pub = "warn"

  [lints.clippy]
  pedantic = { level = "warn", priority = -1 }
  unwrap_used = "deny"
  expect_used = "warn"
  print_stdout = "warn"   # stdout belongs to JSON-RPC / CLI output; write through `output` only

  [profile.release]
  lto = true
  codegen-units = 1
  panic = "abort"
  strip = true
  opt-level = "s"
  ```
  - `schemars` 1.x unifies with rmcp's `schemars ^1.0`, so there is one version in the tree.
  - `chrono` unifies with the copy schemars already pulls (`chrono04`).
- [x] Modules: `client` (HTTP), `ops` (argument structs + operations), `mcp` (rmcp handler), `cli` (clap tree), `config`, `time`, `error`, `output`. `src/main.rs` stays a thin call into `yrem::run() -> ExitCode`.
- [x] Repo files:
  - [x] `LICENSE` (MIT)
  - [x] `NOTICE`: credits yantrikdb-mcp (MIT) for the adapted MCP instructions
  - [x] `README.md`
  - [x] `SECURITY.md`
  - [x] `.gitignore` including `config.toml` and `*.token`
- [ ] GitHub settings: secret scanning + push protection, Dependabot alerts, branch protection on `main` (CI required).
- [x] CI (ubuntu + macos), each in both the default (`tls`) and `--no-default-features` (plain HTTP) variants:
  - [x] `fmt --check`
  - [x] `clippy --all-targets`
  - [x] `test`
  - [x] coverage gate (M6)
  - [x] `cargo-deny` 0.20.2: advisories; licenses allow-list compatible with MIT distribution; duplicate versions as warnings
- [x] Dependabot for `cargo` and `github-actions`, weekly. Bump `rust-toolchain.toml` by hand on each Rust release (next: 1.100.0).
- [x] Commit `Cargo.lock`.

## M1 — HTTP client (`src/client`)

- [ ] Create a scratch database + token on the NAS server.
  - Done in the meantime against a locally built `yantrikdb-server` 0.20.0 (scratch DB, bundled embedder): 71 fixtures in `tests/fixtures/`, API notes in `tests/fixtures/README.md`. Re-capture on the NAS to pin its exact version. Capture request/response fixtures for every v0.1 endpoint into `tests/fixtures/*.json`. Never capture from the real store. This repo is public.
- [x] Config resolution order: CLI flags > env > `~/.config/yrem/config.toml` (`$XDG_CONFIG_HOME` honored).
  - [x] `#[derive(Deserialize)] #[serde(deny_unknown_fields)]` so a typo in the file is an error, not a silent default.
- [x] Optional `token_cmd` config key (e.g. `op read op://...`, `security find-generic-password -w ...`). Run it once with `std::process::Command` during config load, before any async work; its stdout is the token.
- [x] `reqwest::Client`:
  - [x] `connect_timeout(3s)`, `timeout(15s)`
  - [x] `user_agent("yrem/<version>")`
  - [x] default `Authorization: Bearer` header with `HeaderValue::set_sensitive(true)`, so it never shows up in `Debug` output
- [x] Error enum (`thiserror`), mapped from HTTP status:
  - [x] 401/403 → `Auth`
  - [x] 404 → `NotFound`
  - [x] 429 → `Quota`
  - [x] 501 with `error = "not_yet_available_over_http"` → `Unsupported { operation, reason }`
  - [x] 503 → `Unavailable` (read-only node / no leader)
  - [x] anything else → `Http { status, body }`
  - [x] `reqwest::Error` → `Transport` (`#[from]`); timeouts become `Unavailable`
  - [x] invalid JSON → `Decode`
  - [x] `Error::exit_code()` and `Error::is_retryable()`
- [x] Typed request structs. Lenient response structs: `#[serde(default)]` on fields plus `#[serde(flatten)] extra: Map<String, Value>`, so server additions don't break decoding.
- [x] Endpoints v0.1:
  - [x] `GET /v1/health`, `GET /v1/health/deep`, `GET /v1/stats`
  - [x] `POST /v1/remember`, `POST /v1/remember/batch`, `POST /v1/recall`, `POST /v1/forget`, `POST /v1/correct`, `POST /v1/think`
  - [x] `POST /v1/relate`, `POST /v1/graph` (`action: query`)
  - [x] `GET /v1/conflicts`, `POST /v1/conflicts/{id}/resolve`
  - [x] `GET /v1/session/digest`, `POST /v1/session/end` (summary capture), `POST /v1/sessions`, `DELETE /v1/sessions/{id}`
  - [x] `GET /v1/memory/{rid}`, `GET /v1/memories`, `GET /v1/current?namespace=`
  - [x] `GET /v1/insights/gaps`
- [ ] Endpoints v0.2:
  - [ ] `GET /v1/temporal/stale`, `GET /v1/temporal/upcoming`, `POST /v1/temporal/as_of`
  - [ ] `GET /v1/triggers`, `GET /v1/triggers/history`, `POST /v1/triggers/{id}/{action}`
  - [ ] `POST /v1/claim`, `GET /v1/claims`
  - [ ] `POST /v1/procedures/surface`, `GET /v1/tasks`
- [x] Unavailable server-side (501), do not wrap: task add/update/delete, procedure learn/reinforce, category learn/reset.

## M2 — Ops layer (`src/ops`)

- [x] One argument struct per leaf operation:
  ```rust
  #[derive(Debug, clap::Args, serde::Deserialize, schemars::JsonSchema)]
  #[serde(deny_unknown_fields)]
  ```
  - Doc comments are both the `--help` text and the schema `description`.
  - Use `#[schemars(range(min = 0.0, max = 1.0))]` where the model benefits from bounds.
  - Each derive ignores the other derives' attributes, so `#[arg(…)]`, `#[serde(…)]` and `#[schemars(…)]` coexist on one field.
- [x] CLI-only fields (e.g. `--stdin`) carry `#[serde(skip)] #[schemars(skip)]`. MCP-only fields (e.g. `remember.memories` for batch) carry `#[arg(skip)]` and are reachable from the CLI via `yrem call`.
- [x] One `async fn` per operation on `App`, taking the args struct and returning `Result<serde_json::Value, yrem::Error>`. MCP and CLI both call these; neither contains logic.
- [x] Port validation from yantrikdb-mcp `tools.py`:
  - [x] clamp `importance` / `certainty` to 0..1 and `valence` to -1..1
  - [x] `memory_type` as a `ValueEnum` + `JsonSchema` enum: semantic | episodic | procedural
  - [x] non-empty `text`
- [x] `src/time` (chrono):
  - [x] accepted inputs: RFC 3339 with offset, `YYYY-MM-DD`, unix seconds, relative spans (`7d`, `12h`, `30m`)
  - [x] the relative-span parser is ~20 lines, table-tested
  - [x] `since` / `until` become `time_window`
- [x] `recall` behaviour:
  - [x] post-filter results by `time_window` (the server may ignore it)
  - [x] `min_score_ratio`
  - [x] `order` (`ValueEnum`): relevance | recency | certainty | first_mention
  - [x] `refine_exclude` filtered client-side, all via `extract_if`
- [x] `remember`: single (`text`) or batch (`memories`, MCP / `yrem call`). Refuse `created_at`, `idempotency_key` and `claims` with an explicit error until the server accepts them; never drop them silently.
  - Server 0.20 accepts `created_at` (unix secs) and `idempotency_key` on `/v1/remember`, so both are passed through. `claims` is not a field: `deny_unknown_fields` rejects it.
- [x] `session_capture`: summary text → `/v1/session/end`; the server splits it into atomic facts.
- [x] `correct` (args: `rid`, `reason` required, `new_text?`, `new_importance?`, `new_valence?`, `metadata_merge?`):
  1. `POST /v1/correct`. On 2xx, return the native result with `"emulated": false`.
  2. On `Unsupported` (501), `GET /v1/memory/{rid}`. On 404, return a `NotFound` error; nothing has been written.
  3. Build the replacement record:
     - `memory_type`, `domain`, `source`, `namespace`, `certainty`, `emotional_state` come from the old row.
     - `importance` / `valence` come from the overrides, else the old values.
     - `text` is `new_text`, else the old text.
     - `metadata` is the old metadata ∪ `metadata_merge` ∪ `{"corrects": old_rid, "correct_reason": reason, "corrected_at": <RFC 3339>}`.
  4. `POST /v1/remember` first. If it fails, return the error and leave the old record untouched: a duplicate is recoverable, a loss is not.
  5. `POST /v1/forget` the old rid. If that fails, return `Error::PartialCorrect { old_rid, new_rid }`, which tells the agent to retry `forget(old_rid)`.
  6. Success result: `{"old_rid", "new_rid", "emulated": true}`.
  - [x] Note in the tool description that the new memory's `created_at` is now, not the original date.
- [x] Output shaping: ISO UTC timestamp plus unix, scores rounded to 4 dp, nulls pruned, compact JSON.

## M3 — MCP (`yrem mcp`, rmcp)

- [x] `struct Server { app: Arc<App> }` with `#[tool_router]`; one `#[tool]` per MCP tool taking `Parameters<Args>`.
- [x] Annotations on every tool:
  - `read_only_hint` on recall / get / list / digest
  - `destructive_hint` on forget / correct / conflict_resolve
  - `idempotent_hint` where true
  - `open_world_hint = false` everywhere
- [x] `#[tool_handler]` + `ServerHandler::get_info`: server name/version, tools capability, `instructions`.
- [x] Serve: `Server::new(app).serve(rmcp::transport::stdio()).await?.waiting().await?`. rmcp negotiates both spec eras: the 2025-11-25 `initialize` flow and 2026-07-28 stateless `server/discover`.
- [x] Tool results:
  - [x] success → `CallToolResult::success(vec![ContentBlock::text(compact_json)])`. No `structuredContent`; it would duplicate the tokens.
  - [x] `yrem::Error` → `CallToolResult::error(…)` with actionable text (operation name for 501, both rids for `PartialCorrect`)
  - [x] `ErrorData` only for protocol-level failures
- [x] Cancellation: every tool body runs through `cancellable(&ctx.ct, async || …)`.
- [x] Tool descriptions: one line each, ≤ 15 words, saying when to use the tool. Canonical text:

  | Tool | Description |
  |---|---|
  | `remember` | Store a durable fact. Specific, self-contained text. |
  | `recall` | Semantic search over memories. Returns ranked hits with rids. |
  | `forget` | Delete a memory by rid. |
  | `correct` | Replace a stored fact that changed. New record links the old rid. |
  | `think` | Consolidate memories and detect conflicts. |
  | `graph_relate` | Link two entities with a typed relation. |
  | `graph_edges` | List an entity's relations. |
  | `conflict_list` | List open conflicts between stored facts. |
  | `conflict_resolve` | Resolve a conflict. |
  | `session_digest` | Session-start briefing: open decisions, conflicts, stale facts. |
  | `session_capture` | Split a session summary into separate stored facts. |
  | `memory_get` | Fetch one memory by rid. |
  | `memory_current` | Latest value of a fact that changes over time. |

- [x] Arg doc comments: ≤ 8 words, only where the name isn't self-explanatory (e.g. `importance`: "Higher decays slower."). No description on `rid`, `query`, `text`, `namespace`, `top_k`.
- [x] Schema budget:
  - [x] golden-file test of `tools/list` + instructions, asserting instructions ≤ 150 tokens and `tools/list` ≤ 2,000 tokens (chars / 4)
  - [x] use `#[tool(input_schema = compact_schema::<T>())]` from the start: generate with schemars, strip `$schema`, `title` and integer `format`s; keep `description`, `enum`, `minimum` / `maximum`, `required`
- [x] Instructions: `src/mcp/instructions.md` (~145 tokens), loaded with `include_str!`. It condenses the upstream golden path (~950 tokens) to 8 lines: digest → recall, remember, correct, memory_current, think, what not to store, and the trust boundary.
- [ ] Run the official MCP conformance suite against `yrem mcp` in CI.

## M4 — CLI (`yrem <command>`, clap)

- [x] clap tree:
  - plain: `remember`, `recall`, `forget`, `correct`, `think`
  - grouped: `graph {relate,edges}`, `conflict {list,resolve,dismiss}`, `session {digest,capture,start,end}`, `memory {get,current,list,gaps,stats}`
  - other: `call`, `doctor`, `mcp`
  - alias: `digest` = `session digest`
- [x] Leaf commands embed the shared args struct via `#[command(flatten)]`. Global flags: `--url` (`env = "YREM_URL"`), `--token` (`env = "YREM_TOKEN"`, `hide_env_values = true`), `--config` (`env = "YREM_CONFIG"`), `--pretty`, `--compact`.
- [x] Positional fields: `remember` → `text`, `recall` → `query`, `forget` / `correct` / `memory get` → `rid`, `conflict resolve|dismiss` → `id`, `session capture` → `summary`, `session end` → `session_id`.
- [x] `--stdin` reads the positional text from stdin (`remember`, `recall`, `session capture`).
- [x] `yrem call <mcp_tool> [JSON]`: deserialize JSON (argument or stdin) into that tool's args struct and run it. Covers MCP-only fields (batch `memories`, `metadata_merge` objects).
- [x] Output: JSON on stdout. Pretty on a TTY, compact when piped; `--pretty` / `--compact` override.
- [x] Exit codes:
  - 0 ok
  - 1 other
  - 2 usage (clap's default)
  - 3 auth
  - 4 unsupported (501)
  - 5 unavailable/timeout
  - 6 partial correct
- [x] `yrem digest --hook`: print `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"..."}}`. Truncate the context to stay under Claude Code's 10k-char hook output cap. If the server is unreachable or auth fails, print nothing, write the reason to stderr and exit 0, so a session never fails to start because of memory.
- [x] `yrem doctor`: print resolved config (token redacted), connectivity, auth, server version from `/v1/health/deep`, and the list of operations the server answers 501 for.

## M5 — Integration

- [x] Claude Code, MCP route: `claude mcp add --scope user yrem -- yrem mcp`. Credentials come from `~/.config/yrem/config.toml`, so no token lands in `~/.claude.json`.
- [ ] Claude Code, CLI route: `~/.claude/skills/yrem/SKILL.md`, a `SessionStart` hook running `yrem digest --hook`, and `Bash(yrem:*)` in `permissions.allow`.
- [x] Codex: `[mcp_servers.yrem] command = "yrem"`, `args = ["mcp"]`. Prefer MCP here: shell commands in Codex's workspace-write sandbox have no network by default.
- [ ] Sandbox profiles (SBPL / bubblewrap) wrapping the agents must allow egress to `nas:7438` if the CLI route is used.
- [x] Remove yantrikdb-mcp from both clients. Run a side-by-side recall comparison on the real store before switching.
  - yantrikdb-mcp could not reach the LAN address either, so yrem was compared with raw `POST /v1/recall` instead: same rids and scores. The yantrikdb hook plugins (`yantrikdb-hooks`, `yantrikdb-chatgpt`) are disabled too.
- macOS Local Network privacy blocks LAN addresses (`192.168.x`) for non-Apple binaries started from a terminal whose permission is off: `EHOSTUNREACH`, while `/usr/bin/curl` works. The config uses the NAS Tailscale address (`100.64.23.221:7438`), which that privacy check does not cover.

## M6 — Tests and coverage

Rule: every module ships with its tests in the same PR. A PR that lowers coverage fails CI.

- [x] **Coverage gate:** cargo-llvm-cov 0.9.1, `cargo llvm-cov --all-features --fail-under-lines 97 --fail-under-regions 95`.
  - [x] lcov artifact + a per-file table in the job summary. No third-party coverage service.
  - [ ] Ratchet the floor up as coverage grows; never down. (Raised to 97 / 95; measured 98.6 % lines, 97.0 % regions.)
- [ ] **Mutation testing:** cargo-mutants 27.1.0, weekly scheduled job. Surviving mutants in `ops`, `error`, `mcp` and `time` get an issue each. Target: zero survivors in those modules.
- [x] **Test support:**
  - [x] `tests/support/fake_server.rs`: a `tokio::net::TcpListener` task with a scripted queue of (expected method, path, auth, body matcher) → (status, fixture, optional delay) pairs. It records every request so tests can assert which calls happened and which did not.
  - [x] Golden files with a `YREM_BLESS=1` regenerate switch.
- [x] **client:**
  - [x] status → error table test (401, 403, 404, 429, 501 with and without `operation`, 503, 500, connection refused, timeout, invalid JSON)
  - [x] lenient decode with unknown fields
  - [x] auth header sensitive and present
  - [x] user agent
- [x] **config:** precedence matrix (flag / env / fallback env / file / `token_cmd`); unknown keys rejected; missing config; `token_cmd` failure and non-UTF-8 output.
- [x] **time:** table-driven tests: RFC 3339 with offsets, date-only, unix, `7d` / `12h` / `30m`, invalid inputs, `since > until`.
- [x] **ops (every operation):**
  - [x] happy path against fixtures
  - [x] every validation error
  - [x] output shaping (rounding, ISO, null pruning)
- [x] **schema/args drift:** for every args struct, assert:
  - [x] the schemars schema's properties = the serde field names = the clap arg ids (minus the documented CLI-only / MCP-only fields)
  - [x] `clap::Command::debug_assert()` passes
- [x] **recall:** each filter (`time_window`, `min_score_ratio`, `order`, `refine_exclude`) with its `filtered_by_*` count.
- [x] **correct:** one test per path:
  - [x] native 2xx
  - [x] 501 → emulated success: assert the request order get → remember → forget, and the carried fields + metadata
  - [x] GET 404: assert no remember or forget was sent
  - [x] remember fails: assert no forget was sent
  - [x] forget fails: `PartialCorrect` with both rids, exit code 6
- [x] **mcp** (rmcp client over `TokioChildProcess` spawning `yrem mcp`):
  - [x] `tools/list` golden + token budgets (instructions ≤ 150, `tools/list` ≤ 2,000)
  - [x] each tool's success and `isError` paths
  - [x] cancellation aborts the in-flight HTTP request (fake server delay + assert the connection was dropped)
  - [x] concurrent calls
  - [x] both protocol eras (`initialize` flow and `server/discover` flow)
  - [x] stdout carries only JSON-RPC frames
- [x] **cli:**
  - [x] every command against the fake server via `std::process::Command` on `env!("CARGO_BIN_EXE_yrem")`
  - [x] `--stdin`, `yrem call`
  - [x] TTY vs pipe output (via `script` on unix)
  - [x] every exit code
  - [x] `--help` snapshot
  - [x] `digest --hook` truncation at a multi-byte boundary
  - [x] `digest --hook` with the server down: empty stdout, exit 0
- [ ] **Contract CI job:** docker compose with `yantrikdb-server` pinned to the NAS version; bootstrap a scratch database and token; run the suite with `YREM_CONTRACT=1`. This job also records whether `/v1/correct` is still 501.
- [ ] **Nightly contract job** against the latest server release to catch API drift. On failure it opens an issue.

## M7 — Release

- [x] `.github/workflows/release.yml`: on `v*` tags (must match `Cargo.toml`), builds Linux x86_64 / aarch64 musl, macOS aarch64, Windows x86_64 / aarch64; `.tar.gz` (`.zip` on Windows) + `.sha256`, published as a GitHub release.
- [x] Pre-commit hook `.githooks/pre-commit` (`cargo fmt --check` + clippy `-D warnings`); enable with `git config core.hooksPath .githooks`.

- [x] GitHub Releases + Homebrew tap (`avbel/homebrew-tap`, `Formula/yrem.rb` regenerated by `release.yml`) without cargo-dist. A shell installer is not done.
- [ ] Targets: `aarch64-apple-darwin`, `x86_64-unknown-linux-musl` (NAS), `aarch64-unknown-linux-musl`. `tls` is now a default feature (aws-lc-rs, C); `--no-default-features` has no C dependencies. Release builds are static musl with TLS; Windows x64 uses aws-lc prebuilt NASM objects, Windows arm64 needs clang-cl.
- [ ] Publish to crates.io.

## Later

- [ ] Streamable HTTP mode (rmcp `transport-streamable-http-server`, stateless) behind a cargo feature, so the server can be hosted on the NAS and the laptop install dropped entirely.
- [ ] v0.2 tools: `temporal_stale`, `temporal_upcoming`, `temporal_as_of`, triggers.
- [ ] Leader discovery via `/v1/health` → `cluster.accepts_writes`, only if the server ever becomes a cluster.
- [ ] Switch `correct` to native once the server ships RFC 032; the native-first path already handles it. `remember(claims=…)` via `/v1/claim` after the write.
- [ ] Upstream PR to yantrikdb-mcp `HttpBackend`: it stubs endpoints the server already serves (get, list, temporal, triggers, conversation, tasks read, procedures surface, claims).
