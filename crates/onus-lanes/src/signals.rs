//! Production signals in the records (ADR 0016): `signals.jsonl`, the third
//! records file beside `pulls.jsonl` and `outcomes.jsonl`. Each signal is
//! kept once, by its id, and tied to a change: by the change it names when
//! that is on record, else by its commit. An incident or a rollback tied to
//! a change also becomes that change's outcome, through the same outcome
//! logic as `/onus incident`, so the agent setup's track record and every
//! summary count it. A signal tied to no change is still kept: it counts
//! toward its components' history.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use onus_core::ids;
use onus_core::signal::{ProductionSignal, Severity, SignalKind};
use serde::Serialize;

use crate::classify::{AreaHistories, AreaHistory};
use crate::outcomes::{self, HistoryScope, Outcome};
use crate::pulls::{self, PullRecord};

/// The signals file in a records folder.
pub const FILE: &str = "signals.jsonl";

/// The signals on record, the first line of each id only: a records branch
/// merged from two runs can hold the same signal twice. A missing file is
/// no signals.
pub fn load(path: &Path) -> Result<Vec<ProductionSignal>> {
    let Ok(f) = std::fs::File::open(path) else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for (i, line) in std::io::BufReader::new(f).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line).with_context(|| {
            format!(
                "line {} of {} is not a production signal",
                i + 1,
                path.display()
            )
        })?);
    }
    Ok(first_of_each(out))
}

/// The first signal of each id, in order.
pub fn first_of_each(signals: Vec<ProductionSignal>) -> Vec<ProductionSignal> {
    let mut seen = HashSet::new();
    signals
        .into_iter()
        .filter(|s| seen.insert(s.id.clone()))
        .collect()
}

pub fn append(path: &Path, signal: &ProductionSignal) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    writeln!(f, "{}", serde_json::to_string(signal)?)?;
    Ok(())
}

/// Whether `s` is a change id, `owner/repo#123`.
pub fn is_change(s: &str) -> bool {
    s.split_once('#')
        .is_some_and(|(repo, n)| repo.contains('/') && n.parse::<u64>().is_ok())
}

/// Whether `s` is a commit: hex, at least 7 digits.
pub fn is_commit(s: &str) -> bool {
    s.len() >= 7 && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Why a signal cannot be recorded, if it cannot.
pub fn check(s: &ProductionSignal) -> Result<(), String> {
    if s.id.trim().is_empty() {
        return Err("`id` is empty".into());
    }
    if let Some(c) = &s.commit
        && !is_commit(c)
    {
        return Err(format!(
            "`commit` {c:?} is not a commit: write at least 7 hex digits"
        ));
    }
    if let Some(c) = &s.change
        && !is_change(c)
    {
        return Err(format!(
            "`change` {c:?} is not a change: write owner/repo#123"
        ));
    }
    match s.kind {
        SignalKind::Trace if s.calls.is_empty() => {
            return Err("a trace signal needs `calls`".into());
        }
        SignalKind::Deploy => {
            if s.commit.is_none() && s.change.is_none() {
                return Err("a deploy names its `commit` or `change`".into());
            }
            if s.rollout.as_ref().is_some_and(|r| r.decision.is_some()) {
                return Err(
                    "a deploy carries no `rollout.decision`: decisions are Onus's (onus ci rollout)"
                        .into(),
                );
            }
        }
        SignalKind::Rollout => {}
        _ if s.rollout.is_some() => {
            return Err(format!(
                "`rollout` belongs on a deploy, not on a {} signal",
                s.kind.as_str()
            ));
        }
        _ => {}
    }
    if let Some(r) = &s.rollout
        && r.percent > 100
    {
        return Err(format!("`rollout.percent` {} is above 100", r.percent));
    }
    Ok(())
}

/// Whether two commits, either one a prefix, are the same.
fn same_commit(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    !a.is_empty() && !b.is_empty() && (a.starts_with(&b) || b.starts_with(&a))
}

/// The changes a commit may be: those whose landed commit (latest outcome)
/// or head (latest pull record) it matches, most recent first.
fn by_commit(commit: &str, outcomes: &[Outcome], pulls: &[PullRecord]) -> Vec<String> {
    let mut at: BTreeMap<&str, u64> = BTreeMap::new();
    for o in outcomes::latest(outcomes) {
        if o.commit.as_deref().is_some_and(|c| same_commit(c, commit)) {
            let t = at.entry(&o.change).or_default();
            *t = (*t).max(o.at);
        }
    }
    let mut last: BTreeMap<&str, &PullRecord> = BTreeMap::new();
    for p in pulls {
        last.insert(&p.change, p);
    }
    for p in last.into_values() {
        if same_commit(&p.head, commit) {
            let t = at.entry(&p.change).or_default();
            *t = (*t).max(p.at);
        }
    }
    let mut found: Vec<(&str, u64)> = at.into_iter().collect();
    found.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    found.into_iter().map(|(c, _)| c.to_string()).collect()
}

/// How a signal is tied to a change, and the other changes its commit also
/// matched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attribution {
    pub change: Option<String>,
    /// `change` or `commit`.
    pub by: Option<&'static str>,
    pub also: Vec<String>,
}

