//! The session briefing: the server's digest, narrative and decisions vetted before they are
//! shown.

use std::time::Duration;

use serde_json::Value;
use tokio::time::{Instant, timeout_at};

use super::lineage::{self, Vetter};
use super::state::Kind;
use super::{Hook, Maintenance, decisions, render};
use crate::error::Error;
use crate::ops::{App, MemoryListArgs, SessionDigestArgs};

/// Everything the briefing waits for, requests and vetting together. Harnesses give a
/// `SessionStart` hook 10 to 15 seconds.
const BUDGET: Duration = Duration::from_secs(8);

impl Hook<'_> {
    pub(super) async fn briefing(&self, app: &App) -> anyhow::Result<Option<String>> {
        let s = &self.settings;
        if !s.digest
            || self.headless
            || !self
                .store
                .claim_digest(self.session(), self.input.source.as_deref())
        {
            return Ok(None);
        }
        match self.compose(app).await {
            Ok(text) => {
                self.store.finish_digest(self.session());
                Ok(text)
            }
            Err(e) => {
                self.store.release_digest(self.session());
                Err(e)
            }
        }
    }

    /// The digest and the listings are fetched together. Only the digest is required: a
    /// listing that fails or runs out of time leaves out the decisions it would have fed.
    async fn compose(&self, app: &App) -> anyhow::Result<Option<String>> {
        let deadline = Instant::now() + BUDGET;
        let always = self.settings.maintenance == Maintenance::Always;
        let digest = timeout_at(
            deadline,
            app.session_digest(SessionDigestArgs {
                namespace: self.project.clone(),
                include_gaps: always,
            }),
        );
        let in_project = async {
            match &self.project {
                Some(p) => {
                    self.list(app, Some(p.clone()), decisions::FETCH_PROJECT, deadline)
                        .await
                }
                None => Vec::new(),
            }
        };
        let (digest, recent, in_project) = tokio::join!(
            digest,
            self.list(app, None, decisions::FETCH_RECENT, deadline),
            in_project
        );
        let digest = digest.map_err(|_| Error::Unavailable("the digest timed out".into()))??;

        let mut vetter = Vetter::new(app.client.clone(), deadline);
        let listed: Vec<Value> = recent.iter().chain(&in_project).cloned().collect();
        let narrative = self.vetted_narrative(&mut vetter, &digest, &listed).await;
        let mut excluded: Vec<String> = narrative.iter().map(|(rid, _)| rid.clone()).collect();
        let picked = loop {
            let picked = decisions::select(
                &recent,
                &in_project,
                &self.gate,
                self.settings.decisions,
                super::now_secs(),
                &excluded,
            );
            let mut unfit = false;
            for d in &picked {
                if !excluded.contains(&d.rid) && !vetter.clean(&d.sources).await {
                    excluded.push(d.rid.clone());
                    unfit = true;
                }
            }
            if !unfit {
                break picked;
            }
        };

        let conflicts = digest
            .get("open_conflict_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let maintenance = match self.settings.maintenance {
            Maintenance::Off => false,
            Maintenance::Always => true,
            // Taken last, with the text about to be printed: a briefing that fails earlier
            // leaves the day's slot for the next one.
            Maintenance::Daily => conflicts > 0 && self.store.claim_maintenance(),
        };
        let text = render::digest_text(
            narrative.as_ref().map(|(_, t)| t.as_str()),
            &digest,
            &picked,
            maintenance,
            always,
        );
        let shown = picked.iter().map(|d| d.rid.as_str());
        for rid in shown.chain(narrative.iter().map(|(rid, _)| rid.as_str())) {
            let _ = self.store.claim(self.session(), Kind::Seen, rid);
        }
        Ok(text)
    }

    /// Newest rows first, or nothing when the listing is off, fails or is late.
    async fn list(
        &self,
        app: &App,
        namespace: Option<String>,
        limit: u32,
        deadline: Instant,
    ) -> Vec<Value> {
        if self.settings.decisions == 0 {
            return Vec::new();
        }
        let listing = app.memory_list(MemoryListArgs {
            namespace,
            limit: Some(limit),
            desc: true,
            ..MemoryListArgs::default()
        });
        match timeout_at(deadline, listing).await {
            Ok(Ok(answer)) => decisions::records(&answer),
            Ok(Err(e)) => {
                eprintln!("yrem hook: decisions: {e}");
                Vec::new()
            }
            Err(_) => {
                eprintln!("yrem hook: decisions: timed out");
                Vec::new()
            }
        }
    }

    /// The digest's narrative head as `(rid, text)`, when it names a memory that is fit to show:
    /// found among the listed rows, else read on its own. Without a rid it is dropped.
    async fn vetted_narrative(
        &self,
        vetter: &mut Vetter,
        digest: &Value,
        listed: &[Value],
    ) -> Option<(String, String)> {
        let head = digest.get("narrative_head")?;
        let text = render::narrative_text(head)?;
        let rid = head
            .get("rid")
            .and_then(Value::as_str)
            .filter(|r| !r.is_empty())?;
        if self.gate.eligible(head).is_some() {
            return None;
        }
        let row = listed
            .iter()
            .find(|r| r.get("rid").and_then(Value::as_str) == Some(rid));
        let fit = match row {
            Some(row) => {
                self.gate.eligible(row).is_none() && vetter.clean(&lineage::sources_of(row)).await
            }
            None => vetter.clean_rid(rid).await,
        };
        fit.then(|| (rid.to_owned(), text.to_owned()))
    }
}
