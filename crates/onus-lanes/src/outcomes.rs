//! Outcome records: what happened to each change, per agent setup and per
//! judge configuration. They give a new setup its stricter lanes, let people
//! rotate judge configurations, and measure Phase 4's gate: the miss rate
//! of sampled human audits of auto-approved changes, and the share of
//! changes in the human lane.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
use onus_core::{Lane, ids};
use serde::{Deserialize, Serialize};

use crate::classify::{AreaHistories, AreaHistory, Record};
use crate::learned::Features;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// `merged`, `reverted`, `rolled-back`, `incident`, `closed` or `open`.
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
    /// The components the change touched (from its report): what per-area
    /// history counts reverts and incidents against.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// Who approved it on review (`@login`). Empty in the human lane means
    /// it merged without a counted review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviewed_by: Vec<String>,
    /// Chosen when it merged for a person to audit (`lanes.auditAfterMerge`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sampled: bool,
    /// The latest audit by a person: who, when and what they found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<AuditMark>,
    /// Who merged it (`@login`), when the host said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_by: Option<String>,
    /// What the change looked like when it was classified: what the learned
    /// lane model trains on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Features>,
    /// A person with write access said Onus caught a problem in it
    /// (`/onus caught`): a problem for training, like an incident.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub caught: bool,
    /// The production signal that produced this outcome (`signals.jsonl`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    /// A rollback done on purpose (a flag turned off as planned), not
    /// because production moved: it counts against neither the agent setup
    /// nor the components.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub planned: bool,
}

/// A person's audit of a merged change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditMark {
    /// `@login`, or a name for people without one.
    pub by: String,
    /// Unix seconds.
    pub at: u64,
    /// The audit found a problem the automatic path missed.
    pub missed: bool,
    /// What the automatic path missed, or anything worth keeping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Approved without a person: auto-merge, or the judge approved.
pub fn approved_without_person(lane: Lane, verdict: Option<&str>) -> bool {
    lane == Lane::AutoMerge || (lane == Lane::Judge && verdict == Some("approve"))
}

/// The record of a person's audit of `change`, built from its latest
/// outcome. A change with no outcome yet cannot be audited.
pub fn audit_record(
    all: &[Outcome],
    change: &str,
    by: &str,
    missed: bool,
    note: Option<&str>,
    at: u64,
) -> Result<Outcome> {
    let Some(last) = all.iter().rev().find(|o| o.change == change) else {
        anyhow::bail!("{change} has no outcome yet; audit it after it merges");
    };
    Ok(Outcome {
        at,
        audited: true,
        missed,
        note: Some(format!(
            "audited by {by}{}",
            note.map(|n| format!(": {n}")).unwrap_or_default()
        )),
        audit: Some(AuditMark {
            by: by.to_string(),
            at,
            missed,
            note: note.map(str::to_string),
        }),
        signal: None,
        ..last.clone()
    })
}

/// What waits for a person to audit, and the audit log.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Audits {
    /// Sampled and not audited yet, in the order the changes were first
    /// recorded (oldest first).
    pub awaiting: Vec<Outcome>,
    /// Every change a person audited (sampled or not), most recent audit
    /// first (`audit.at`, falling back to `at` for audits recorded without
    /// a mark).
    pub audited: Vec<Outcome>,
}