/// Ties a signal to a change: the change it names, when that is on record;
/// else the change whose landed commit or head its commit matches (either
/// one a prefix of the other), the most recent when several do.
pub fn attribution(
    sig: &ProductionSignal,
    outcomes: &[Outcome],
    pulls: &[PullRecord],
) -> Attribution {
    if let Some(c) = &sig.change
        && (outcomes.iter().any(|o| &o.change == c) || pulls.iter().any(|p| &p.change == c))
    {
        return Attribution {
            change: Some(c.clone()),
            by: Some("change"),
            also: vec![],
        };
    }
    if let Some(commit) = &sig.commit {
        let mut found = by_commit(commit, outcomes, pulls).into_iter();
        if let Some(first) = found.next() {
            return Attribution {
                change: Some(first),
                by: Some("commit"),
                also: found.collect(),
            };
        }
    }
    Attribution::default()
}

/// [`attribution`]: the change, and how it was found.
pub fn attribute(
    sig: &ProductionSignal,
    outcomes: &[Outcome],
    pulls: &[PullRecord],
) -> (Option<String>, Option<&'static str>) {
    let a = attribution(sig, outcomes, pulls);
    (a.change, a.by)
}

/// A new outcome of `change` with `result`, from its latest outcome, else
/// from its latest pull record the way `onus ci closed` builds one. None
/// when Onus has neither.
pub fn outcome_of(
    change: &str,
    outcomes: &[Outcome],
    pulls: &[PullRecord],
    at: u64,
    result: &str,
) -> Option<Outcome> {
    if let Some(last) = outcomes::latest(outcomes)
        .into_iter()
        .find(|o| o.change == change)
    {
        return Some(Outcome {
            at,
            result: result.into(),
            signal: None,
            planned: false,
            ..last.clone()
        });
    }
    let p = pulls::latest(pulls, change)?;
    Some(Outcome {
        at,
        change: change.to_string(),
        agent: p.agent.key(),
        judge: p.judge.clone(),
        lane: p.lane,
        verdict: p.verdict.clone(),
        result: result.into(),
        audited: false,
        missed: false,
        commit: None,
        involved: vec![],
        note: None,
        components: p.components.clone(),
        reviewed_by: p.reviewed_by.clone(),
        approval: p.approval,
        sampled: false,
        audit: None,
        merged_by: None,
        features: p.features.clone(),
        caught: p.caught,
        signal: None,
        planned: false,
    })
}

