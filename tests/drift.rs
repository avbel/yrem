//! Schema / serde / clap drift: one args struct must mean the same thing on every face.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use clap::{Args, CommandFactory};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use yrem::ops::*;

/// Fields only on the CLI (`#[serde(skip)]`) and only in MCP (`#[arg(skip)]`).
const CLI_ONLY: &[&str] = &["stdin"];
const MCP_ONLY: &[&str] = &["metadata", "memories", "metadata_merge"];

fn check<T: Args + JsonSchema + DeserializeOwned + std::fmt::Debug + 'static>(name: &'static str) {
    let schema = Value::Object(yrem::mcp::compact_schema::<T>().as_ref().clone());
    let props: BTreeSet<String> = schema["properties"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();

    let cmd = T::augment_args(clap::Command::new(name));
    let clap_ids: BTreeSet<String> = cmd
        .get_arguments()
        .map(|a| a.get_id().to_string())
        .filter(|id| !CLI_ONLY.contains(&id.as_str()))
        .collect();
    let schema_minus_mcp: BTreeSet<String> = props
        .iter()
        .filter(|p| !MCP_ONLY.contains(&p.as_str()))
        .cloned()
        .collect();
    assert_eq!(
        clap_ids, schema_minus_mcp,
        "{name}: clap ids vs schema properties"
    );

    for p in &props {
        let err = serde_json::from_value::<T>(json!({ p: null }))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            !err.contains("unknown field"),
            "{name}: serde does not know `{p}`: {err}"
        );
    }
    for c in CLI_ONLY {
        if cmd.get_arguments().any(|a| a.get_id() == *c) {
            let err = serde_json::from_value::<T>(json!({ *c: true })).unwrap_err();
            assert!(
                err.to_string().contains("unknown field"),
                "{name}: `{c}` leaks into serde"
            );
            assert!(!props.contains(*c), "{name}: `{c}` leaks into the schema");
        }
    }
}

#[test]
fn args_structs_agree() {
    check::<RememberArgs>("remember");
    check::<RecallArgs>("recall");
    check::<ForgetArgs>("forget");
    check::<CorrectArgs>("correct");
    check::<ThinkArgs>("think");
    check::<GraphRelateArgs>("graph_relate");
    check::<GraphEdgesArgs>("graph_edges");
    check::<ConflictListArgs>("conflict_list");
    check::<ConflictResolveArgs>("conflict_resolve");
    check::<ConflictDismissArgs>("conflict_dismiss");
    check::<SessionDigestArgs>("session_digest");
    check::<SessionCaptureArgs>("session_capture");
    check::<SessionStartArgs>("session_start");
    check::<SessionEndArgs>("session_end");
    check::<MemoryGetArgs>("memory_get");
    check::<MemoryCurrentArgs>("memory_current");
    check::<MemoryListArgs>("memory_list");
    check::<MemoryGapsArgs>("memory_gaps");
    check::<MemoryStatsArgs>("memory_stats");
}

#[test]
fn clap_tree_is_valid() {
    yrem::cli::Cli::command().debug_assert();
}

#[test]
fn every_mcp_tool_has_a_cli_leaf() {
    let cmd = yrem::cli::Cli::command();
    for tool in yrem::ops::TOOLS {
        let path: Vec<&str> = match tool.split_once('_') {
            Some((group, leaf))
                if cmd
                    .find_subcommand(group)
                    .is_some_and(clap::Command::has_subcommands) =>
            {
                vec![group, leaf]
            }
            _ => vec![tool],
        };
        let mut c = &cmd;
        for p in &path {
            c = c
                .find_subcommand(p)
                .unwrap_or_else(|| panic!("{tool}: no `yrem {}`", path.join(" ")));
        }
    }
}
