//! `yrem <command>`: the clap tree. Leaf commands embed the shared args structs.

pub mod doctor;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::parser::ValueSource;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use serde_json::{Value, json};

use crate::client::Client;
use crate::config::{Config, Overrides};
use crate::error::Error;
use crate::hook::{Event, Harness};
use crate::mcp::Server;
use crate::ops::{
    App, ClaimAddArgs, ClaimListArgs, ConflictDismissArgs, ConflictListArgs, ConflictResolveArgs,
    CorrectArgs, ForgetArgs, GraphEdgesArgs, GraphRelateArgs, MemoryCurrentArgs, MemoryGapsArgs,
    MemoryGetArgs, MemoryListArgs, MemoryStatsArgs, ProcedureSurfaceArgs, RecallArgs, RememberArgs,
    SessionCaptureArgs, SessionDigestArgs, SessionEndArgs, SessionStartArgs, TaskListArgs,
    TemporalAsOfArgs, TemporalListArgs, ThinkArgs, TriggerActionArgs, TriggerHistoryArgs,
    TriggerListArgs,
};
use crate::output::{self, Format};

const LONG_ABOUT: &str = "\
Remote-only CLI and MCP server for yantrikdb-server. Prints JSON.

OUTPUT
  Every command prints one JSON document on stdout: pretty when stdout is a terminal, \
compact when piped (override with --pretty / --compact). Diagnostics and errors go to \
stderr; a failed command also prints {\"error\", \"exit_code\"} on stdout.

CONFIG
  Highest priority first: flags (--url, --token, --config), environment (YREM_URL, \
YREM_TOKEN, YREM_CONFIG; YANTRIKDB_SERVER_URL and YANTRIKDB_TOKEN as fallbacks), then \
$XDG_CONFIG_HOME/yrem/config.toml (default ~/.config/yrem/config.toml) with keys url, token \
and token_cmd. token_cmd is a shell command whose stdout is the token, used when no token \
is set elsewhere. One token grants access to one database; a namespace is only a row-level \
tag inside it.

TIME ARGUMENTS
  --since, --until, --at, --valid-from, --valid-to and --created-at accept RFC 3339 with an \
offset (2026-10-01T09:00:00+02:00), a date (2026-10-01, midnight UTC), unix seconds, or a \
span back from now: a number plus s, m, h, d or w (7d, 12h, 30m).

EXIT CODES
  0  success
  1  other error (including not found)
  2  usage error
  3  authentication failed (HTTP 401/403)
  4  operation not supported by this server (HTTP 501)
  5  server unavailable or timed out
  6  correct wrote the new memory but could not forget the old one; retry `yrem forget <old_rid>`
  7  refused: the data looks like a secret. Redact it and retry, or pass --force if it is \
not a secret";

const TOP_EXAMPLES: &str = "\
Examples:
  yrem remember \"The deploy pipeline uses GitHub Actions\" --domain work
  yrem recall \"how do we deploy\" --since 30d --top-k 5
  echo \"notes...\" | yrem session capture --stdin
  yrem doctor";

/// Remote-only CLI and MCP server for yantrikdb-server. Prints JSON.
#[derive(Debug, Parser)]
#[command(
    name = "yrem",
    version,
    about,
    propagate_version = true,
    long_about = LONG_ABOUT,
    after_long_help = TOP_EXAMPLES,
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args)]
pub struct Global {
    /// Server URL.
    #[arg(
        long,
        long_help = "Server base URL, e.g. http://192.168.1.10:7438.",
        global = true,
        env = "YREM_URL"
    )]
    pub url: Option<String>,
    /// Database token.
    #[arg(
        long,
        long_help = "Bearer token for one database (ydb_...). Prefer the environment or token_cmd over the flag: arguments show up in process listings.",
        global = true,
        env = "YREM_TOKEN",
        hide_env_values = true
    )]
    pub token: Option<String>,
    /// Config file.
    #[arg(
        long,
        long_help = "TOML file with url, token and token_cmd; replaces the default location. /dev/null means no file.",
        global = true,
        env = "YREM_CONFIG"
    )]
    pub config: Option<PathBuf>,
    /// Pretty-print JSON.
    #[arg(long, global = true, conflicts_with = "compact")]
    pub pretty: bool,
    /// Compact JSON.
    #[arg(long, global = true)]
    pub compact: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Store a durable fact.
    #[command(after_long_help = "\