/// Both lists from the latest outcome of each change, each cut to `limit`.
/// A sampled change that was later reverted or caused an incident still
/// waits: leaving out the changes that went badly would bias the sample.
pub fn audits(all: &[Outcome], limit: usize) -> Audits {
    let last = latest(all);
    let awaiting = last
        .iter()
        .filter(|o| o.sampled && !o.audited)
        .take(limit)
        .map(|o| (*o).clone())
        .collect();
    let mut audited: Vec<&Outcome> = last.into_iter().filter(|o| o.audited).collect();
    audited.sort_by_key(|o| std::cmp::Reverse(o.audit.as_ref().map_or(o.at, |a| a.at)));
    Audits {
        awaiting,
        audited: audited.into_iter().take(limit).cloned().collect(),
    }
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
/// incident; a planned rollback still counts as merged), and incidents and
/// rollbacks among its last [`RECENT`] changes.
pub fn record_for(outcomes: &[Outcome], agent: &str) -> Record {
    let had = had_incident(outcomes);
    let rolled = had_rollback(outcomes);
    let mine: Vec<&Outcome> = latest(outcomes)
        .into_iter()
        .filter(|o| o.agent == agent)
        .collect();
    let recent = || mine.iter().rev().take(RECENT);
    Record {
        changes: mine
            .iter()
            .filter(|o| o.result == "merged" || (o.result == "rolled-back" && o.planned))
            .count() as u32,
        recent_incidents: recent().filter(|o| had.contains(o.change.as_str())).count() as u32,
        recent_rollbacks: recent()
            .filter(|o| rolled.contains(o.change.as_str()))
            .count() as u32,
    }
}

/// Changes that caused an incident at any point: reverting one later does
/// not erase the incident from its agent setup's record.
pub fn had_incident(outcomes: &[Outcome]) -> std::collections::HashSet<&str> {
    outcomes
        .iter()
        .filter(|o| o.result == "incident")
        .map(|o| o.change.as_str())
        .collect()
}

/// Changes rolled back at any point, not on purpose: like an incident, a
/// later revert does not erase it.
pub fn had_rollback(outcomes: &[Outcome]) -> std::collections::HashSet<&str> {
    outcomes
        .iter()
        .filter(|o| o.result == "rolled-back" && !o.planned)
        .map(|o| o.change.as_str())
        .collect()
}

/// What the history input looks at: from when until when, which
/// repository's changes (`owner/repo` of `owner/repo#123`; None = all), and
/// the change being classified (left out of its own history).
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryScope<'a> {
    /// Unix seconds; earlier records do not count.
    pub since: u64,
    /// Unix seconds; later records do not count, so a backfill sees the
    /// history as it was then (None = no end).
    pub until: Option<u64>,
    pub repo: Option<&'a str>,
    pub exclude: Option<&'a str>,
}

/// The `owner/repo` of a change id `owner/repo#123`.
pub fn repo_of(change: &str) -> Option<&str> {
    change.split_once('#').map(|(repo, _)| repo)
}