/// The outcome an incident or rollback signal tied to `change` adds:
/// `incident` or `rolled-back`, naming the signal. None for every other
/// kind, when an outcome already names the signal, or when the change's
/// latest result is already that (a planned rollback does not count: a
/// rollback after it is a new one).
pub fn derive_outcome(
    sig: &ProductionSignal,
    change: &str,
    outcomes: &[Outcome],
    pulls: &[PullRecord],
    now: u64,
) -> Option<Outcome> {
    let result = match sig.kind {
        SignalKind::Incident => "incident",
        SignalKind::Rollback => "rolled-back",
        _ => return None,
    };
    if outcomes
        .iter()
        .any(|o| o.signal.as_deref() == Some(sig.id.as_str()))
    {
        return None;
    }
    if outcomes::latest(outcomes)
        .into_iter()
        .any(|o| o.change == change && o.result == result && !o.planned)
    {
        return None;
    }
    let mut involved: Vec<String> = Vec::new();
    for i in sig.components.iter().chain(&sig.symbols) {
        if !involved.contains(i) {
            involved.push(i.clone());
        }
    }
    let head = [sig.source.as_str(), sig.id.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Some(Outcome {
        involved,
        note: Some(match &sig.note {
            Some(n) => format!("{head}: {n}"),
            None => head,
        }),
        signal: Some(sig.id.clone()),
        ..outcome_of(change, outcomes, pulls, sig.at.unwrap_or(now), result)?
    })
}

/// A signal tied to a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Attributed {
    pub id: String,
    pub change: String,
    /// `change` or `commit`.
    pub by: String,
}

/// An outcome a signal added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Derived {
    pub change: String,
    pub result: String,
}

/// What an ingest recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct IngestSummary {
    /// Signals recorded.
    pub recorded: u32,
    /// Signals already on record, by id.
    pub duplicates: u32,
    pub attributed: Vec<Attributed>,
    pub outcomes: Vec<Derived>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Records signals in `records`: each new id once, tied to a change, with
/// `at` (default `now`), `change` and `attributedBy` filled in, and the
/// components of the change's pull record when the signal names none. An
/// incident or rollback tied to a change also adds its outcome. `change`
/// and `attributedBy` are Onus's: a posted `attributedBy` is dropped, and so
/// is a posted `change` that is not on record, so a signal cannot claim a
/// tie Onus did not find. Fails, and records nothing, when a signal is
/// malformed or claims to be Onus's own rollout decision.
pub fn ingest(records: &Path, input: Vec<ProductionSignal>, now: u64) -> Result<IngestSummary> {
    for (i, s) in input.iter().enumerate() {
        if let Err(why) = check(s) {
            bail!("signal {} ({}): {why}", i + 1, s.id);
        }
        if s.kind == SignalKind::Rollout {
            bail!(
                "signal {} ({}): rollout stages carry Onus's decisions; they are written by `onus ci rollout`",
                i + 1,
                s.id
            );
        }
    }
    std::fs::create_dir_all(records)?;
    let (signals_file, outcomes_file) = (records.join(FILE), records.join("outcomes.jsonl"));
    let mut all = outcomes::load(&outcomes_file)?;
    let pulls = pulls::load(&records.join("pulls.jsonl"))?;
    let mut seen: HashSet<String> = load(&signals_file)?.into_iter().map(|s| s.id).collect();
    let mut summary = IngestSummary::default();
    for mut s in input {
        if !seen.insert(s.id.clone()) {
            summary.duplicates += 1;
            continue;
        }
        s.at = Some(s.at.unwrap_or(now));
        s.attributed_by = None;
        let a = attribution(&s, &all, &pulls);
        if let (Some(change), Some(by)) = (&a.change, a.by) {
            if !a.also.is_empty() {
                summary.warnings.push(format!(
                    "{}: commit {} matches {} too; tied to {change}, the most recent",
                    s.id,
                    s.commit.as_deref().unwrap_or_default(),
                    a.also.join(", ")
                ));
            }
            s.change = Some(change.clone());
            s.attributed_by = Some(by.to_string());
            if s.components.is_empty() && s.symbols.is_empty() {
                s.components = pulls::latest(&pulls, change)
                    .map(|p| p.components.clone())
                    .or_else(|| {
                        outcomes::latest(&all)
                            .into_iter()
                            .find(|o| &o.change == change)
                            .map(|o| o.components.clone())
                    })
                    .unwrap_or_default();
            }
            summary.attributed.push(Attributed {
                id: s.id.clone(),
                change: change.clone(),
                by: by.to_string(),
            });
        } else if let Some(c) = s.change.take() {
            summary.warnings.push(format!(
                "{}: {c} is not on record; kept tied to no change",
                s.id
            ));
        }
        append(&signals_file, &s)?;
        summary.recorded += 1;
        if let Some(change) = &a.change
            && let Some(o) = derive_outcome(&s, change, &all, &pulls, now)
        {
            outcomes::append(&outcomes_file, &o)?;
            summary.outcomes.push(Derived {
                change: o.change.clone(),
                result: o.result.clone(),
            });
            all.push(o);
        }
    }
    Ok(summary)
}

