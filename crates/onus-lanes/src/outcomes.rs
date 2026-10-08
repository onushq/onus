//! Outcome records: what happened to each change, per agent setup and per
//! judge configuration. They give a new setup its stricter lanes, let people
//! rotate judge configurations, and measure Phase 4's gate: the miss rate
//! of sampled human audits of auto-approved changes, and the share of
//! changes in the human lane.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
use onus_core::Lane;
use serde::{Deserialize, Serialize};

use crate::classify::Record;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// Seconds since the Unix epoch.
    pub at: u64,
    /// The change: `owner/repo#123` or a head commit.
    pub change: String,
    /// The agent setup (`tool/model/config`).
    pub agent: String,
    /// The judge configuration, if a judge decided.
    #[serde(default)]
    pub judge: Option<String>,
    pub lane: Lane,
    /// `approve`, `reject` or `escalate`, if a judge decided.
    #[serde(default)]
    pub verdict: Option<String>,
    /// `merged`, `reverted`, `incident`, `closed` or `open`.
    pub result: String,
    /// A person audited it after an automatic approval.
    #[serde(default)]
    pub audited: bool,
    /// The audit found a problem the automatic path missed.
    #[serde(default)]
    pub missed: bool,
    /// The commit the change landed as, to match reverts against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// For an incident: the components and symbols it involved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub involved: Vec<String>,
    /// For an incident or revert: what happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The latest outcome of each change, in the order changes first appeared:
/// a later `reverted` or `incident` record replaces an earlier `merged`.
pub fn latest(outcomes: &[Outcome]) -> Vec<&Outcome> {
    let mut order: Vec<&str> = Vec::new();
    let mut last: BTreeMap<&str, &Outcome> = BTreeMap::new();
    for o in outcomes {
        if !last.contains_key(o.change.as_str()) {
            order.push(&o.change);
        }
        last.insert(&o.change, o);
    }
    order
        .into_iter()
        .filter_map(|c| last.get(c).copied())
        .collect()
}

pub fn append(path: &Path, outcome: &Outcome) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    writeln!(f, "{}", serde_json::to_string(outcome)?)?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Vec<Outcome>> {
    let Ok(f) = std::fs::File::open(path) else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for (i, line) in std::io::BufReader::new(f).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(&line).with_context(|| {
                format!("line {} of {} is not an outcome", i + 1, path.display())
            })?,
        );
    }
    Ok(out)
}

/// How many of a setup's latest changes count toward its record, and how
/// recent incidents are measured.
pub const RECENT: usize = 20;

/// An agent setup's record: changes still merged (not reverted, no
/// incident), and incidents among its last [`RECENT`] changes.
pub fn record_for(outcomes: &[Outcome], agent: &str) -> Record {
    let mine: Vec<&Outcome> = latest(outcomes)
        .into_iter()
        .filter(|o| o.agent == agent)
        .collect();
    Record {
        changes: mine.iter().filter(|o| o.result == "merged").count() as u32,
        recent_incidents: mine
            .iter()
            .rev()
            .take(RECENT)
            .filter(|o| o.result == "incident")
            .count() as u32,
    }
}

/// A revert found in git history: the commit it reverts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Revert {
    pub revert: String,
    pub reverted: String,
    pub subject: String,
}

