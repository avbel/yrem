//! Whether what a merged memory was made from is fit to show.
//!
//! `think` merges memories into a new row that carries `consolidated_from` but not the
//! sources' `sensitive` flag, so a merge of a sensitive memory would be injected like any other
//! row. Before a merge is shown, its sources are read, however deep. A source that cannot be
//! read in time counts as sensitive: the check fails closed.
//!
//! The merge also takes `source=user` whatever it was made from, so a merge of captured prompts
//! passes for a curated fact: 41 of 54 harness-text memories in a real store were such merges.
//! A source that was auto-captured makes the merge unfit too, unless captures may be shown.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;
use tokio::time::{Instant, timeout_at};

use super::gate;
use crate::client::Client;

/// More memories than this behind one row, and it is not worth the requests.
const MAX_LINEAGE: usize = 30;

/// A stored memory as far as the check needs it: unfit to show or not, and what it was merged
/// from.
type Node = (bool, Vec<String>);

/// Reads memories until `deadline` and remembers what it read.
#[derive(Debug)]
pub struct Vetter {
    client: Client,
    deadline: Instant,
    nodes: HashMap<String, Option<Node>>,
    captures_unfit: bool,
}

impl Vetter {
    #[must_use]
    pub fn new(client: Client, deadline: Instant, captures_unfit: bool) -> Self {
        Self {
            client,
            deadline,
            nodes: HashMap::new(),
            captures_unfit,
        }
    }

    /// The memory `rid`, or `None` when it cannot be read before the deadline.
    async fn node(&mut self, rid: &str) -> Option<Node> {
        if let Some(known) = self.nodes.get(rid) {
            return known.clone();
        }
        let read = timeout_at(self.deadline, self.client.memory_get(rid))
            .await
            .ok()
            .and_then(Result::ok)
            .map(|m| {
                let meta = gate::parse_metadata(&m.metadata);
                let captured = gate::is_captured(m.source.as_deref(), &meta);
                let unfit = gate::is_sensitive(&meta) || (self.captures_unfit && captured);
                (unfit, gate::sources_in(&meta))
            });
        self.nodes.insert(rid.to_owned(), read.clone());
        read
    }

    /// Whether none of `sources`, or of what they were merged from, is sensitive or (when
    /// captures are unfit) auto-captured.
    pub async fn clean(&mut self, sources: &[String]) -> bool {
        let mut queue: VecDeque<String> = sources.iter().cloned().collect();
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(rid) = queue.pop_front() {
            if !seen.insert(rid.clone()) {
                continue;
            }
            if seen.len() > MAX_LINEAGE {
                return false;
            }
            match self.node(&rid).await {
                Some((false, deeper)) => queue.extend(deeper),
                _ => return false,
            }
        }
        true
    }

    /// Whether the memory `rid` is fit to show and so are its sources.
    pub async fn clean_rid(&mut self, rid: &str) -> bool {
        match self.node(rid).await {
            Some((false, sources)) => self.clean(&sources).await,
            _ => false,
        }
    }
}

/// A row's own metadata says what it was merged from; a row with no sources needs no lookup.
#[must_use]
pub fn sources_of(row: &Value) -> Vec<String> {
    gate::merge_sources(row)
}