/// Whether an error spike, latency or metric signal counts: medium
/// severity or more, or none given.
fn counts_as_spike(s: &ProductionSignal) -> bool {
    matches!(
        s.kind,
        SignalKind::ErrorSpike | SignalKind::Latency | SignalKind::Metric
    ) && s.severity.is_none_or(|v| v >= Severity::Medium)
}

/// Adds the production signals recorded in the window to the area history
/// of `wanted` components: incidents and rollbacks tied to no change (those
/// tied to one already became outcomes, which `outcomes::area_history`
/// counts), and error spikes, latency and metric signals. Deploys, rollout
/// stages and traces are not history. A signal's components are its
/// `components` and the components of its `symbols`.
pub fn extend_history(
    areas: &mut AreaHistories,
    signals: &[ProductionSignal],
    wanted: &BTreeSet<String>,
    scope: &HistoryScope,
) {
    for s in signals {
        let at = s.at.unwrap_or(0);
        if at < scope.since || scope.until.is_some_and(|until| at > until) {
            continue;
        }
        if let Some(c) = &s.change {
            if scope.exclude == Some(c.as_str()) {
                continue;
            }
            if let (Some(want), Some(repo)) = (scope.repo, outcomes::repo_of(c))
                && want != repo
            {
                continue;
            }
        }
        let attributed = s.attributed_by.is_some();
        enum Count {
            Incident,
            Revert,
            Spike,
        }
        let what = match s.kind {
            SignalKind::Incident if !attributed => Count::Incident,
            SignalKind::Rollback if !attributed => Count::Revert,
            _ if counts_as_spike(s) => Count::Spike,
            _ => continue,
        };
        let touched: BTreeSet<&str> = s
            .components
            .iter()
            .map(String::as_str)
            .chain(s.symbols.iter().map(|i| ids::component_of(i).unwrap_or(i)))
            .filter(|c| wanted.contains(*c))
            .collect();
        for c in touched {
            let h: &mut AreaHistory = areas.entry(c.to_string()).or_default();
            match what {
                Count::Incident => h.incidents += 1,
                Count::Revert => h.reverts += 1,
                Count::Spike => h.error_spikes += 1,
            }
            if !matches!(what, Count::Spike) {
                let who = s.change.clone().unwrap_or_else(|| s.id.clone());
                if let Err(i) = h.changes.binary_search(&who) {
                    h.changes.insert(i, who);
                }
            }
            h.last_at = Some(h.last_at.unwrap_or(0).max(at));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::submission::AgentSetup;
    use onus_core::Lane;
    use onus_core::signal::{MetricSample, RolloutStage};

    fn signal(id: &str, kind: SignalKind) -> ProductionSignal {
        serde_json::from_value(serde_json::json!({ "id": id, "kind": kind })).unwrap()
    }

    fn pull(change: &str, head: &str, at: u64) -> PullRecord {
        PullRecord {
            at,
            change: change.into(),
            base: "b".into(),
            head: head.into(),
            agent: AgentSetup {
                tool: "codex".into(),
                ..Default::default()
            },
            lane: Lane::Judge,
            verdict: None,
            judge: None,
            approvals: vec![],
            components: vec!["billing".into()],
            requested: vec![],
            reviewed_by: vec![],
            approval: None,
            audit_after_merge: None,
            features: None,
            caught: false,
            rollout: None,
        }
    }

    fn merged(p: &PullRecord, commit: &str, at: u64) -> Outcome {
        Outcome {
            commit: Some(commit.into()),
            ..outcome_of(&p.change, &[], std::slice::from_ref(p), at, "merged").unwrap()
        }
    }

    #[test]
    fn signals_are_tied_by_change_then_commit() {
        let p1 = pull("acme/shop#1", "aaaa1111", 1);
        let p2 = pull("acme/shop#2", "bbbb2222cccc", 2);
        let p3 = pull("acme/shop#3", "ffff0000", 3);
        let outcomes = vec![
            merged(&p1, "1234567890abcdef", 10),
            merged(&p2, "1234567fff", 20),
        ];
        let pulls = vec![p1, p2, p3];
        let tie = |change: Option<&str>, commit: Option<&str>| {
            let mut s = signal("x", SignalKind::Incident);
            s.change = change.map(str::to_string);
            s.commit = commit.map(str::to_string);
            attribution(&s, &outcomes, &pulls)
        };
        let a = tie(Some("acme/shop#3"), None);
        assert_eq!(
            (a.change.as_deref(), a.by),
            (Some("acme/shop#3"), Some("change"))
        );
        // A change not on record falls back to the commit.
        let a = tie(Some("acme/shop#9"), Some("ffff0000aaaa"));
        assert_eq!(
            (a.change.as_deref(), a.by),
            (Some("acme/shop#3"), Some("commit"))
        );
        // A prefix either way.
        assert_eq!(
            tie(None, Some("1234567890abcdef0000")).change.as_deref(),
            Some("acme/shop#1")
        );
        assert_eq!(
            tie(None, Some("bbbb222")).change.as_deref(),
            Some("acme/shop#2")
        );
        // Several: the most recent, and the others named.
        let a = tie(None, Some("1234567"));
        assert_eq!(a.change.as_deref(), Some("acme/shop#2"));
        assert_eq!(a.also, ["acme/shop#1"]);
        assert_eq!(tie(None, Some("9999999")), Attribution::default());
        assert_eq!(tie(None, None), Attribution::default());
        assert_eq!(
            attribute(&signal("x", SignalKind::Incident), &outcomes, &pulls),
            (None, None)
        );
    }

    fn records() -> (tempfile::TempDir, PullRecord) {
        let dir = tempfile::tempdir().unwrap();
        let p = pull("acme/shop#1", "aaaa1111bbbb", 1);
        pulls::append(&dir.path().join("pulls.jsonl"), &p).unwrap();
        outcomes::append(
            &dir.path().join("outcomes.jsonl"),
            &merged(&p, "abcdef1234567", 10),
        )
        .unwrap();
        (dir, p)
    }

    #[test]
    fn incidents_and_rollbacks_become_outcomes_once() {
        let (dir, _) = records();
        let r = dir.path();
        let mut incident = signal("pd:Q1", SignalKind::Incident);
        incident.commit = Some("abcdef1".into());
        incident.source = "pagerduty".into();
        incident.note = Some("checkout 500s".into());
        incident.symbols = vec!["billing:src/a.ts#charge".into()];
        let mut metric = signal("dd:7", SignalKind::Metric);
        metric.components = vec!["billing".into()];
        metric.metrics = vec![MetricSample {
            name: "checkout_conversion".into(),
            value: 0.02,
            baseline: Some(0.03),
            unit: None,
        }];
        let lost = signal("sentry:9", SignalKind::Incident);
        let s = ingest(
            r,
            vec![incident.clone(), metric, incident.clone(), lost],
            100,
        )
        .unwrap();
        assert_eq!((s.recorded, s.duplicates), (3, 1));
        assert_eq!(
            s.attributed,
            [Attributed {
                id: "pd:Q1".into(),
                change: "acme/shop#1".into(),
                by: "commit".into()
            }]
        );
        assert_eq!(
            s.outcomes,
            [Derived {
                change: "acme/shop#1".into(),
                result: "incident".into()
            }]
        );
        let all = outcomes::load(&r.join("outcomes.jsonl")).unwrap();
        let last = all.last().unwrap();
        assert_eq!(last.result, "incident");
        assert_eq!(last.signal.as_deref(), Some("pd:Q1"));
        assert_eq!(last.note.as_deref(), Some("pagerduty pd:Q1: checkout 500s"));
        assert_eq!(last.involved, ["billing:src/a.ts#charge"]);
        assert_eq!(last.commit.as_deref(), Some("abcdef1234567"));
        assert_eq!(last.at, 100);
        // Every signal is kept, the unattributed one too, with its time.
        let kept = load(&r.join(FILE)).unwrap();
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].attributed_by.as_deref(), Some("commit"));
        assert_eq!(kept[0].change.as_deref(), Some("acme/shop#1"));
        assert!(kept[2].attributed_by.is_none() && kept[2].at == Some(100));
        // The metric signal named its components; nothing became an outcome.
        assert_eq!(kept[1].components, ["billing"]);
        // Again: nothing new.
        let again = ingest(r, vec![incident], 200).unwrap();
        assert_eq!((again.recorded, again.duplicates), (0, 1));
        assert_eq!(outcomes::load(&r.join("outcomes.jsonl")).unwrap().len(), 2);
        // A new id about the same change does not repeat the incident.
        let mut second = signal("pd:Q2", SignalKind::Incident);
        second.change = Some("acme/shop#1".into());
        let s = ingest(r, vec![second], 300).unwrap();
        assert_eq!((s.recorded, s.outcomes.len()), (1, 0));
        // A rollback does.
        let mut rollback = signal("argo:rb1", SignalKind::Rollback);
        rollback.change = Some("acme/shop#1".into());
        let s = ingest(r, vec![rollback], 400).unwrap();
        assert_eq!(s.outcomes[0].result, "rolled-back");
        let all = outcomes::load(&r.join("outcomes.jsonl")).unwrap();
        assert_eq!(all.last().unwrap().signal.as_deref(), Some("argo:rb1"));
        assert!(!all.last().unwrap().planned);
    }

    #[test]
    fn a_posted_signal_cannot_claim_a_tie_onus_did_not_find() {
        let (dir, _) = records();
        let r = dir.path();
        let mut claimed = signal("pd:x", SignalKind::Incident);
        claimed.components = vec!["billing".into()];
        claimed.change = Some("acme/shop#77".into());
        claimed.attributed_by = Some("change".into());
        let mut deploy = signal("deploy:1", SignalKind::Deploy);
        deploy.change = Some("acme/shop#77".into());
        let s = ingest(r, vec![claimed, deploy], 100).unwrap();
        assert_eq!((s.recorded, s.attributed.len()), (2, 0));
        assert!(
            s.warnings[0].contains("acme/shop#77 is not on record"),
            "{s:?}"
        );
        // Kept tied to no change, so it counts toward its component, and
        // no change's rollout counts the deploy.
        let kept = load(&r.join(FILE)).unwrap();
        assert!(
            kept.iter()
                .all(|k| k.change.is_none() && k.attributed_by.is_none()),
            "{kept:?}"
        );
        let mut areas = AreaHistories::new();
        extend_history(
            &mut areas,
            &kept,
            &["billing".to_string()].into(),
            &HistoryScope::default(),
        );
        assert_eq!(areas["billing"].incidents, 1);
        // A posted attributedBy on a change on record is Onus's own again.
        let mut named = signal("pd:y", SignalKind::Metric);
        named.commit = Some("abcdef1".into());
        named.attributed_by = Some("change".into());
        ingest(r, vec![named], 100).unwrap();
        let kept = load(&r.join(FILE)).unwrap();
        assert_eq!(kept[2].attributed_by.as_deref(), Some("commit"));
    }

    #[test]
    fn a_rollback_after_a_planned_one_counts() {
        let (dir, p) = records();
        let r = dir.path();
        let planned = Outcome {
            planned: true,
            ..outcome_of(&p.change, &[], std::slice::from_ref(&p), 20, "rolled-back").unwrap()
        };
        outcomes::append(&r.join("outcomes.jsonl"), &planned).unwrap();
        let mut rollback = signal("argo:rb2", SignalKind::Rollback);
        rollback.change = Some(p.change.clone());
        let s = ingest(r, vec![rollback], 30).unwrap();
        assert_eq!(s.outcomes[0].result, "rolled-back");
        let last = outcomes::load(&r.join("outcomes.jsonl"))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            (last.planned, last.signal.as_deref()),
            (false, Some("argo:rb2"))
        );
    }

    #[test]
    fn a_change_still_open_gets_its_outcome_from_its_pull_record() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let p = pull("acme/shop#5", "cafe0000cafe", 1);
        pulls::append(&r.join("pulls.jsonl"), &p).unwrap();
        let mut incident = signal("pd:5", SignalKind::Incident);
        incident.commit = Some("cafe000".into());
        let s = ingest(r, vec![incident], 50).unwrap();
        assert_eq!(s.outcomes.len(), 1);
        let o = &outcomes::load(&r.join("outcomes.jsonl")).unwrap()[0];
        assert_eq!(
            (o.change.as_str(), o.agent.as_str()),
            ("acme/shop#5", "codex//")
        );
        assert_eq!(o.components, ["billing"]);
        assert_eq!(o.note.as_deref(), Some("pd:5"));
        // Its components came from the pull record too.
        assert_eq!(load(&r.join(FILE)).unwrap()[0].components, ["billing"]);
    }

    #[test]
    fn malformed_signals_and_posted_decisions_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let refused = |s: ProductionSignal| {
            ingest(dir.path(), vec![signal("ok", SignalKind::Metric), s], 1)
                .unwrap_err()
                .to_string()
        };
        assert!(refused(signal(" ", SignalKind::Incident)).contains("signal 2"));
        let mut bad = signal("x", SignalKind::Incident);
        bad.commit = Some("xyz".into());
        assert!(refused(bad).contains("is not a commit"));
        let mut bad = signal("x", SignalKind::Incident);
        bad.change = Some("nope".into());
        assert!(refused(bad).contains("owner/repo#123"));
        assert!(refused(signal("x", SignalKind::Trace)).contains("needs `calls`"));
        let mut bad = signal("x", SignalKind::Incident);
        bad.rollout = Some(RolloutStage {
            percent: 5,
            decision: None,
            flag: None,
        });
        assert!(refused(bad).contains("belongs on a deploy"));
        let mut posted = signal("x", SignalKind::Rollout);
        posted.change = Some("acme/shop#1".into());
        assert!(refused(posted).contains("written by `onus ci rollout`"));
        // A deploy names what it deployed, and carries no decision.
        assert!(refused(signal("d", SignalKind::Deploy)).contains("names its `commit`"));
        let mut decided = signal("d", SignalKind::Deploy);
        decided.commit = Some("abcdef1".into());
        decided.rollout = Some(RolloutStage {
            percent: 25,
            decision: Some("promote".into()),
            flag: None,
        });
        assert!(refused(decided).contains("no `rollout.decision`"));
        // Nothing was recorded.
        assert!(load(&dir.path().join(FILE)).unwrap().is_empty());
    }

    #[test]
    fn a_deploy_is_tied_and_kept_and_is_no_outcome() {
        let (dir, _) = records();
        let r = dir.path();
        let mut deploy = signal("deploy:4711:25", SignalKind::Deploy);
        deploy.commit = Some("abcdef1234".into());
        deploy.environment = Some("production".into());
        deploy.rollout = Some(RolloutStage {
            percent: 25,
            decision: None,
            flag: None,
        });
        let s = ingest(r, vec![deploy], 5).unwrap();
        assert_eq!(
            (s.recorded, s.attributed.len(), s.outcomes.len()),
            (1, 1, 0)
        );
        let kept = &load(&r.join(FILE)).unwrap()[0];
        assert_eq!(kept.change.as_deref(), Some("acme/shop#1"));
        assert_eq!(kept.rollout.as_ref().unwrap().percent, 25);
    }

    #[test]
    fn the_first_line_of_an_id_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let mut a = signal("x", SignalKind::Metric);
        a.at = Some(1);
        append(&path, &a).unwrap();
        a.at = Some(2);
        append(&path, &a).unwrap();
        let all = load(&path).unwrap();
        assert_eq!((all.len(), all[0].at), (1, Some(1)));
        assert!(load(&dir.path().join("none.jsonl")).unwrap().is_empty());
    }

    #[test]
    fn signals_extend_the_area_history() {
        let s = |id: &str, kind: SignalKind, at: u64, f: &dyn Fn(&mut ProductionSignal)| {
            let mut x = signal(id, kind);
            x.at = Some(at);
            f(&mut x);
            x
        };
        let all = vec![
            // A symbol counts for its component.
            s("pd:1", SignalKind::Incident, 100, &|x| {
                x.symbols = vec!["billing:src/a.ts#charge".into()]
            }),
            // Tied to a change: already an outcome, not counted again.
            s("pd:2", SignalKind::Incident, 100, &|x| {
                x.components = vec!["billing".into()];
                x.change = Some("acme/shop#1".into());
                x.attributed_by = Some("change".into());
            }),
            s("argo:1", SignalKind::Rollback, 110, &|x| {
                x.components = vec!["billing".into(), "orders".into()]
            }),
            s("dd:1", SignalKind::ErrorSpike, 120, &|x| {
                x.components = vec!["billing".into()];
                x.change = Some("acme/shop#1".into());
                x.attributed_by = Some("change".into());
            }),
            s("dd:2", SignalKind::Latency, 130, &|x| {
                x.components = vec!["billing".into()];
                x.severity = Some(Severity::Low);
            }),
            // Before the window.
            s("pd:0", SignalKind::Incident, 10, &|x| {
                x.components = vec!["billing".into()]
            }),
            // Not history.
            s("deploy:1", SignalKind::Deploy, 140, &|x| {
                x.components = vec!["billing".into()];
                x.commit = Some("abcdef1".into());
            }),
            // Another repository's change.
            s("dd:3", SignalKind::Metric, 150, &|x| {
                x.components = vec!["billing".into()];
                x.change = Some("acme/web#1".into());
            }),
        ];
        let wanted: BTreeSet<String> = ["billing".to_string()].into();
        let mut areas = AreaHistories::new();
        extend_history(
            &mut areas,
            &all,
            &wanted,
            &HistoryScope {
                since: 50,
                until: None,
                repo: Some("acme/shop"),
                exclude: None,
            },
        );
        assert_eq!(areas.len(), 1);
        assert_eq!(
            areas["billing"],
            AreaHistory {
                reverts: 1,
                incidents: 1,
                changes: vec!["argo:1".into(), "pd:1".into()],
                error_spikes: 1,
                last_at: Some(120),
            }
        );
        // Added to what the outcomes gave.
        let mut areas: AreaHistories = [(
            "billing".to_string(),
            AreaHistory {
                incidents: 1,
                changes: vec!["acme/shop#1".into()],
                last_at: Some(100),
                ..AreaHistory::default()
            },
        )]
        .into();
        extend_history(&mut areas, &all, &wanted, &HistoryScope::default());
        let b = &areas["billing"];
        assert_eq!((b.incidents, b.reverts, b.error_spikes), (3, 1, 2));
        assert_eq!(b.changes, ["acme/shop#1", "argo:1", "pd:0", "pd:1"]);
        assert_eq!(b.last_at, Some(150));
    }
}