Examples:
  yrem remember \"Alice owns the billing service\" --domain people --importance 0.8
  echo \"multi-line fact\" | yrem remember --stdin

Several memories at once: yrem call remember '{\"memories\":[{\"text\":\"...\"},{\"text\":\"...\"}]}'
A refused secret exits 7 and names the findings; redact and retry, or add --force.
The answer carries `warnings` for a text over 1,500 characters (split it into facts) and for \
a single memory that is nearly a stored one (cosine 0.92 or more: use `yrem correct`). The \
check never blocks the write; it is skipped with --sensitive, --force or --idempotency-key.")]
    Remember(RememberArgs),
    /// Semantic search over memories.
    #[command(after_long_help = "\
Filters --namespace, --domain and --memory-type are applied by the server; --since, \
--until, --min-score-ratio and --refine-exclude are applied after the response, and the \
output reports how many hits each removed as filtered_by_*. With --since or --until \
the server is asked for three times --top-k (at most 100) and the answer is cut back to \
--top-k after filtering.

Examples:
  yrem recall \"billing owner\" --namespace work --top-k 5
  yrem recall \"deploys\" --since 7d --order recency")]
    Recall(RecallArgs),
    /// Delete a memory by rid.
    #[command(
        long_about = "Delete a memory by rid.\n\nThe server tombstones it: the memory disappears from \
        recall and memory list, but its text stays in the server's store, index and replication \
        log. Forgetting is not a way to remove a leaked secret from the server. A server merge \
        also keeps the memories it was merged from; --cascade forgets those too.\n\n\
Examples:\n  yrem forget 01a18d82-690f-7a90-b75b-2331ceea45eb --reason \"duplicate\"\n  yrem forget \
01a18d82-690f-7a90-b75b-2331ceea45eb --cascade"
    )]
    Forget(ForgetArgs),
    /// Replace a stored fact that changed.
    #[command(
        long_about = "Replace a stored fact that changed.\n\nTries POST /v1/correct; when \
        the server answers 501 it is emulated as get -> remember -> forget. The new record \
        links the old rid in its metadata and keeps the old created_at. Exit code 6 means the \
        new record was written but the old one was not forgotten: retry `yrem forget <old_rid>`. \
        The old text stays on the server (see `yrem forget --help`).\n\n\
Examples:\n  yrem correct 01a18d82-690f-7a90-b75b-2331ceea45eb --reason \"migrated\" \
--new-text \"The service stores data in MySQL\"\n  yrem call correct '{\"rid\":\"...\",\"reason\":\"owner changed\",\
\"metadata_merge\":{\"team\":\"billing\"}}'\n\n\
metadata_merge is only available through `yrem call` and MCP; it is merged into the old \
record's metadata."
    )]
    Correct(CorrectArgs),
    /// Consolidate memories and detect conflicts.
    Think(ThinkArgs),
    /// Entity graph.
    #[command(
        subcommand,
        after_long_help = "Example:\n  yrem graph relate --src billing-svc --dst postgres --rel-type uses"
    )]
    Graph(GraphCommand),
    /// Conflicts between stored facts.
    #[command(subcommand)]
    Conflict(ConflictCommand),
    /// Session briefing, capture and tracking.
    #[command(subcommand)]
    Session(SessionCommand),
    /// Inspect stored memories.
    #[command(
        subcommand,
        after_long_help = "\
Example:
  yrem memory list --namespace work --limit 20 --desc
  yrem memory current --namespace work"
    )]
    Memory(MemoryCommand),
    /// Stale, upcoming and point-in-time memories.
    #[command(
        subcommand,
        after_long_help = "\
Examples:
  yrem temporal stale --days 30
  yrem temporal as-of \"who owns billing\" --at 2026-01-01
  yrem temporal as-of \"deploy process\" --at 90d"
    )]
    Temporal(TemporalCommand),
    /// Server-raised triggers.
    #[command(
        subcommand,
        after_long_help = "\
A trigger moves pending -> delivered -> acknowledged -> acted, one step at a time. \
`dismiss` works only while it is pending. A transition the current state does not permit, \
or an unknown id, exits 1 (not found).

