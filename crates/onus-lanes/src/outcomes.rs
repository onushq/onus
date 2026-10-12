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
    /// The components the change touched (from its report): what per-area
    /// history counts reverts and incidents against.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// Who approved it on review (`@login`). Empty in the human lane means
    /// it merged without a counted review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviewed_by: Vec<String>,
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
    let had = had_incident(outcomes);
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
            .filter(|o| had.contains(o.change.as_str()))
            .count() as u32,
    }
}

/// Changes that caused an incident at any point: reverting one later does
/// not erase the incident from its agent setup's record.
fn had_incident(outcomes: &[Outcome]) -> std::collections::HashSet<&str> {
    outcomes
        .iter()
        .filter(|o| o.result == "incident")
        .map(|o| o.change.as_str())
        .collect()
}

/// What the history input looks at: since when, which repository's changes
/// (`owner/repo` of `owner/repo#123`; None = all), and the change being
/// classified (left out of its own history).
#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryScope<'a> {
    /// Unix seconds; earlier records do not count.
    pub since: u64,
    pub repo: Option<&'a str>,
    pub exclude: Option<&'a str>,
}

/// The `owner/repo` of a change id `owner/repo#123`.
pub fn repo_of(change: &str) -> Option<&str> {
    change.split_once('#').map(|(repo, _)| repo)
}

/// Reverts (and rollbacks) and incidents per component among `areas`,
/// recorded at or after `scope.since`. A change's areas are its
/// `components` plus the component of each `involved` id. Every record
/// counts, not only the latest of each change: a revert and an incident are
/// events at their own time. A change counts once per area and kind.
/// Records of a bare commit (no `#`) count for every repository.
pub fn area_history(
    outcomes: &[Outcome],
    areas: &BTreeSet<String>,
    scope: &HistoryScope,
) -> AreaHistories {
    // Area → (reverted changes, changes with incidents).
    type Changes<'o> = (BTreeSet<&'o str>, BTreeSet<&'o str>);
    let mut found: BTreeMap<&str, Changes> = BTreeMap::new();
    for o in outcomes {
        if o.at < scope.since || scope.exclude == Some(o.change.as_str()) {
            continue;
        }
        if let (Some(want), Some(repo)) = (scope.repo, repo_of(&o.change))
            && want != repo
        {
            continue;
        }
        let revert = match o.result.as_str() {
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
            let (reverts, incidents) = found.entry(c).or_default();
            if revert {
                reverts.insert(&o.change);
            } else {
                incidents.insert(&o.change);
            }
        }
    }
    found
        .into_iter()
        .map(|(c, (reverts, incidents))| {
            let changes: BTreeSet<&str> = reverts.union(&incidents).copied().collect();
            (
                c.to_string(),
                AreaHistory {
                    reverts: reverts.len() as u32,
                    incidents: incidents.len() as u32,
                    changes: changes.into_iter().map(str::to_string).collect(),
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
    fn add(&mut self, o: &Outcome, incident: bool) {
        self.changes += 1;
        match o.result.as_str() {
            "merged" => self.merged += 1,
            // A flag turned off or a rollout rolled back undoes a change too.
            "reverted" | "rolled-back" => self.reverted += 1,
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
    let had = had_incident(outcomes);
    for o in latest(outcomes) {
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
                repo: Some("acme/web"),
                exclude: None,
            },
        );
        assert_eq!(web["logger"].changes, ["acme/web#2"]);
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
}