/// Reverts (and rollbacks) and incidents per component among `areas`,
/// recorded from `scope.since` to `scope.until`. A change's areas are its
/// `components` plus the component of each `involved` id. Every record
/// counts, not only the latest of each change: a revert and an incident are
/// events at their own time. A change counts once per area and kind. A
/// planned rollback does not count. Records of a bare commit (no `#`) count
/// for every repository.
pub fn area_history(
    outcomes: &[Outcome],
    areas: &BTreeSet<String>,
    scope: &HistoryScope,
) -> AreaHistories {
    // Area → (reverted changes, changes with incidents, latest time).
    type Changes<'o> = (BTreeSet<&'o str>, BTreeSet<&'o str>, u64);
    let mut found: BTreeMap<&str, Changes> = BTreeMap::new();
    for o in outcomes {
        if o.at < scope.since
            || scope.until.is_some_and(|until| o.at > until)
            || scope.exclude == Some(o.change.as_str())
        {
            continue;
        }
        if let (Some(want), Some(repo)) = (scope.repo, repo_of(&o.change))
            && want != repo
        {
            continue;
        }
        let revert = match o.result.as_str() {
            "rolled-back" if o.planned => continue,
            "reverted" | "rolled-back" => true,
            "incident" => false,
            _ => continue,
        };
        let touched = o
            .components
            .iter()
            .map(String::as_str)
            .chain(o.involved.iter().map(|i| ids::component_of(i).unwrap_or(i)))
            .filter(|c| areas.contains(*c));
        for c in touched {
            let (reverts, incidents, last) = found.entry(c).or_default();
            if revert {
                reverts.insert(&o.change);
            } else {
                incidents.insert(&o.change);
            }
            *last = (*last).max(o.at);
        }
    }
    found
        .into_iter()
        .map(|(c, (reverts, incidents, last))| {
            let changes: BTreeSet<&str> = reverts.union(&incidents).copied().collect();
            (
                c.to_string(),
                AreaHistory {
                    reverts: reverts.len() as u32,
                    incidents: incidents.len() as u32,
                    changes: changes.into_iter().map(str::to_string).collect(),
                    error_spikes: 0,
                    last_at: Some(last),
                },
            )
        })
        .collect()
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
                // Landed and not undone yet: an incident is often what
                // gets a change reverted.
                && matches!(o.result.as_str(), "merged" | "incident")
        }) else {
            continue;
        };
        added.push(Outcome {
            at,
            result: "reverted".into(),
            signal: None,
            planned: false,
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
    /// Sampled for a person to audit when they merged.
    #[serde(default)]
    pub sampled: u32,
    /// Rolled back in production (also counted in `reverted`).
    #[serde(default)]
    pub rolled_back: u32,
}

impl Tally {
    fn add(&mut self, o: &Outcome, incident: bool) {
        self.changes += 1;
        match o.result.as_str() {
            "merged" => self.merged += 1,
            // A flag turned off or a rollout rolled back undoes a change too.
            "reverted" => self.reverted += 1,
            "rolled-back" => {
                self.reverted += 1;
                self.rolled_back += 1;
            }
            _ => {}
        }
        if incident {
            self.incidents += 1;
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
        if o.sampled {
            self.sampled += 1;
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
    /// Sampled changes not audited yet.
    #[serde(default)]
    pub awaiting_audit: u32,
    /// Misses among sampled changes a person audited: the unbiased estimate
    /// of how many risky changes slip through without a person.
    #[serde(default)]
    pub sampled_miss_rate: Option<f64>,
}

pub fn summarize(outcomes: &[Outcome]) -> Summary {
    let mut s = Summary::default();
    let had = had_incident(outcomes);
    let (mut sampled_audits, mut sampled_misses) = (0, 0);
    for o in latest(outcomes) {
        if o.sampled && !o.audited {
            s.awaiting_audit += 1;
        }
        if o.sampled && o.audited {
            sampled_audits += 1;
            sampled_misses += u32::from(o.missed);
        }
        let incident = had.contains(o.change.as_str());
        s.total.add(o, incident);
        s.by_agent
            .entry(o.agent.clone())
            .or_default()
            .add(o, incident);
        // A judge configuration's record is the changes it decided: a verdict was recorded.
        if let (Some(j), Some(_)) = (&o.judge, &o.verdict) {
            s.by_judge.entry(j.clone()).or_default().add(o, incident);
        }
        *s.by_lane.entry(o.lane.as_str().to_string()).or_default() += 1;
    }
    let ratio = |a: u32, b: u32| (b > 0).then(|| f64::from(a) / f64::from(b));
    s.human_share = ratio(
        s.by_lane.get("human").copied().unwrap_or(0),
        s.total.changes,
    );
    s.audit_miss_rate = ratio(s.total.missed, s.total.audited);
    s.sampled_miss_rate = ratio(sampled_misses, sampled_audits);
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
            components: vec![],
            reviewed_by: vec![],
            sampled: false,
            audit: None,
            merged_by: None,
            features: None,
            caught: false,
            signal: None,
            planned: false,
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

    #[test]
    fn by_judge_counts_only_judged_changes() {
        let judged = o("a/b/c", Lane::Judge, "merged", false, false);
        // Written before ci pr recorded a judge only when one decided.
        let mut old = judged.clone();
        old.change = "acme/shop#2".into();
        old.verdict = None;
        let mut unjudged = judged.clone();
        unjudged.change = "acme/shop#3".into();
        unjudged.judge = None;
        unjudged.verdict = None;
        let s = summarize(&[judged, old, unjudged]);
        assert_eq!(s.by_judge.len(), 1);
        assert_eq!(s.by_judge["j1"].changes, 1);
        assert_eq!(s.total.changes, 3);
    }

    #[test]
    fn area_history_counts_events_in_the_window_and_the_repository() {
        let event = |change: &str, result: &str, at: u64, components: &[&str]| Outcome {
            at,
            change: change.into(),
            result: result.into(),
            components: components.iter().map(|c| c.to_string()).collect(),
            ..o("a/b/c", Lane::Judge, "merged", false, false)
        };
        let all = vec![
            event("acme/shop#1", "merged", 100, &["logger"]),
            event("acme/shop#1", "incident", 100, &["logger"]),
            event("acme/shop#1", "reverted", 110, &["logger"]),
            // Recorded twice: one change.
            event("acme/shop#1", "reverted", 120, &["logger"]),
            // Before the window.
            event("acme/shop#3", "incident", 10, &["logger"]),
            // Another repository.
            event("acme/web#2", "incident", 100, &["logger"]),
            // The change being classified.
            event("acme/shop#9", "incident", 100, &["logger"]),
            // A bare commit counts everywhere.
            event("abc1234", "rolled-back", 100, &["billing"]),
            Outcome {
                involved: vec!["billing:src/a.ts#charge".into()],
                ..event("acme/shop#4", "incident", 100, &[])
            },
            event("acme/shop#5", "incident", 100, &["orders"]),
        ];
        let areas: BTreeSet<String> = ["logger", "billing"].map(String::from).into();
        let h = area_history(
            &all,
            &areas,
            &HistoryScope {
                since: 50,
                until: None,
                repo: Some("acme/shop"),
                exclude: Some("acme/shop#9"),
            },
        );
        assert_eq!(h.len(), 2, "{h:?}");
        assert_eq!(
            h["logger"],
            AreaHistory {
                reverts: 1,
                incidents: 1,
                changes: vec!["acme/shop#1".into()],
                error_spikes: 0,
                last_at: Some(120),
            }
        );
        assert_eq!((h["billing"].reverts, h["billing"].incidents), (1, 1));
        assert_eq!(h["billing"].changes, ["abc1234", "acme/shop#4"]);
        // From another repository, only its own and bare commits count.
        let web = area_history(
            &all,
            &areas,
            &HistoryScope {
                since: 0,
                until: None,
                repo: Some("acme/web"),
                exclude: None,
            },
        );
        assert_eq!(web["logger"].changes, ["acme/web#2"]);
        // A backfill does not see what was recorded after its time.
        let then = area_history(
            &all,
            &areas,
            &HistoryScope {
                since: 50,
                until: Some(105),
                repo: Some("acme/shop"),
                exclude: Some("acme/shop#9"),
            },
        );
        assert_eq!((then["logger"].reverts, then["logger"].incidents), (0, 1));
    }

    #[test]
    fn components_survive_reverts_incidents_and_old_lines() {
        let mut merged = o("a/b/c", Lane::AutoMerge, "merged", false, false);
        merged.commit = Some("abc1234".into());
        merged.components = vec!["logger".into()];
        let reverts = [Revert {
            revert: "f".repeat(40),
            reverted: "abc1234".into(),
            subject: "Revert".into(),
        }];
        let added = ingest_reverts(std::slice::from_ref(&merged), &reverts, 5);
        assert_eq!(added[0].components, ["logger"]);
        // What `onus outcomes incident` writes: the last record, changed.
        let incident = Outcome {
            result: "incident".into(),
            involved: vec!["logger".into()],
            ..merged.clone()
        };
        assert_eq!(incident.components, ["logger"]);
        let line = serde_json::to_string(&merged).unwrap();
        assert!(line.contains(r#""components":["logger"]"#));
        let old = r#"{"at":1,"change":"x","agent":"a//","lane":"judge","result":"merged"}"#;
        let o: Outcome = serde_json::from_str(old).unwrap();
        assert!(o.components.is_empty());
        assert!(!serde_json::to_string(&o).unwrap().contains("components"));
    }

    #[test]
    fn approved_without_a_person() {
        for lane in [Lane::AutoMerge, Lane::Judge, Lane::Human, Lane::Blocked] {
            for verdict in [None, Some("approve"), Some("escalate")] {
                let want =
                    lane == Lane::AutoMerge || (lane == Lane::Judge && verdict == Some("approve"));
                assert_eq!(
                    approved_without_person(lane, verdict),
                    want,
                    "{lane:?} {verdict:?}"
                );
            }
        }
    }

    #[test]
    fn an_audit_is_built_from_the_latest_outcome() {
        let mut merged = o("a/b/c", Lane::AutoMerge, "merged", false, false);
        merged.change = "acme/shop#7".into();
        merged.sampled = true;
        let err = audit_record(&[], "acme/shop#7", "@alice", true, None, 9).unwrap_err();
        assert!(err.to_string().contains("has no outcome yet"), "{err}");
        let a = audit_record(
            std::slice::from_ref(&merged),
            "acme/shop#7",
            "@alice",
            true,
            Some("the opt-out was ignored"),
            9,
        )
        .unwrap();
        assert!(a.audited && a.missed && a.sampled);
        assert_eq!(a.at, 9);
        assert_eq!(
            a.note.as_deref(),
            Some("audited by @alice: the opt-out was ignored")
        );
        assert_eq!(
            a.audit,
            Some(AuditMark {
                by: "@alice".into(),
                at: 9,
                missed: true,
                note: Some("the opt-out was ignored".into()),
            })
        );
        let ok = audit_record(&[merged], "acme/shop#7", "@bob", false, None, 10).unwrap();
        assert_eq!(ok.note.as_deref(), Some("audited by @bob"));
        assert!(!ok.missed && ok.audit.unwrap().note.is_none());
    }

    #[test]
    fn the_audit_queue_and_log() {
        let at = |change: &str, t: u64, result: &str, sampled: bool| Outcome {
            at: t,
            change: change.into(),
            sampled,
            ..o("a/b/c", Lane::AutoMerge, result, false, false)
        };
        let mut all = vec![
            at("acme/shop#1", 1, "merged", true),
            at("acme/shop#2", 2, "merged", false),
            at("acme/shop#3", 3, "merged", true),
            at("acme/shop#4", 4, "merged", true),
            at("acme/shop#5", 5, "merged", true),
        ];
        // Later reverted or an incident: still waits.
        all.push(at("acme/shop#3", 6, "reverted", true));
        all.push(at("acme/shop#4", 7, "incident", true));
        all.push(audit_record(&all, "acme/shop#5", "@alice", false, None, 20).unwrap());
        all.push(audit_record(&all, "acme/shop#2", "@bob", true, Some("x"), 30).unwrap());
        // An audit written before the mark existed.
        let mut legacy = at("acme/shop#6", 25, "merged", false);
        legacy.audited = true;
        all.push(legacy);
        let q = audits(&all, 100);
        let changes = |v: &[Outcome]| v.iter().map(|o| o.change.clone()).collect::<Vec<_>>();
        assert_eq!(
            changes(&q.awaiting),
            ["acme/shop#1", "acme/shop#3", "acme/shop#4"]
        );
        assert_eq!(q.awaiting[1].result, "reverted");
        assert_eq!(
            changes(&q.audited),
            ["acme/shop#2", "acme/shop#6", "acme/shop#5"]
        );
        let cut = audits(&all, 1);
        assert_eq!((cut.awaiting.len(), cut.audited.len()), (1, 1));
        assert_eq!(audits(&[], 100), Audits::default());
    }

    #[test]
    fn summaries_count_the_sample() {
        let at = |change: &str, sampled: bool, audited: bool, missed: bool| Outcome {
            change: change.into(),
            sampled,
            ..o("a/b/c", Lane::AutoMerge, "merged", audited, missed)
        };
        let all = vec![
            at("acme/shop#1", true, true, true),
            at("acme/shop#2", true, true, false),
            at("acme/shop#3", true, false, false),
            // Chosen by a person, not sampled.
            at("acme/shop#4", false, true, true),
            at("acme/shop#5", false, false, false),
        ];
        let s = summarize(&all);
        assert_eq!(s.total.sampled, 3);
        assert_eq!(s.awaiting_audit, 1);
        assert_eq!(s.sampled_miss_rate, Some(0.5));
        assert_eq!(s.audit_miss_rate, Some(2.0 / 3.0));
        assert_eq!(summarize(&all[3..]).sampled_miss_rate, None);
        // A summary stored before these fields still reads.
        let old = r#"{"total":{"changes":0,"merged":0,"reverted":0,"incidents":0,"autoMerged":0,"audited":0,"missed":0},"byAgent":{},"byJudge":{},"byLane":{},"humanShare":null,"auditMissRate":null}"#;
        let s: Summary = serde_json::from_str(old).unwrap();
        assert_eq!((s.awaiting_audit, s.total.sampled), (0, 0));
    }

    #[test]
    fn unsampled_lines_stay_the_same() {
        let old = r#"{"at":1,"change":"x","agent":"a//","lane":"auto-merge","result":"merged","audited":false,"missed":false}"#;
        let o: Outcome = serde_json::from_str(old).unwrap();
        assert!(!o.sampled && o.audit.is_none());
        let line = serde_json::to_string(&o).unwrap();
        assert!(
            !line.contains("sampled") && !line.contains("\"audit\""),
            "{line}"
        );
        let mut s = o.clone();
        s.sampled = true;
        assert!(
            serde_json::to_string(&s)
                .unwrap()
                .contains(r#""sampled":true"#)
        );
        // A later revert keeps the sample and the audit.
        s.commit = Some("abc1234".into());
        s = audit_record(&[s], "x", "@a", false, None, 2).unwrap();
        let reverts = [Revert {
            revert: "f".repeat(40),
            reverted: "abc1234".into(),
            subject: "Revert".into(),
        }];
        let added = ingest_reverts(std::slice::from_ref(&s), &reverts, 5);
        assert!(added[0].sampled && added[0].audit.is_some());
    }

    #[test]
    fn who_merged_is_kept_and_old_lines_stay_the_same() {
        let old = r#"{"at":1,"change":"x","agent":"a//","judge":null,"lane":"human","verdict":null,"result":"merged","audited":false,"missed":false}"#;
        let o: Outcome = serde_json::from_str(old).unwrap();
        assert_eq!(o.merged_by, None);
        // Written without it, a line keeps the same bytes.
        assert_eq!(serde_json::to_string(&o).unwrap(), old);
        let mut merged = o.clone();
        merged.merged_by = Some("@mike".into());
        merged.commit = Some("abc1234".into());
        let line = serde_json::to_string(&merged).unwrap();
        assert!(line.ends_with(r#","mergedBy":"@mike"}"#), "{line}");
        assert_eq!(serde_json::from_str::<Outcome>(&line).unwrap(), merged);
        // A revert carries it forward.
        let reverts = [Revert {
            revert: "f".repeat(40),
            reverted: "abc1234".into(),
            subject: "Revert".into(),
        }];
        let added = ingest_reverts(&[merged], &reverts, 5);
        assert_eq!(added[0].merged_by.as_deref(), Some("@mike"));
    }

    #[test]
    fn rollbacks_count_against_the_setup_unless_planned() {
        let at = |change: &str, result: &str, planned: bool| Outcome {
            change: change.into(),
            planned,
            ..o("a/b/c", Lane::AutoMerge, result, false, false)
        };
        let all = vec![
            at("acme/shop#1", "merged", false),
            at("acme/shop#2", "merged", false),
            at("acme/shop#3", "merged", false),
            at("acme/shop#1", "rolled-back", false),
            // A flag turned off as planned.
            at("acme/shop#2", "rolled-back", true),
            // Rolled back, then reverted: still a rollback.
            at("acme/shop#3", "rolled-back", false),
            at("acme/shop#3", "reverted", false),
        ];
        // The planned one still counts as merged.
        let r = record_for(&all, "a/b/c");
        assert_eq!(
            (r.changes, r.recent_incidents, r.recent_rollbacks),
            (1, 0, 2)
        );
        let s = summarize(&all);
        assert_eq!((s.total.rolled_back, s.total.reverted), (2, 3));
        // A planned rollback is no history either.
        let with = |c: &str, result: &str, planned: bool| Outcome {
            components: vec!["billing".into()],
            ..at(c, result, planned)
        };
        let areas: BTreeSet<String> = ["billing".to_string()].into();
        let h = area_history(
            &[
                with("acme/shop#1", "rolled-back", false),
                with("acme/shop#2", "rolled-back", true),
            ],
            &areas,
            &HistoryScope::default(),
        );
        assert_eq!(h["billing"].reverts, 1);
        assert_eq!(h["billing"].changes, ["acme/shop#1"]);
    }

    #[test]
    fn lines_without_signal_or_planned_stay_the_same() {
        let old = r#"{"at":1,"change":"x","agent":"a//","judge":null,"lane":"judge","verdict":null,"result":"rolled-back","audited":false,"missed":false}"#;
        let o: Outcome = serde_json::from_str(old).unwrap();
        assert_eq!((o.signal.as_deref(), o.planned), (None, false));
        assert_eq!(serde_json::to_string(&o).unwrap(), old);
        let mut from = o.clone();
        from.signal = Some("rollout:x:25:9".into());
        from.planned = true;
        let line = serde_json::to_string(&from).unwrap();
        assert!(
            line.ends_with(r#","signal":"rollout:x:25:9","planned":true}"#),
            "{line}"
        );
        assert_eq!(serde_json::from_str::<Outcome>(&line).unwrap(), from);
    }
}