Example:
  yrem trigger list
  yrem trigger deliver <id> && yrem trigger acknowledge <id> && yrem trigger act <id>"
    )]
    Trigger(TriggerCommand),
    /// Temporal claims about entities.
    #[command(
        subcommand,
        after_long_help = "\
Example:
  yrem claim add alice owns billing-svc --valid-from 2026-01-01 --confidence-band high
  yrem claim list alice"
    )]
    Claim(ClaimCommand),
    /// Stored procedures.
    #[command(subcommand)]
    Procedure(ProcedureCommand),
    /// Tasks.
    #[command(subcommand)]
    Task(TaskCommand),
    /// Session-start briefing (alias of `session digest`).
    #[command(after_long_help = "\
For agent hooks use `yrem hook session-start`.

Example:
  yrem digest --namespace work --include-gaps")]
    Digest(SessionDigestArgs),
    /// Run an MCP tool with raw JSON arguments.
    #[command(after_long_help = "\
Tool names are the MCP tools (remember, recall, correct, graph_relate, ...). Fields not \
exposed as flags work here: remember's `memories` batch and `metadata`, correct's \
`metadata_merge`.

Examples:
  yrem call remember '{\"memories\":[{\"text\":\"A\"},{\"text\":\"B\",\"namespace\":\"work\"}]}'
  echo '{\"rid\":\"...\",\"reason\":\"x\",\"metadata_merge\":{\"k\":\"v\"}}' | yrem call correct")]
    Call(CallArgs),
    /// Agent lifecycle hook for the harness plugins in plugins/.
    #[command(after_long_help = "\
Reads the harness's event JSON on stdin ({\"session_id\", \"cwd\", \"prompt\", \"source\", \
\"transcript_path\", \"turns\"}) and prints context to inject: hook JSON for --harness claude \
or codex, plain text for --harness plain. It never fails: errors go to stderr, the exit code \
is 0. Settings: YREM_HOOK_* environment variables (see README, Agent hooks).

Examples:
  echo '{\"session_id\":\"s1\",\"prompt\":\"how do we deploy the billing service\"}' \\
    | yrem hook prompt --harness plain
  yrem hook session-end --harness codex --detach < event.json")]
    Hook(HookCli),
    /// Check config, connectivity, auth and server capabilities.
    Doctor,
    /// Run the MCP server on stdio.
    Mcp,
}

#[derive(Debug, Subcommand)]
pub enum GraphCommand {
    /// Link two entities with a typed relation.
    Relate(GraphRelateArgs),
    /// List an entity's relations.
    Edges(GraphEdgesArgs),
}

#[derive(Debug, Subcommand)]
pub enum ConflictCommand {
    /// List conflicts.
    List(ConflictListArgs),
    /// Resolve a conflict.
    Resolve(ConflictResolveArgs),
    /// Close a conflict as noise; the note is marked "dismissed".
    Dismiss(ConflictDismissArgs),
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// Session-start briefing.
    Digest(SessionDigestArgs),
    /// Split a session summary into separate stored facts.
    Capture(SessionCaptureArgs),
    /// Start a tracked session.
    Start(SessionStartArgs),
    /// End a tracked session.
    End(SessionEndArgs),
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// Fetch one memory by rid.
    Get(MemoryGetArgs),
    /// Latest value of a fact that changes over time.
    Current(MemoryCurrentArgs),
    /// List memories.
    List(MemoryListArgs),
    /// Recall topics with weak results.
    Gaps(MemoryGapsArgs),
    /// Store statistics.
    Stats(MemoryStatsArgs),
}

