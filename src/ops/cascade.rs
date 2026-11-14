//! `forget --cascade`: a server merge keeps its sources, hidden but readable, so forgetting
//! only the merge leaves their text behind.

use std::collections::{HashSet, VecDeque};

use serde_json::{Value, json};

use super::{App, view};
use crate::client::ForgetRequest;
use crate::error::{Error, Result};
use crate::output::shape;

/// More than this many sources is not a merge worth guessing about.
const MAX_SOURCES: usize = 50;

impl App {
    /// Forgets the merge, then every memory it was merged from. Forget is idempotent and
    /// tombstoned rows stay readable, so rerunning after a failure finishes the job.
    pub(super) async fn forget_cascade(&self, req: ForgetRequest) -> Result<Value> {
        let sources = self.merge_sources(&req.rid).await?;
        let mut out = shape(self.client.forget(&req).await?);
        self.forget_sources(&sources, req.reason).await?;
        if let Value::Object(o) = &mut out {
            o.insert("cascaded".into(), json!(sources));
        }
        Ok(out)
    }

    pub(super) async fn forget_sources(
        &self,
        sources: &[String],
        reason: Option<String>,
    ) -> Result<()> {
        for rid in sources {
            let source = ForgetRequest {
                rid: rid.clone(),
                namespace: None,
                reason: reason.clone(),
            };
            self.client.forget(&source).await?;
        }
        Ok(())
    }

    /// Every memory `root` was merged from, however deep, nearest first. A source the server
    /// no longer has is a leaf: it is listed, and there is nothing behind it to follow.
    pub(super) async fn merge_sources(&self, root: &str) -> Result<Vec<String>> {
        let mut seen = HashSet::from([root.to_owned()]);
        let mut queue = VecDeque::from([root.to_owned()]);
        let mut sources = Vec::new();
        while let Some(rid) = queue.pop_front() {
            let memory = match self.client.memory_get(&rid).await {
                Err(Error::NotFound(_)) if rid != root => continue,
                other => other?,
            };
            for source in consolidated_from(memory.metadata) {
                if !seen.insert(source.clone()) {
                    continue;
                }
                if sources.len() == MAX_SOURCES {
                    return Err(Error::Invalid(format!(
                        "`{root}` was merged from more than {MAX_SOURCES} memories; forget them one by one"
                    )));
                }
                sources.push(source.clone());
                queue.push_back(source);
            }
        }
        Ok(sources)
    }
}

fn consolidated_from(metadata: Value) -> Vec<String> {
    view::metadata_object(metadata)
        .remove("consolidated_from")
        .and_then(|v| match v {
            Value::Array(items) => Some(items),
            _ => None,
        })
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
