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

/// An agent setup's record: changes merged without being reverted.
pub fn record_for(outcomes: &[Outcome], agent: &str) -> Record {
    Record {
        changes: outcomes
            .iter()
            .filter(|o| o.agent == agent && o.result == "merged")
            .count() as u32,
    }
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
    for o in outcomes {
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
        }
    }

    #[test]
    fn records_and_summaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outcomes.jsonl");
        for x in [
            o("claude/sonnet/a", Lane::AutoMerge, "merged", true, false),
            o("claude/sonnet/a", Lane::AutoMerge, "merged", true, true),
            o("claude/sonnet/a", Lane::Human, "merged", false, false),
            o("codex/gpt/b", Lane::Judge, "reverted", false, false),
        ] {
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