#[derive(Debug, Subcommand)]
pub enum TemporalCommand {
    /// Memories not accessed within the window.
    Stale(TemporalListArgs),
    /// Memories due within the window.
    Upcoming(TemporalListArgs),
    /// Recall as the store was at a past time.
    AsOf(TemporalAsOfArgs),
}

#[derive(Debug, Subcommand)]
pub enum TriggerCommand {
    /// List pending triggers.
    List(TriggerListArgs),
    /// List triggers of every status.
    History(TriggerHistoryArgs),
    /// Mark a trigger delivered.
    Deliver(TriggerActionArgs),
    /// Mark a delivered trigger acknowledged.
    Acknowledge(TriggerActionArgs),
    /// Mark an acknowledged trigger acted on.
    Act(TriggerActionArgs),
    /// Dismiss a pending trigger.
    Dismiss(TriggerActionArgs),
}

#[derive(Debug, Subcommand)]
pub enum ClaimCommand {
    /// Assert a claim between two entities.
    Add(ClaimAddArgs),
    /// List claims about an entity.
    List(ClaimListArgs),
}

#[derive(Debug, Subcommand)]
pub enum ProcedureCommand {
    /// Procedures relevant to a query.
    Surface(ProcedureSurfaceArgs),
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// List tasks.
    List(TaskListArgs),
}

#[derive(Debug, Args)]
pub struct HookCli {
    /// Lifecycle event.
    #[arg(value_enum)]
    pub event: Event,
    /// Output format of the calling harness.
    #[arg(long, value_enum, default_value = "plain")]
    pub harness: Harness,
    /// Finish in a background process and return at once.
    #[arg(
        long,
        long_help = "Read stdin, hand the work to a background yrem and return at once, for hosts that clamp hook time (Codex SessionEnd: 3 s)."
    )]
    pub detach: bool,
}

#[derive(Debug, Args)]
pub struct CallArgs {
    /// MCP tool name, e.g. remember.
    pub tool: String,
    /// JSON arguments; read from stdin when omitted.
    pub json: Option<String>,
}

/// Parses the command line and runs it.
pub async fn run() -> ExitCode {
    let matches = Cli::command().get_matches();
    let from_env = |id: &str| matches.value_source(id) == Some(ValueSource::EnvVariable);
    let (url_from_env, token_from_env) = (from_env("url"), from_env("token"));
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };
    let stdout_is_protocol = matches!(cli.command, Command::Mcp);
    match execute(cli, url_from_env, token_from_env).await {
        Ok(code) => code,
        Err(e) => report(&e, stdout_is_protocol),
    }
}

/// Prints an error (JSON on stdout, text on stderr) and picks the exit code.
///
/// `yrem mcp` owns stdout for JSON-RPC, so its failures go to stderr only.
fn report(e: &anyhow::Error, stdout_is_protocol: bool) -> ExitCode {
    eprintln!("yrem: {e:#}");
    let (mut body, code) = match e.downcast_ref::<Error>() {
        Some(err) => (
            json!({ "error": err.to_string(), "exit_code": err.exit_code() }),
            ExitCode::from(err),
        ),
        None => (
            json!({ "error": format!("{e:#}"), "exit_code": 1 }),
            ExitCode::FAILURE,
        ),
    };
    if let Some(Error::Sensitive { findings, .. }) = e.downcast_ref::<Error>() {
        body["findings"] = json!(findings);
    }
    if !stdout_is_protocol {
        output::print_raw(&body.to_string());
    }
    code
}