/// Reverts in `repo`'s history since `since` (a ref), from git's own
/// "This reverts commit <sha>." line.
pub fn find_reverts(repo: &Path, since: Option<&str>) -> Result<Vec<Revert>> {
    let mut args = vec!["log".to_string(), "--format=%H%x1f%s%x1f%b%x1e".to_string()];
    if let Some(s) = since {
        args.push(format!("{s}..HEAD"));
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(&args)
        .output()
        .context("cannot run git")?;
    if !out.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found = Vec::new();
    for rec in text.split('\u{1e}') {
        let parts: Vec<&str> = rec.trim_matches('\n').split('\u{1f}').collect();
        let [sha, subject, body] = parts.as_slice() else {
            continue;
        };
        if let Some(at) = body.find("This reverts commit ") {
            let rest = &body[at + "This reverts commit ".len()..];
            let reverted: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if reverted.len() >= 7 {
                found.push(Revert {
                    revert: sha.to_string(),
                    reverted,
                    subject: subject.to_string(),
                });
            }
        }
    }
    Ok(found)
}

/// Records each revert against the outcome whose commit it reverts; returns
/// the outcomes added. A revert of an unknown commit is left alone.
pub fn ingest_reverts(outcomes: &[Outcome], reverts: &[Revert], at: u64) -> Vec<Outcome> {
    let mut added = Vec::new();
    for r in reverts {
        let Some(o) = latest(outcomes).into_iter().find(|o| {
            o.commit
                .as_deref()
                .is_some_and(|c| c.starts_with(&r.reverted) || r.reverted.starts_with(c))
                && o.result == "merged"
        }) else {
            continue;
        };
        added.push(Outcome {
            at,
            result: "reverted".into(),
            note: Some(format!(
                "reverted by {} ({})",
                &r.revert[..12.min(r.revert.len())],
                r.subject
            )),
            ..o.clone()
        });
    }
    added
}

/// Where held-out tests should go next: the components and symbols
/// incidents involved, most often first.
pub fn backlog(outcomes: &[Outcome]) -> Vec<(String, u32)> {
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    for o in outcomes.iter().filter(|o| o.result == "incident") {
        for i in &o.involved {
            *counts.entry(i).or_default() += 1;
        }
    }
    let mut out: Vec<(String, u32)> = counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    pub changes: u32,
    pub merged: u32,
    pub reverted: u32,
    pub incidents: u32,
    pub auto_merged: u32,
    pub audited: u32,
    pub missed: u32,
}

impl Tally {
    fn add(&mut self, o: &Outcome) {
        self.changes += 1;
        match o.result.as_str() {
            "merged" => self.merged += 1,
            "reverted" => self.reverted += 1,
            "incident" => self.incidents += 1,
            _ => {}
        }
        if o.lane == Lane::AutoMerge && o.result == "merged" {
            self.auto_merged += 1;
        }
        if o.audited {
            self.audited += 1;
        }
        if o.missed {
            self.missed += 1;
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub total: Tally,
    pub by_agent: BTreeMap<String, Tally>,
    pub by_judge: BTreeMap<String, Tally>,
    pub by_lane: BTreeMap<String, u32>,
    /// The share of changes in the human lane: the review budget.
    pub human_share: Option<f64>,
    /// Misses found by human audits of automatic approvals, per audit.
    pub audit_miss_rate: Option<f64>,
}

pub fn summarize(outcomes: &[Outcome]) -> Summary {
    let mut s = Summary::default();
    for o in latest(outcomes) {
        s.total.add(o);
        s.by_agent.entry(o.agent.clone()).or_default().add(o);
        if let Some(j) = &o.judge {
            s.by_judge.entry(j.clone()).or_default().add(o);
        }
        *s.by_lane.entry(o.lane.as_str().to_string()).or_default() += 1;
    }
    let ratio = |a: u32, b: u32| (b > 0).then(|| f64::from(a) / f64::from(b));
    s.human_share = ratio(
        s.by_lane.get("human").copied().unwrap_or(0),
        s.total.changes,
    );
    s.audit_miss_rate = ratio(s.total.missed, s.total.audited);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(agent: &str, lane: Lane, result: &str, audited: bool, missed: bool) -> Outcome {
        Outcome {
            at: 1,
            change: "x".into(),
            agent: agent.into(),
            judge: Some("j1".into()),
            lane,
            verdict: Some("approve".into()),
            result: result.into(),
            audited,
            missed,
            commit: None,
            involved: vec![],
            note: None,
        }
    }

    #[test]
    fn reverts_and_incidents_change_the_record() {
        let mut merged = o("a/b/c", Lane::AutoMerge, "merged", false, false);
        merged.change = "acme/shop#1".into();
        merged.commit = Some("abc1234def".into());
        let mut other = merged.clone();
        other.change = "acme/shop#2".into();
        other.commit = Some("fff0000".into());
        let all = vec![merged.clone(), other];
        assert_eq!(record_for(&all, "a/b/c").changes, 2);
        let reverts = [Revert {
            revert: "999".repeat(10),
            reverted: "abc1234def5678".into(),
            subject: "Revert \"x\"".into(),
        }];
        let added = ingest_reverts(&all, &reverts, 5);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].result, "reverted");
        let mut all = all;
        all.extend(added);
        assert_eq!(record_for(&all, "a/b/c").changes, 1);
        let mut incident = merged.clone();
        incident.change = "acme/shop#2".into();
        incident.result = "incident".into();
        incident.involved = vec!["billing".into(), "billing:src/a.ts#charge".into()];
        all.push(incident);
        let r = record_for(&all, "a/b/c");
        assert_eq!((r.changes, r.recent_incidents), (0, 1));
        assert_eq!(backlog(&all)[0], ("billing".to_string(), 1));
    }

    #[test]
    fn records_and_summaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outcomes.jsonl");
        for (i, mut x) in [
            o("claude/sonnet/a", Lane::AutoMerge, "merged", true, false),
            o("claude/sonnet/a", Lane::AutoMerge, "merged", true, true),
            o("claude/sonnet/a", Lane::Human, "merged", false, false),
            o("codex/gpt/b", Lane::Judge, "reverted", false, false),
        ]
        .into_iter()
        .enumerate()
        {
            x.change = format!("acme/shop#{i}");
            append(&path, &x).unwrap();
        }
        let all = load(&path).unwrap();
        assert_eq!(record_for(&all, "claude/sonnet/a").changes, 3);
        assert_eq!(record_for(&all, "codex/gpt/b").changes, 0);
        let s = summarize(&all);
        assert_eq!(s.total.changes, 4);
        assert_eq!(s.by_agent["codex/gpt/b"].reverted, 1);
        assert_eq!(s.human_share, Some(0.25));
        assert_eq!(s.audit_miss_rate, Some(0.5));
    }
}
