//! `yrem <command>`: the clap tree. Leaf commands embed the shared args structs.

pub mod doctor;
pub mod hook;

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

/// Remote-only CLI and MCP server for yantrikdb-server. Prints JSON.
#[derive(Debug, Parser)]
#[command(name = "yrem", version, about, propagate_version = true)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args)]
pub struct Global {
    /// Server URL.
    #[arg(long, global = true, env = "YREM_URL")]
    pub url: Option<String>,
    /// Database token.
    #[arg(long, global = true, env = "YREM_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    /// Config file.
    #[arg(long, global = true, env = "YREM_CONFIG")]
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
    Remember(RememberArgs),
    /// Semantic search over memories.
    Recall(RecallArgs),
    /// Delete a memory by rid.
    Forget(ForgetArgs),
    /// Replace a stored fact that changed.
    #[command(
        long_about = "Replace a stored fact that changed.\n\nTries POST /v1/correct; when \
        the server answers 501 it is emulated as get -> remember -> forget. The new record \
        links the old rid in its metadata and its created_at is now. Exit code 6 means the \
        new record was written but the old one was not forgotten: retry `yrem forget <old_rid>`."
    )]
    Correct(CorrectArgs),
    /// Consolidate memories and detect conflicts.
    Think(ThinkArgs),
    /// Entity graph.
    #[command(subcommand)]
    Graph(GraphCommand),
    /// Conflicts between stored facts.
    #[command(subcommand)]
    Conflict(ConflictCommand),
    /// Session briefing, capture and tracking.
    #[command(subcommand)]
    Session(SessionCommand),
    /// Inspect stored memories.
    #[command(subcommand)]
    Memory(MemoryCommand),
    /// Stale, upcoming and point-in-time memories.
    #[command(subcommand)]
    Temporal(TemporalCommand),
    /// Server-raised triggers.
    #[command(subcommand)]
    Trigger(TriggerCommand),
    /// Temporal claims about entities.
    #[command(subcommand)]
    Claim(ClaimCommand),
    /// Stored procedures.
    #[command(subcommand)]
    Procedure(ProcedureCommand),
    /// Tasks.
    #[command(subcommand)]
    Task(TaskCommand),
    /// Session-start briefing (alias of `session digest`).
    Digest(DigestCli),
    /// Run an MCP tool with raw JSON arguments.
    Call(CallArgs),
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
    /// Resolve with strategy `dismiss`.
    Dismiss(ConflictDismissArgs),
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// Session-start briefing.
    Digest(DigestCli),
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
pub struct DigestCli {
    #[command(flatten)]
    pub args: SessionDigestArgs,
    /// Print Claude Code `SessionStart` hook JSON; never fail.
    #[arg(long)]
    pub hook: bool,
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
    match execute(cli, url_from_env, token_from_env).await {
        Ok(code) => code,
        Err(e) => report(&e),
    }
}

/// Prints an error (JSON on stdout for library errors, text on stderr) and picks the exit code.
fn report(e: &anyhow::Error) -> ExitCode {
    eprintln!("yrem: {e:#}");
    match e.downcast_ref::<Error>() {
        Some(err) => {
            let mut body = json!({ "error": err.to_string(), "exit_code": err.exit_code() });
            if let Error::Sensitive { findings, .. } = err {
                body["findings"] = json!(findings);
            }
            output::print_raw(&body.to_string());
            ExitCode::from(err)
        }
        None => ExitCode::FAILURE,
    }
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
    if let Command::Digest(d) | Command::Session(SessionCommand::Digest(d)) = &cli.command
        && d.hook
    {
        hook::run(&overrides, d.args.clone()).await;
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
        Command::Session(SessionCommand::Digest(d)) | Command::Digest(d) => {
            app.session_digest(d.args).await?
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
        Command::Doctor => unreachable!("handled above"),
    };
    output::print(&value, format);
    Ok(ExitCode::SUCCESS)
}