async fn execute(cli: Cli, url_from_env: bool, token_from_env: bool) -> anyhow::Result<ExitCode> {
    let format = Format::detect(cli.global.pretty, cli.global.compact);
    let overrides = Overrides {
        url: cli.global.url,
        token: cli.global.token,
        config: cli.global.config,
        url_from_env,
        token_from_env,
    };
    if let Command::Hook(h) = &cli.command {
        crate::hook::run(&overrides, h.event, h.harness, h.detach).await;
        return Ok(ExitCode::SUCCESS);
    }
    let config = Config::load(&overrides)?;
    if let Command::Doctor = cli.command {
        let report = doctor::run(&config).await;
        output::print(&report, format);
        return Ok(ExitCode::SUCCESS);
    }
    let app = App::new(Client::new(&config)?);
    let value = match cli.command {
        Command::Mcp => {
            Server::new(app).serve_stdio().await?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Remember(a) => app.remember(a).await?,
        Command::Recall(a) => app.recall(a).await?,
        Command::Forget(a) => app.forget(a).await?,
        Command::Correct(a) => app.correct(a).await?,
        Command::Think(a) => app.think(a).await?,
        Command::Graph(GraphCommand::Relate(a)) => app.graph_relate(a).await?,
        Command::Graph(GraphCommand::Edges(a)) => app.graph_edges(a).await?,
        Command::Conflict(ConflictCommand::List(a)) => app.conflict_list(a).await?,
        Command::Conflict(ConflictCommand::Resolve(a)) => app.conflict_resolve(a).await?,
        Command::Conflict(ConflictCommand::Dismiss(a)) => app.conflict_dismiss(a).await?,
        Command::Session(SessionCommand::Digest(a)) | Command::Digest(a) => {
            app.session_digest(a).await?
        }
        Command::Session(SessionCommand::Capture(a)) => app.session_capture(a).await?,
        Command::Session(SessionCommand::Start(a)) => app.session_start(a).await?,
        Command::Session(SessionCommand::End(a)) => app.session_end(a).await?,
        Command::Memory(MemoryCommand::Get(a)) => app.memory_get(a).await?,
        Command::Memory(MemoryCommand::Current(a)) => app.memory_current(a).await?,
        Command::Memory(MemoryCommand::List(a)) => app.memory_list(a).await?,
        Command::Memory(MemoryCommand::Gaps(a)) => app.memory_gaps(a).await?,
        Command::Memory(MemoryCommand::Stats(a)) => app.memory_stats(a).await?,
        Command::Temporal(TemporalCommand::Stale(a)) => app.temporal_stale(a).await?,
        Command::Temporal(TemporalCommand::Upcoming(a)) => app.temporal_upcoming(a).await?,
        Command::Temporal(TemporalCommand::AsOf(a)) => app.temporal_as_of(a).await?,
        Command::Trigger(TriggerCommand::List(a)) => app.trigger_list(a).await?,
        Command::Trigger(TriggerCommand::History(a)) => app.trigger_history(a).await?,
        Command::Trigger(TriggerCommand::Deliver(a)) => app.trigger_deliver(a).await?,
        Command::Trigger(TriggerCommand::Acknowledge(a)) => app.trigger_acknowledge(a).await?,
        Command::Trigger(TriggerCommand::Act(a)) => app.trigger_act(a).await?,
        Command::Trigger(TriggerCommand::Dismiss(a)) => app.trigger_dismiss(a).await?,
        Command::Claim(ClaimCommand::Add(a)) => app.claim_add(a).await?,
        Command::Claim(ClaimCommand::List(a)) => app.claim_list(a).await?,
        Command::Procedure(ProcedureCommand::Surface(a)) => app.procedure_surface(a).await?,
        Command::Task(TaskCommand::List(a)) => app.task_list(a).await?,
        Command::Call(c) => {
            let raw = if let Some(j) = c.json {
                j
            } else {
                let mut s = String::new();
                std::io::stdin()
                    .read_to_string(&mut s)
                    .context("reading stdin")?;
                s
            };
            let args: Value = if raw.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&raw)
                    .map_err(|e| Error::Invalid(format!("JSON arguments: {e}")))?
            };
            app.call(&c.tool, args).await?
        }
        Command::Doctor | Command::Hook(_) => unreachable!("handled above"),
    };
    output::print(&value, format);
    Ok(ExitCode::SUCCESS)
}
