//! Builds a change's provenance record (`onus_core::provenance`) from the
//! facts Onus keeps about it: the submission, classification and judgment
//! from while it was open, the pull and outcome records, the token, the door
//! log and escalation requests, and the escalations Onus Cloud decided with
//! the door's inspection of the token. Every field is computed here, the
//! approver included, so a host only stores and shows the result.
//!
//! Facts from while the change was open (intent, report, token, evidence,
//! escalations, lane reasons, judge, doors) come from the fresh inputs when
//! they are given, and otherwise from an earlier record (`from`). Facts from
//! after it closed (stage, merge commit, approval, rollout) are recomputed on
//! every build from the records, production signals included: the rollout
//! status, its stages and the signals tied to the change.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use onus_core::Lane;
use onus_core::provenance::{
    Agent, Approval, ApprovedBy, CloudState, Commits, DoorEntry, Doors, Escalation,
    EscalationDecision, Evidence, Gap, Intent, IntentSource, Judge, JudgeStep, LaneFacts,
    LaneReason, Moment, PlanFacts, ProvenanceRecord, ReportFacts, Rollout, RolloutEvent,
    RolloutStatus, RowApproval, SCHEMA, SignalRef, Stage, StageEvent, Token, TokenInspection,
    TokenSource, TrackRecord,
};
use onus_core::signal::{ProductionSignal, SignalKind};
use onus_doors::audit::{AuditLog, Entry};
use onus_doors::escalation::Request;
use sha2::{Digest, Sha256};

use crate::classify::Classification;
use crate::judge::{Judgment, StepStatus, Verdict};
use crate::outcomes::{self, Outcome};
use crate::pulls::{self, PullRecord};
use crate::rollout::RolloutPlan;
use crate::submission::{AgentSetup, Submission};

/// What a record is built from. Everything but the change is optional.
#[derive(Debug)]
pub struct Inputs<'a> {
    pub change: &'a str,
    /// Unix seconds.
    pub now: u64,
    /// An earlier record of this change.
    pub from: Option<ProvenanceRecord>,
    pub submission: Option<&'a Submission>,
    /// Where the submission's intent came from.
    pub intent_source: IntentSource,
    /// How the agent was recognised (`pulls::detect_agent_source`).
    pub agent_source: Option<&'a str>,
    pub classification: Option<&'a Classification>,
    pub judgment: Option<&'a Judgment>,
    /// The lane policy's hash (`judge::judge_id`).
    pub policy: Option<String>,
    pub pulls: &'a [PullRecord],
    pub outcomes: &'a [Outcome],
    /// Production signals (`signals.jsonl`); those of this change are used.
    pub signals: &'a [ProductionSignal],
    /// A token the caller verified ([`TokenSource::Token`]).
    pub token: Option<Token>,
    /// The door log's entries, and whether its hash chain verified.
    pub audit: Option<(Vec<Entry>, bool)>,
    pub requests: &'a [Request],
    /// Escalations as Onus Cloud keeps them, with their decisions. Given and
    /// empty, Onus Cloud has none for the change.
    pub decisions: Option<&'a [EscalationDecision]>,
    /// What of Onus Cloud's escalations could not be read, one note each.
    /// While any is unread, escalations are a gap.
    pub unread: &'a [String],
    /// The door's inspection of the task token ([`TokenSource::Inspection`]).
    pub inspection: Option<TokenInspection>,
    /// Why an inspection that was given could not be read: the token gap
    /// says so.
    pub inspection_unread: Option<&'a str>,
    /// The change's branch and commits (base excluded): an escalation in
    /// Onus Cloud that names no pull request is this change's when its
    /// `ref` or `commit` names one of them.
    pub branch: Option<&'a str>,
    pub commits: &'a [String],
    /// The task whose door decisions to include (default: the token's).
    pub task: Option<&'a str>,
}

impl<'a> Inputs<'a> {
    /// Inputs that know only the change.
    pub fn new(change: &'a str, now: u64) -> Inputs<'a> {
        Inputs {
            change,
            now,
            from: None,
            submission: None,
            intent_source: IntentSource::Submission,
            agent_source: None,
            classification: None,
            judgment: None,
            policy: None,
            pulls: &[],
            outcomes: &[],
            signals: &[],
            token: None,
            audit: None,
            requests: &[],
            decisions: None,
            unread: &[],
            inspection: None,
            inspection_unread: None,
            branch: None,
            commits: &[],
            task: None,
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn verdict_name(v: Verdict) -> &'static str {
    match v {
        Verdict::Approve => "approve",
        Verdict::Reject => "reject",
        Verdict::Escalate => "escalate",
    }
}

fn status_name(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Passed => "passed",
        StepStatus::Failed => "failed",
        StepStatus::Skipped => "skipped",
        StepStatus::Concern => "concern",
    }
}

/// The first 8 bytes of a token's sha256, as hex: enough to tell tokens
/// apart, and all a submission or a record keeps of one.
pub fn fingerprint(token: &str) -> String {
    sha256_hex(token.trim().as_bytes())[..16].to_string()
}

/// Reads a door log. A log whose hash chain does not verify is still read,
/// line by line, and reported as not intact.
pub fn read_audit(path: &Path) -> Result<(Vec<Entry>, bool)> {
    if let Ok(entries) = AuditLog::new(path).verify() {
        return Ok((entries, true));
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the audit log {}", path.display()))?;
    let entries = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    Ok((entries, false))
}

/// The token as the door log saw it: the `mint` entry of `task`, widened by
/// the scopes of the requests granted to it.
fn token_from_audit(entries: &[Entry], task: &str) -> Option<Token> {
    let mint = entries
        .iter()
        .rev()
        .find(|e| e.action == "mint" && e.subject == task)?;
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut rights = strings(&mint.details["rights"]);
    for e in entries.iter().filter(|e| {
        e.decision == "granted"
            && matches!(e.action.as_str(), "grant" | "escalate")
            && e.details["task"].as_str() == Some(task)
    }) {
        for r in strings(&e.details["scopes"]) {
            if !rights.contains(&r) {
                rights.push(r);
            }
        }
    }
    Some(Token {
        task: task.to_string(),
        rights,
        attenuations: vec![],
        expires: mint.details["expires"].as_u64(),
        fingerprint: None,
        source: TokenSource::Audit,
    })
}

/// The last decision on an escalation request in the door log (the same
/// rule `onus ui` shows).
fn decision_of<'e>(entries: &'e [Entry], id: &str) -> Option<&'e Entry> {
    entries.iter().rev().find(|e| {
        e.subject == id && matches!(e.action.as_str(), "escalate" | "grant" | "deny" | "answer")
    })
}

fn escalation(id: &str, requests: &[Request], entries: &[Entry], listed: bool) -> Escalation {
    let req = requests.iter().find(|r| r.id == id);
    let decided = decision_of(entries, id);
    let decision = match decided {
        Some(e) => e.decision.clone(),
        // A submission lists only escalations verified as granted.
        None if listed => "granted".into(),
        None => "open".into(),
    };
    let by = decided.and_then(|e| match e.action.as_str() {
        "escalate" if e.decision == "granted" => Some("policy".to_string()),
        "escalate" => None,
        _ => Some(e.actor.clone()),
    });
    Escalation {
        id: id.to_string(),
        kind: req.and_then(|r| {
            serde_json::to_value(r.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
        }),
        scopes: req
            .map(|r| r.scopes.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default(),
        best_grade: req.and_then(Request::best_grade),
        decision,
        by,
        at: decided.map(|e| e.at),
        reason: decided.map(|e| e.reason.clone()).filter(|r| !r.is_empty()),
        cloud_id: None,
    }
}

/// Unix seconds of a time Onus Cloud gave.
fn seconds(m: &Moment) -> Option<u64> {
    match m {
        Moment::Seconds(n) => Some(*n),
        Moment::Date(d) => onus_doors::token::parse_rfc3339(d),
    }
}

const NOT_A_TOKEN: &str =
    "give the door's inspection of the task token ({task, rights, expires}), never the token";

/// A raw task token given where only its inspection belongs. It is always
/// refused, even where other unreadable input is only warned about.
#[derive(Debug)]
pub struct HoldsToken(String);

impl std::fmt::Display for HoldsToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {NOT_A_TOKEN}", self.0)
    }
}

impl std::error::Error for HoldsToken {}

/// Escalations as Onus Cloud keeps them, as far as they could be read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CloudEscalations {
    pub list: Vec<EscalationDecision>,
    /// One note for each escalation that could not be read, naming it.
    pub unread: Vec<String>,
}

/// Reads escalations as Onus Cloud keeps them: a JSON array
/// (`schemas/escalation-decisions.schema.json`), or the API's
/// `{"escalations": [...]}`, whose views put the decision under
/// `decision: {by: {id, name}, at, answer}`. An escalation that cannot be
/// read is left out with a note, so one bad entry never stops a run. A file
/// that is not such a list is an error, and one that holds a token is
/// [`HoldsToken`].
pub fn parse_decisions(text: &str) -> Result<CloudEscalations> {
    let mut v: serde_json::Value = serde_json::from_str(text)
        .context("not JSON: give a JSON array of escalations as Onus Cloud keeps them")?;
    if let Some(list) = v.get_mut("escalations") {
        v = list.take();
    }
    let Some(items) = v.as_array_mut() else {
        bail!("not a JSON array of escalations");
    };
    if let Some((n, e)) = items
        .iter()
        .enumerate()
        .find(|(_, e)| e["token"].is_string())
    {
        return Err(HoldsToken(format!("escalation {} holds a token", name_of(e, n))).into());
    }
    let mut read = CloudEscalations::default();
    for (n, e) in items.iter_mut().enumerate() {
        let name = name_of(e, n);
        let Some(o) = e.as_object_mut() else {
            read.unread
                .push(format!("escalation {name}: not a JSON object"));
            continue;
        };
        // An API view: the decision is nested, the request's input is not.
        if let Some(d) = o.remove("decision").filter(|d| d.is_object()) {
            for (key, value) in [
                ("decidedBy", &d["by"]["id"]),
                ("decidedByName", &d["by"]["name"]),
                ("decidedAt", &d["at"]),
                ("answer", &d["answer"]),
            ] {
                if !o.contains_key(key) && !value.is_null() {
                    o.insert(key.into(), value.clone());
                }
            }
        }
        // A stored row: what was sent is under `input`.
        if let Some(input) = o.remove("input").filter(|i| i.is_object()) {
            for key in ["scopes", "reason"] {
                if !o.contains_key(key) && !input[key].is_null() {
                    o.insert(key.into(), input[key].clone());
                }
            }
        }
        if o.get("scopes").is_some_and(|s| s.is_null()) {
            o.remove("scopes");
        }
        let d: EscalationDecision = match serde_json::from_value(e.clone()) {
            Ok(d) => d,
            Err(err) => {
                let base = serde_json::json!({ "id": "x", "state": "waiting" });
                read.unread.push(format!(
                    "escalation {name}: {}",
                    why_unread::<EscalationDecision>(e, base, &err)
                ));
                continue;
            }
        };
        if [&d.decided_at, &d.expires_at]
            .into_iter()
            .flatten()
            .any(|at| seconds(at).is_none())
        {
            read.unread.push(format!(
                "escalation {name}: a time is neither Unix seconds nor an RFC 3339 date in UTC"
            ));
            continue;
        }
        read.list.push(d);
    }
    Ok(read)
}

/// How an entry is named in a note: its id when that looks like one, else
/// its place in the list. Input is never echoed beyond that.
fn name_of(e: &serde_json::Value, n: usize) -> String {
    match e["id"].as_str() {
        Some(id)
            if (1..=64).contains(&id.len())
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) =>
        {
            id.to_string()
        }
        _ => format!("#{}", n + 1),
    }
}

/// Why `v` could not be read as a `T`, naming the field and what it should
/// be, never the value: a value may be anything, a token included. `base`
/// holds what `T` needs, so each field can be tried on its own.
fn why_unread<T: serde::de::DeserializeOwned>(
    v: &serde_json::Value,
    base: serde_json::Value,
    err: &serde_json::Error,
) -> String {
    // serde names what it expected after the value it found, and the value
    // may itself hold ", expected ".
    let expected = |e: &serde_json::Error| {
        let text = e.to_string();
        if text.starts_with("missing field") {
            return Some(text);
        }
        let (_, rest) = text.rsplit_once(", expected ")?;
        Some(format!("expected {rest}"))
    };
    let Some(o) = v.as_object() else {
        return "not a JSON object".into();
    };
    for (key, value) in o {
        let mut one = base.clone();
        one[key.as_str()] = value.clone();
        if let Err(e) = serde_json::from_value::<T>(one) {
            return match expected(&e) {
                Some(why) => format!("`{key}` cannot be read ({why})"),
                None => format!("`{key}` cannot be read"),
            };
        }
    }
    expected(err)
        .filter(|why| why.starts_with("missing field"))
        .unwrap_or_else(|| "it cannot be read".into())
}

/// Whether `text` is a raw token rather than JSON: a Biscuit is one run of
/// base64url, alone or as a JSON string.
fn is_raw_token(text: &str) -> bool {
    let t = text.trim();
    let t = serde_json::from_str::<String>(t).unwrap_or_else(|_| t.to_string());
    t.len() >= 16
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=' | '+' | '/'))
}

/// Reads the door's inspection of a task token (`{task, rights, expires}`,
/// as `onus token inspect` prints it). A token itself is [`HoldsToken`].
pub fn parse_inspection(text: &str) -> Result<TokenInspection> {
    if is_raw_token(text) {
        return Err(HoldsToken("this is a token".into()).into());
    }
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| anyhow::anyhow!("not JSON: {NOT_A_TOKEN}"))?;
    serde_json::from_value(v.clone()).map_err(|e| {
        let why = why_unread::<TokenInspection>(&v, serde_json::json!({ "task": "x" }), &e);
        anyhow::anyhow!("not an inspection ({why}): {NOT_A_TOKEN}")
    })
}

/// Whether a decision from Onus Cloud is about the escalation `e`.
fn is_about(d: &EscalationDecision, e: &Escalation) -> bool {
    let id = e.id.as_str();
    d.id == id
        || d.client_request_id.as_deref() == Some(id)
        || d.request_id.as_deref() == Some(id)
        || e.cloud_id.as_deref() == Some(d.id.as_str())
}

/// The task an escalation in Onus Cloud was filed for.
fn task_of(d: &EscalationDecision) -> Option<&str> {
    d.task
        .as_deref()
        .or_else(|| d.token.as_ref().map(|t| t.task.as_str()))
}

/// Whether a decision, as the record names it, is final.
fn is_final(decision: &str) -> bool {
    !matches!(decision, "open" | "needs-person")
}

/// The decision Onus Cloud keeps on an escalation, as the record names it.
/// A final decision made here (the submission's grant, the door log's)
/// stands against one Onus Cloud has not made yet, and Onus Cloud's fills
/// what it left out. When both are final and differ, Onus Cloud's is
/// recorded, since it decided, and the disagreement is returned as a note.
/// A stored row still waiting past its expiry at `now` has expired, as
/// Onus Cloud shows it.
fn decide_from_cloud(e: &mut Escalation, d: &EscalationDecision, now: u64) -> Option<String> {
    let expired = d
        .expires_at
        .as_ref()
        .and_then(seconds)
        .is_some_and(|t| t < now);
    let decision = match d.state {
        CloudState::Waiting if expired => "expired",
        CloudState::Waiting => "needs-person",
        CloudState::Assessing => "open",
        CloudState::Granted => "granted",
        CloudState::Denied => "denied",
        CloudState::Resolved => "answered",
        CloudState::Withdrawn => "withdrawn",
        CloudState::Expired => "expired",
        CloudState::Failed => "failed",
    };
    let by = d.decided_by.as_deref().map(|by| {
        if by == "system:policy" {
            "policy".to_string()
        } else {
            d.decided_by_name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| by.to_string())
        }
    });
    let at = d.decided_at.as_ref().and_then(seconds);
    // The decider's reason, as the door log keeps it; never the agent's.
    let reason = d.answer.clone().filter(|r| !r.is_empty());
    let mut note = None;
    if is_final(&e.decision) && is_final(decision) && e.decision != decision {
        note = Some(format!(
            "escalation {} is {decision} in Onus Cloud but was {} by the submission or the door log; Onus Cloud's decision is recorded",
            e.id, e.decision
        ));
        e.decision = decision.to_string();
        e.by = by;
        e.at = at;
        e.reason = reason;
    } else if !is_final(&e.decision) && (is_final(decision) || e.decision == "open") {
        // Onus Cloud's decision, with what it left out kept from here.
        e.decision = decision.to_string();
        e.by = by.or(e.by.take());
        e.at = at.or(e.at);
        e.reason = reason.or(e.reason.take());
    } else if e.decision == decision {
        e.by = e.by.take().or(by);
        e.at = e.at.or(at);
        e.reason = e.reason.take().or(reason);
    }
    e.cloud_id = Some(d.id.clone());
    // The facts of a request kept here are the binary's own; Onus Cloud's
    // fill only what is missing.
    if e.kind.is_none() {
        e.kind = d.kind.clone();
    }
    if e.scopes.is_empty() {
        e.scopes = d.scopes.clone();
    }
    if e.best_grade.is_none() {
        e.best_grade = d.best_grade;
    }
    note
}

/// The token as the door inspected it, widened by the scopes Onus Cloud
/// granted to its task.
fn token_from_inspection(t: TokenInspection, decisions: &[EscalationDecision]) -> Token {
    let mut rights = t.rights;
    for d in decisions
        .iter()
        .filter(|d| d.state == CloudState::Granted && task_of(d) == Some(t.task.as_str()))
    {
        for r in &d.scopes {
            if !rights.contains(r) {
                rights.push(r.clone());
            }
        }
    }
    Token {
        task: t.task,
        rights,
        attenuations: t.attenuations,
        // `onus token inspect` and the door give 0 for no expiry.
        expires: t.expires.filter(|&e| e > 0),
        fingerprint: None,
        source: TokenSource::Inspection,
    }
}

fn agent_of(setup: &AgentSetup, detected_from: &str, outcomes: &[Outcome]) -> Agent {
    let key = setup.key();
    let r = outcomes::record_for(outcomes, &key);
    Agent {
        tool: setup.tool.clone(),
        model: setup.model.clone(),
        config: setup.config.clone(),
        team: setup.team.clone(),
        key,
        detected_from: detected_from.to_string(),
        record: Some(TrackRecord {
            merged: r.changes,
            recent_incidents: r.recent_incidents,
            recent_rollbacks: r.recent_rollbacks,
        }),
    }
}

fn plan_facts(p: &RolloutPlan) -> PlanFacts {
    PlanFacts {
        strategy: p.strategy.as_str().into(),
        steps: p.steps.clone(),
        bake_minutes: p.bake_minutes,
        guards: p.guards.clone(),
        automatic_rollback: p.automatic_rollback,
        flag: p.flag.clone(),
        reasons: p.reasons.clone(),
    }
}

/// Whether a stage counts toward the rollout status: production only.
fn in_production(e: &StageEvent) -> bool {
    e.environment
        .as_deref()
        .is_none_or(|env| env == "production")
}

/// Where the change is in production, and the share of traffic, from its
/// stage, its latest outcome and its production stages (first rule that
/// applies).
fn status_of(
    stage: Stage,
    last: Option<&Outcome>,
    stages: &[StageEvent],
) -> (RolloutStatus, Option<u8>) {
    let counted: Vec<&StageEvent> = stages.iter().filter(|e| in_production(e)).collect();
    let last_stage = counted.last();
    let percent = last_stage.map(|e| e.percent);
    match stage {
        Stage::Open => return (RolloutStatus::Pending, None),
        Stage::Closed => return (RolloutStatus::NotShipped, None),
        Stage::Merged => {}
    }
    match last.map(|o| o.result.as_str()) {
        Some("rolled-back") => return (RolloutStatus::RolledBack, percent),
        Some("reverted") => return (RolloutStatus::Reverted, percent),
        _ => {}
    }
    let Some(e) = last_stage else {
        return (RolloutStatus::AwaitingDeploy, None);
    };
    match e.decision.as_deref() {
        Some("rollback") => (RolloutStatus::RolledBack, percent),
        Some("hold") => (RolloutStatus::Holding, percent),
        Some("complete") | None if e.percent >= 100 => (RolloutStatus::Complete, Some(100)),
        _ => (RolloutStatus::InProgress, percent),
    }
}

/// The record of one change. It is sealed: `digest` is set.
pub fn build(i: Inputs) -> ProvenanceRecord {
    let from = i.from.as_ref();
    let sub = i.submission;
    let pull = pulls::latest(i.pulls, i.change);
    let lines: Vec<&Outcome> = i.outcomes.iter().filter(|o| o.change == i.change).collect();

    let commits = Commits {
        base: pull
            .map(|p| p.base.clone())
            .or_else(|| sub.map(|s| s.base.clone()))
            .or_else(|| from.map(|f| f.commits.base.clone()))
            .unwrap_or_default(),
        head: pull
            .map(|p| p.head.clone())
            .or_else(|| sub.map(|s| s.head.clone()))
            .or_else(|| from.map(|f| f.commits.head.clone()))
            .unwrap_or_default(),
        merge: None,
    };

    // Facts from while the change was open.
    let intent = match sub {
        Some(s) => s.intent.as_ref().map(|text| Intent {
            source: i.intent_source,
            sha256: sha256_hex(text.as_bytes()),
            check: s.report.intent_check.clone(),
        }),
        None => from.and_then(|f| f.intent.clone()),
    };
    let report = match sub {
        Some(s) => Some(ReportFacts {
            summary: s.report.summary.clone(),
            needs_person: s
                .report
                .changes
                .iter()
                .filter(|r| r.hints.needs_person)
                .map(|r| r.id.clone())
                .collect(),
            changed_files: s.changed_files.len() as u32,
        }),
        None => from.and_then(|f| f.report.clone()),
    };
    // Onus Cloud's escalations for this change. For a pull request, a list
    // that also holds other repositories' or pull requests' leaves those
    // out; a local change (a head commit) names neither.
    let pull_ref = i
        .change
        .rsplit_once('#')
        .and_then(|(repo, n)| Some((repo, n.parse::<u64>().ok()?)));
    let given = i.decisions.unwrap_or(&[]);
    let decisions: Vec<&EscalationDecision> = given
        .iter()
        .filter(|d| match pull_ref {
            Some((repo, number)) => {
                d.repository
                    .as_deref()
                    .is_none_or(|r| r.eq_ignore_ascii_case(repo))
                    && d.pull.is_none_or(|p| p == number)
            }
            None => true,
        })
        .collect();
    // The change's commits: those given, its head and every head classified.
    let shas: BTreeSet<String> = i
        .commits
        .iter()
        .map(String::as_str)
        .chain([commits.head.as_str()])
        .chain(sub.map(|s| s.head.as_str()))
        .chain(
            i.pulls
                .iter()
                .filter(|p| p.change == i.change)
                .map(|p| p.head.as_str()),
        )
        .filter(|c| !c.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    // Whether an escalation names this change: it is on this pull request,
    // the submission lists it, the earlier record kept it, or it was filed
    // at one of the change's commits or on its branch. A commit elsewhere
    // outweighs the branch's name, which may be used again.
    let listed: Vec<&str> = sub
        .map(|s| s.escalations.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let kept: &[Escalation] = from.map_or(&[], |f| &f.escalations);
    let names_change = |d: &EscalationDecision| {
        let ids = [
            Some(d.id.as_str()),
            d.client_request_id.as_deref(),
            d.request_id.as_deref(),
        ];
        if ids.into_iter().flatten().any(|id| listed.contains(&id))
            || kept.iter().any(|e| is_about(d, e))
        {
            return true;
        }
        let ours = |c: &str| shas.contains(&c.to_ascii_lowercase());
        let at_ours = d.reference.as_deref().is_some_and(ours);
        let on_branch = i.branch.is_some_and(|b| {
            d.reference
                .as_deref()
                .is_some_and(|r| r == b || r.strip_prefix("refs/heads/") == Some(b))
        });
        let pull_head = pull_ref.is_some_and(|(_, n)| {
            d.reference.as_deref() == Some(format!("refs/pull/{n}/head").as_str())
        });
        pull_ref.is_some() && d.pull.is_some()
            || pull_head
            || at_ours
            || d.commit.as_deref().map_or(on_branch, ours)
    };
    // The task: given, or the token's, the submission's, the inspection's or
    // the earlier record's, else the one task the escalations naming this
    // change name. One that does not name the change never says its task.
    let task: Option<String> = i
        .task
        .map(str::to_string)
        .or_else(|| i.token.as_ref().map(|t| t.task.clone()))
        .or_else(|| sub.and_then(|s| s.scope.as_ref()).map(|s| s.task.clone()))
        .or_else(|| i.inspection.as_ref().map(|t| t.task.clone()))
        .or_else(|| from.and_then(|f| f.token.as_ref()).map(|t| t.task.clone()))
        .or_else(|| {
            let tasks: BTreeSet<&str> = decisions
                .iter()
                .filter(|d| pull_ref.is_none() || names_change(d))
                .filter_map(|d| task_of(d))
                .collect();
            (tasks.len() == 1).then(|| tasks.into_iter().next().unwrap_or_default().to_string())
        });
    // For a pull request, an escalation that names it is kept whatever its
    // task, and one that does not only for the change's task. For a local
    // change, those of its task, or all of them when it is not known.
    let decisions: Vec<EscalationDecision> = decisions
        .into_iter()
        .filter(|d| match pull_ref {
            Some(_) => names_change(d) || task.is_some() && task_of(d) == task.as_deref(),
            None => task.is_none() || task_of(d).is_none_or(|t| Some(t) == task.as_deref()),
        })
        .cloned()
        .collect();
    let decisions = decisions.as_slice();
    let entries: &[Entry] = i.audit.as_ref().map_or(&[], |(e, _)| e);
    let token = i
        .token
        .clone()
        .or_else(|| {
            sub.and_then(|s| s.scope.as_ref()).map(|s| Token {
                task: s.task.clone(),
                rights: s.rights.clone(),
                attenuations: s.attenuations.clone(),
                expires: s.expires,
                fingerprint: s.fingerprint.clone(),
                source: TokenSource::Submission,
            })
        })
        .or_else(|| task.as_deref().and_then(|t| token_from_audit(entries, t)))
        // A token verified earlier outranks the door's word on a refresh.
        .or_else(|| {
            from.and_then(|f| f.token.clone())
                .filter(|t| t.source != TokenSource::Inspection)
        })
        .or_else(|| {
            // The door's inspection: given, else the one Onus Cloud kept with
            // an escalation of the task.
            i.inspection
                .clone()
                .filter(|t| task.as_deref().is_none_or(|task| task == t.task))
                .or_else(|| {
                    let task = task.as_deref()?;
                    decisions
                        .iter()
                        .filter(|d| task_of(d) == Some(task))
                        .find_map(|d| d.token.clone())
                })
                .map(|t| token_from_inspection(t, decisions))
        })
        .or_else(|| from.and_then(|f| f.token.clone()));
    let evidence = match sub {
        Some(s) => s
            .evidence
            .iter()
            .map(|e| Evidence {
                command: e.command.clone(),
                commit: e.commit.clone(),
                image: e.image.clone(),
                exit_code: e.exit_code,
                manifest: e.manifest.clone(),
                at_head: e.commit == commits.head,
            })
            .collect(),
        None => from.map(|f| f.evidence.clone()).unwrap_or_default(),
    };
    let local_escalations = sub.is_some() || !i.requests.is_empty() || i.audit.is_some();
    let fresh_escalations = local_escalations || i.decisions.is_some();
    let mut disagreements: Vec<String> = Vec::new();
    let escalations: Vec<Escalation> = if fresh_escalations {
        let mut ids: Vec<&str> = Vec::new();
        let for_task = i
            .requests
            .iter()
            .filter(|r| task.as_deref() == Some(r.task.as_str()))
            .map(|r| r.id.as_str());
        for id in listed.iter().copied().chain(for_task) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        // With only Onus Cloud's list fresh, it updates the escalations kept
        // earlier, which may hold some only the door knew.
        let mut out: Vec<Escalation> = if local_escalations {
            ids.into_iter()
                .map(|id| escalation(id, i.requests, entries, listed.contains(&id)))
                .collect()
        } else {
            from.map(|f| f.escalations.clone()).unwrap_or_default()
        };
        // Onus Cloud's decisions: on the request they are about, else as
        // escalations of their own.
        for d in decisions {
            let at = match out.iter().position(|e| is_about(d, e)) {
                Some(n) => n,
                None => {
                    let id = d
                        .client_request_id
                        .as_deref()
                        .or(d.request_id.as_deref())
                        .unwrap_or(&d.id);
                    out.push(escalation(id, i.requests, entries, false));
                    out.len() - 1
                }
            };
            disagreements.extend(decide_from_cloud(&mut out[at], d, i.now));
        }
        out
    } else {
        from.map(|f| f.escalations.clone()).unwrap_or_default()
    };
    let judge = match i.judgment {
        Some(j) => Some(Judge {
            id: j.judge.clone(),
            verdict: verdict_name(j.verdict).into(),
            steps: j
                .steps
                .iter()
                .map(|s| JudgeStep {
                    name: s.name.clone(),
                    status: status_name(s.status).into(),
                    details: s.details.clone(),
                })
                .collect(),
            reasons: j.reasons.clone(),
        }),
        None => from.and_then(|f| f.judge.clone()),
    };
    let doors = match &i.audit {
        Some((all, intact)) => {
            let ids: BTreeSet<&str> = escalations.iter().map(|e| e.id.as_str()).collect();
            let mut picked: Vec<&Entry> = all
                .iter()
                .filter(|e| {
                    task.as_deref()
                        .is_some_and(|t| e.actor == t || (e.action == "mint" && e.subject == t))
                        || (matches!(e.action.as_str(), "grant" | "deny")
                            && ids.contains(e.subject.as_str()))
                })
                .collect();
            picked.sort_by_key(|e| e.seq);
            Some(Doors {
                intact: *intact,
                entries: picked
                    .into_iter()
                    .map(|e| DoorEntry {
                        seq: e.seq,
                        at: e.at,
                        actor: e.actor.clone(),
                        action: e.action.clone(),
                        subject: e.subject.clone(),
                        decision: e.decision.clone(),
                        reason: e.reason.clone(),
                        hash: e.hash.clone(),
                    })
                    .collect(),
            })
        }
        None => from.and_then(|f| f.doors.clone()),
    };

    // The lane: its reasons from while it was open, the final lane from the
    // latest record.
    let final_lane: Option<Lane> = pull
        .map(|p| p.lane)
        .or_else(|| i.judgment.map(|j| j.lane))
        .or_else(|| i.classification.map(|c| c.lane))
        .or_else(|| lines.last().map(|o| o.lane))
        .or_else(|| from.and_then(|f| f.lane.as_ref()).map(|l| l.lane));
    let lane = final_lane.map(|l| LaneFacts {
        lane: l,
        applied: match i.classification {
            Some(c) => c
                .applied
                .iter()
                .map(|a| LaneReason {
                    lane: a.lane,
                    source: a.source.clone(),
                    reason: a.reason.clone(),
                    rows: a.rows.clone(),
                })
                .collect(),
            None => from
                .and_then(|f| f.lane.as_ref())
                .map(|l| l.applied.clone())
                .unwrap_or_default(),
        },
        policy: i.policy.clone().or_else(|| {
            from.and_then(|f| f.lane.as_ref())
                .and_then(|l| l.policy.clone())
        }),
    });

    let agent = match (pull, sub, from, lines.last()) {
        (Some(p), _, _, _) => {
            let kept = from
                .map(|f| &f.agent)
                .filter(|a| a.key == p.agent.key())
                .map(|a| a.detected_from.as_str());
            agent_of(
                &p.agent,
                i.agent_source.or(kept).unwrap_or("record"),
                i.outcomes,
            )
        }
        (None, Some(s), _, _) => {
            agent_of(&s.agent, i.agent_source.unwrap_or("submission"), i.outcomes)
        }
        (None, None, Some(f), _) => {
            let setup = AgentSetup {
                tool: f.agent.tool.clone(),
                model: f.agent.model.clone(),
                config: f.agent.config.clone(),
                team: f.agent.team.clone(),
            };
            agent_of(&setup, &f.agent.detected_from, i.outcomes)
        }
        (None, None, None, Some(o)) => {
            let mut parts = o.agent.splitn(3, '/');
            let setup = AgentSetup {
                tool: parts.next().unwrap_or("").into(),
                model: parts.next().unwrap_or("").into(),
                config: parts.next().unwrap_or("").into(),
                team: String::new(),
            };
            agent_of(&setup, "outcome", i.outcomes)
        }
        (None, None, None, None) => agent_of(
            &AgentSetup {
                tool: "unknown".into(),
                ..Default::default()
            },
            "unknown",
            i.outcomes,
        ),
    };

    // Facts from after it closed: the stage and rollout from the outcome
    // lines, in order.
    let landed = |o: &&&Outcome| {
        matches!(
            o.result.as_str(),
            "merged" | "reverted" | "rolled-back" | "incident"
        )
    };
    let merged_line = lines
        .iter()
        .find(|o| o.result == "merged")
        .or_else(|| lines.iter().find(landed))
        .copied();
    let closed_line = lines.iter().find(|o| o.result == "closed").copied();
    let (stage, first) = match (merged_line, closed_line) {
        (Some(m), _) => (Stage::Merged, Some(m)),
        (None, Some(c)) => (Stage::Closed, Some(c)),
        (None, None) => (Stage::Open, None),
    };
    let last = lines.last();
    // Production: the change's deploy and rollout stages, and its other
    // signals, in time order.
    let mut mine: Vec<&ProductionSignal> = i
        .signals
        .iter()
        .filter(|s| s.change.as_deref() == Some(i.change))
        .collect();
    mine.sort_by_key(|s| s.at.unwrap_or(0));
    let stages: Vec<StageEvent> = mine
        .iter()
        .filter(|s| matches!(s.kind, SignalKind::Deploy | SignalKind::Rollout))
        .map(|s| StageEvent {
            at: s.at.unwrap_or(0),
            kind: s.kind.as_str().into(),
            // A deploy that names no share reached all of the environment.
            percent: s.rollout.as_ref().map_or(100, |r| r.percent),
            decision: s.rollout.as_ref().and_then(|r| r.decision.clone()),
            environment: s.environment.clone(),
            deploy: s.deploy.clone(),
            source: s.source.clone(),
            note: s.note.clone(),
        })
        .collect();
    let signals: Vec<SignalRef> = mine
        .iter()
        .filter(|s| {
            !matches!(
                s.kind,
                SignalKind::Deploy | SignalKind::Rollout | SignalKind::Trace
            )
        })
        .map(|s| SignalRef {
            id: s.id.clone(),
            kind: s.kind.as_str().into(),
            at: s.at.unwrap_or(0),
            source: s.source.clone(),
            severity: s.severity.map(|v| v.as_str().to_string()),
            note: s.note.clone(),
        })
        .collect();
    let (status, percent) = status_of(stage, last.copied(), &stages);
    let plan = pull
        .and_then(|p| p.rollout.as_ref())
        .map(plan_facts)
        .or_else(|| from.and_then(|f| f.rollout.plan.clone()));
    let rollout = Rollout {
        result: last.map_or_else(|| "open".to_string(), |o| o.result.clone()),
        commit: first.and_then(|o| o.commit.clone()),
        at: first.map(|o| o.at),
        audited: last.is_some_and(|o| o.audit.is_some() || o.audited),
        missed: last.is_some_and(|o| o.audit.as_ref().map_or(o.missed, |a| a.missed)),
        history: lines
            .iter()
            .map(|o| RolloutEvent {
                at: o.at,
                result: o.result.clone(),
                note: o.note.clone(),
                involved: o.involved.clone(),
            })
            .collect(),
        status: Some(status),
        percent,
        plan,
        stages,
        signals,
    };
    let commits = Commits {
        merge: (stage == Stage::Merged)
            .then(|| rollout.commit.clone())
            .flatten(),
        ..commits
    };

    // Who or what approved it.
    let rows: Vec<RowApproval> = match (pull, sub) {
        (Some(p), _) => p
            .approvals
            .iter()
            .map(|a| RowApproval {
                row: a.row.clone(),
                by: a.by.clone(),
            })
            .collect(),
        (None, Some(s)) => s
            .approvals
            .iter()
            .map(|a| RowApproval {
                row: a.row.clone(),
                by: a.by.clone(),
            })
            .collect(),
        (None, None) => from.map(|f| f.approval.rows.clone()).unwrap_or_default(),
    };
    let merged_by = merged_line.and_then(|o| o.merged_by.clone());
    let (reviewed, review): (&[String], _) = match (merged_line, pull) {
        (Some(o), _) if !o.reviewed_by.is_empty() => (&o.reviewed_by, o.approval),
        (_, Some(p)) => (&p.reviewed_by, p.approval),
        _ => (&[], None),
    };
    // No reviewers, so no rule to claim for them.
    let review = review.filter(|_| !reviewed.is_empty());
    let mut people: Vec<String> = Vec::new();
    for who in rows
        .iter()
        .map(|r| &r.by)
        .chain(reviewed)
        .chain(merged_by.iter())
    {
        if !people.contains(who) {
            people.push(who.clone());
        }
    }
    let verdict: Option<String> = pull
        .and_then(|p| p.verdict.clone())
        .or_else(|| judge.as_ref().map(|j| j.verdict.clone()))
        .or_else(|| merged_line.and_then(|o| o.verdict.clone()));
    let by = match stage {
        Stage::Open => ApprovedBy::Pending,
        Stage::Closed => ApprovedBy::None,
        Stage::Merged => match final_lane {
            Some(Lane::AutoMerge) => ApprovedBy::Policy,
            Some(Lane::Judge) if verdict.as_deref() == Some("approve") => ApprovedBy::Judge,
            Some(Lane::Blocked) => ApprovedBy::Override,
            _ => ApprovedBy::Person,
        },
    };
    let approval = Approval {
        by,
        people,
        rows,
        merged_by,
        review,
        judge: (by == ApprovedBy::Judge)
            .then(|| {
                judge
                    .as_ref()
                    .map(|j| j.id.clone())
                    .or_else(|| pull.and_then(|p| p.judge.clone()))
                    .or_else(|| merged_line.and_then(|o| o.judge.clone()))
            })
            .flatten(),
    };

    let mut gaps = Vec::new();
    let mut gap = |field: &str, reason: &str| {
        gaps.push(Gap {
            field: field.into(),
            reason: reason.into(),
        })
    };
    if intent.is_none() {
        gap(
            "intent",
            "no intent was stated: add an onus-intent block to the pull request body, or give onus submit --intent",
        );
    }
    if token.is_none() {
        match i.inspection_unread {
            Some(why) => gap(
                "token",
                &format!("the door's inspection of the task token could not be read: {why}"),
            ),
            None => gap(
                "token",
                "no task token reached this run: give --token/--key-public, --submission, --token-inspection, or --audit with --task",
            ),
        }
    }
    if evidence.is_empty() {
        gap(
            "evidence",
            "no test runs were handed in: give --evidence to onus ci pr or onus submit",
        );
    }
    // Onus Cloud's list, even an empty one, says what was asked there,
    // unless some of it could not be read or none of it is this change's.
    // A rebuild keeps the earlier record's gap.
    let mut doubts: Vec<String> = Vec::new();
    if !i.unread.is_empty() {
        doubts.push(format!(
            "some of Onus Cloud's escalations could not be read, so the list may be short: {}",
            i.unread.join("; ")
        ));
    }
    doubts.extend(disagreements);
    let escalations_gap: Option<String> = if !doubts.is_empty() {
        Some(doubts.join("; "))
    } else if !fresh_escalations && from.is_some() {
        from.and_then(|f| f.gaps.iter().find(|g| g.field == "escalations"))
            .map(|g| g.reason.clone())
    } else if !escalations.is_empty() || i.decisions.is_some_and(|d| d.is_empty()) {
        None
    } else if i.decisions.is_some() {
        Some("Onus Cloud's escalations were all for other changes or tasks: none is on record for this one".into())
    } else {
        Some("no escalation is on record: none was asked for, or none reached this run (give --submission, --escalations with Onus Cloud's decisions, or --escalations and --audit with --task)".into())
    };
    if let Some(reason) = &escalations_gap {
        gap("escalations", reason);
    }
    if judge.is_none() {
        gap(
            "judge",
            "the lane was not judge, or the judge was not run (--judge)",
        );
    }
    match &doors {
        None => gap(
            "doors",
            "no door log reached this run: give --audit with --task, or --submission-id in the repository that keeps it",
        ),
        Some(d) if !d.intact => gap(
            "doors",
            "the audit log's hash chain is broken: an entry was changed or removed",
        ),
        Some(_) => {}
    }
    if by == ApprovedBy::Person && approval.people.is_empty() {
        gap("approval", "who merged it was not recorded");
    }

    let mut record = ProvenanceRecord {
        schema: SCHEMA,
        change: i.change.to_string(),
        stage,
        generated_at: i.now,
        onus: env!("CARGO_PKG_VERSION").to_string(),
        commits,
        intent,
        report,
        agent,
        token,
        evidence,
        escalations,
        lane,
        judge,
        approval,
        rollout,
        doors,
        gaps,
        digest: String::new(),
    };
    seal(&mut record);
    record
}

/// The digest a record should carry: sha256 of its compact JSON with
/// `digest` empty.
fn digest_of(r: &ProvenanceRecord) -> String {
    let mut copy = r.clone();
    copy.digest = String::new();
    sha256_hex(serde_json::to_string(&copy).unwrap_or_default().as_bytes())
}

/// Sets the record's digest.
pub fn seal(r: &mut ProvenanceRecord) {
    r.digest = digest_of(r);
}

/// Whether the record is unchanged since it was sealed.
pub fn digest_ok(r: &ProvenanceRecord) -> bool {
    r.digest == digest_of(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::Applied;
    use crate::judge::Step;
    use crate::submission::{Approval as RowOk, ScopeUsed};
    use onus_core::{IntentCheck, ReportSummary, SemanticReport, StructureNotes, TextStats};
    use onus_doors::audit::Record;
    use onus_doors::escalation::{EscalationKind, Evidence as Proof};
    use onus_doors::runner::TestRun;

    const CHANGE: &str = "acme/shop#7";

    fn submission() -> Submission {
        Submission {
            schema: 1,
            base: "base1".into(),
            head: "head1".into(),
            intent: Some("summary: SMS on shipping".into()),
            report: SemanticReport {
                schema_version: 1,
                base: "base1".into(),
                head: "head1".into(),
                summary: ReportSummary {
                    meaning_changes: 1,
                    needs_attention: 0,
                    secrets: 0,
                    new_rule_violations: 0,
                    intent_mismatches: 0,
                },
                changes: vec![],
                intent_check: Some(IntentCheck {
                    stated: "SMS on shipping".into(),
                    touches: vec!["notifications".into()],
                    contracts: vec![],
                    externals: vec![],
                    mismatches: vec![],
                }),
                rule_violations: vec![],
                structure: StructureNotes::default(),
                text_stats: TextStats::default(),
                map_diagnostics: vec![],
                coverage: None,
                behavior: None,
            },
            changed_files: vec!["a.ts".into(), "b.ts".into()],
            evidence: vec![TestRun {
                commit: "head1".into(),
                image: "node:22".into(),
                setup: None,
                command: "npm test".into(),
                exit_code: 0,
                output_tail: "ok".into(),
                manifest: Some("m1".into()),
            }],
            scope: None,
            escalations: vec![],
            approvals: vec![],
            agent: AgentSetup {
                tool: "codex".into(),
                ..Default::default()
            },
        }
    }

    fn classification(lane: Lane) -> Classification {
        Classification {
            lane,
            applied: vec![Applied {
                lane,
                source: "policy".into(),
                reason: "a rule".into(),
                rows: vec![],
            }],
            learned: None,
            features: None,
            rollout: None,
        }
    }

    fn judgment(verdict: Verdict, lane: Lane) -> Judgment {
        Judgment {
            verdict,
            lane,
            steps: vec![Step {
                name: "evidence".into(),
                status: StepStatus::Passed,
                details: vec!["npm test passed".into()],
            }],
            reasons: vec![],
            judge: "j1".into(),
            rotation: None,
            held_out: None,
        }
    }

    fn pull(lane: Lane, verdict: Option<&str>) -> PullRecord {
        PullRecord {
            at: 1,
            change: CHANGE.into(),
            base: "base1".into(),
            head: "head1".into(),
            agent: AgentSetup {
                tool: "codex".into(),
                ..Default::default()
            },
            lane,
            verdict: verdict.map(str::to_string),
            judge: verdict.map(|_| "j1".to_string()),
            approvals: vec![],
            components: vec![],
            requested: vec![],
            reviewed_by: vec![],
            approval: None,
            audit_after_merge: None,
            features: None,
            caught: false,
            rollout: None,
        }
    }

    fn outcome(p: &PullRecord, result: &str, at: u64) -> Outcome {
        Outcome {
            at,
            change: CHANGE.into(),
            agent: p.agent.key(),
            judge: p.judge.clone(),
            lane: p.lane,
            verdict: p.verdict.clone(),
            result: result.into(),
            audited: false,
            missed: false,
            commit: Some("merge1".into()),
            involved: vec![],
            note: None,
            components: vec![],
            reviewed_by: vec![],
            approval: None,
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
    fn approver_follows_lane_verdict_and_outcome() {
        let by = |lane: Lane, verdict: Option<&str>, results: &[&str], merged_by: Option<&str>| {
            let p = pull(lane, verdict);
            let lines: Vec<Outcome> = results
                .iter()
                .enumerate()
                .map(|(n, r)| Outcome {
                    merged_by: merged_by.map(str::to_string),
                    ..outcome(&p, r, 10 + n as u64)
                })
                .collect();
            let pulls = [p];
            build(Inputs {
                pulls: &pulls,
                outcomes: &lines,
                ..Inputs::new(CHANGE, 100)
            })
            .approval
        };
        for lane in [Lane::AutoMerge, Lane::Judge, Lane::Human, Lane::Blocked] {
            assert_eq!(by(lane, None, &[], None).by, ApprovedBy::Pending);
            assert_eq!(by(lane, None, &["closed"], None).by, ApprovedBy::None);
        }
        assert_eq!(
            by(Lane::AutoMerge, None, &["merged"], None).by,
            ApprovedBy::Policy
        );
        let judged = by(Lane::Judge, Some("approve"), &["merged"], None);
        assert_eq!(judged.by, ApprovedBy::Judge);
        assert_eq!(judged.judge.as_deref(), Some("j1"));
        for verdict in [Some("escalate"), Some("reject"), None] {
            let a = by(Lane::Judge, verdict, &["merged"], Some("@mike"));
            assert_eq!(a.by, ApprovedBy::Person, "{verdict:?}");
            assert_eq!(a.judge, None);
        }
        let human = by(Lane::Human, None, &["merged"], Some("@mike"));
        assert_eq!(human.by, ApprovedBy::Person);
        assert_eq!(human.people, ["@mike"]);
        assert_eq!(
            by(Lane::Blocked, None, &["merged"], Some("@mike")).by,
            ApprovedBy::Override
        );
        // Reverted later: still merged by a person.
        assert_eq!(
            by(Lane::Human, None, &["merged", "reverted"], Some("@mike")).by,
            ApprovedBy::Person
        );
        // People: row approvers, then reviewers, then who merged, each once.
        let mut p = pull(Lane::Human, None);
        p.approvals = vec![RowOk {
            row: "r1".into(),
            by: "@lead".into(),
        }];
        let mut merged = outcome(&p, "merged", 10);
        merged.reviewed_by = vec!["@grace".into(), "@lead".into()];
        merged.approval = Some(onus_core::ApprovalRule::Owner);
        merged.merged_by = Some("@grace".into());
        let r = build(Inputs {
            pulls: std::slice::from_ref(&p),
            outcomes: std::slice::from_ref(&merged),
            ..Inputs::new(CHANGE, 100)
        });
        assert_eq!(r.approval.people, ["@lead", "@grace"]);
        assert_eq!(r.approval.rows[0].row, "r1");
        // Whose review counted, as the records say.
        assert_eq!(r.approval.review, Some(onus_core::ApprovalRule::Owner));
        assert!(!r.gaps.iter().any(|g| g.field == "approval"));
        // Rebuilt with no reviewers in the records: no rule is claimed.
        let again = build(Inputs {
            from: Some(r),
            ..Inputs::new(CHANGE, 110)
        });
        assert_eq!(again.approval.review, None);
        // A human-lane change still waiting: the rule is on the pull record,
        // but nobody approved, so none is claimed.
        let mut waiting = pull(Lane::Human, None);
        waiting.approval = Some(onus_core::ApprovalRule::Owner);
        let r = build(Inputs {
            pulls: std::slice::from_ref(&waiting),
            ..Inputs::new(CHANGE, 100)
        });
        assert_eq!(r.approval.review, None);
    }

    #[test]
    fn facts_from_while_open_survive_a_rebuild_from_records() {
        let sub = submission();
        let c = classification(Lane::Judge);
        let j = judgment(Verdict::Approve, Lane::Judge);
        let p = pull(Lane::Judge, Some("approve"));
        let open = build(Inputs {
            submission: Some(&sub),
            intent_source: IntentSource::PullRequest,
            agent_source: Some("branch"),
            classification: Some(&c),
            judgment: Some(&j),
            policy: Some("j1".into()),
            pulls: std::slice::from_ref(&p),
            ..Inputs::new(CHANGE, 50)
        });
        assert_eq!(open.stage, Stage::Open);
        assert_eq!(open.approval.by, ApprovedBy::Pending);
        assert_eq!(open.agent.detected_from, "branch");
        let intent = open.intent.as_ref().unwrap();
        assert_eq!(intent.source, IntentSource::PullRequest);
        assert_eq!(intent.sha256.len(), 64);
        assert_eq!(intent.check.as_ref().unwrap().stated, "SMS on shipping");
        assert!(open.evidence[0].at_head);
        assert_eq!(open.report.as_ref().unwrap().changed_files, 2);

        // Later, from the records alone.
        let lines = [outcome(&p, "merged", 60)];
        let merged = build(Inputs {
            from: Some(open.clone()),
            pulls: std::slice::from_ref(&p),
            outcomes: &lines,
            ..Inputs::new(CHANGE, 70)
        });
        assert_eq!(merged.stage, Stage::Merged);
        assert_eq!(merged.judge, open.judge);
        assert_eq!(merged.judge.as_ref().unwrap().steps[0].status, "passed");
        assert_eq!(merged.intent, open.intent);
        assert_eq!(merged.evidence, open.evidence);
        assert_eq!(merged.lane, open.lane);
        assert_eq!(merged.agent.detected_from, "branch");
        assert_eq!(merged.agent.record.as_ref().unwrap().merged, 1);
        assert_eq!(merged.approval.by, ApprovedBy::Judge);
        assert_eq!(merged.commits.merge.as_deref(), Some("merge1"));
        assert_eq!(merged.rollout.at, Some(60));
        assert_eq!(merged.generated_at, 70);
        assert!(digest_ok(&merged));
    }

    #[test]
    fn token_precedence() {
        let secret = "EpYBCisKBHRhc2sYAyIJCgcIChIDGIAIMhcKFQoCCBsSDwgFEgsIBBIHCAQSAxiBCBIkCAA";
        let verified = Token {
            task: "sms".into(),
            rights: vec!["write:path:services/**".into()],
            attenuations: vec![],
            expires: Some(99),
            fingerprint: Some(fingerprint(secret)),
            source: TokenSource::Token,
        };
        let mut sub = submission();
        sub.scope = Some(ScopeUsed {
            task: "sms".into(),
            rights: vec!["write:path:services/notifications/**".into()],
            attenuations: vec![],
            expires: Some(50),
            fingerprint: Some(fingerprint(secret)),
        });
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        log.append(
            Record {
                actor: "operator".into(),
                action: "mint".into(),
                subject: "sms".into(),
                decision: "granted".into(),
                reason: "task plan".into(),
                details: serde_json::json!({ "rights": ["write:path:a/**"], "expires": 40 }),
            },
            1,
        )
        .unwrap();
        log.append(
            Record {
                actor: "@lead".into(),
                action: "grant".into(),
                subject: "req1".into(),
                decision: "granted".into(),
                reason: "granted by @lead".into(),
                details: serde_json::json!({ "task": "sms", "scopes": ["write:path:b/**"] }),
            },
            2,
        )
        .unwrap();
        let audit = read_audit(log.path()).unwrap();
        let earlier = ProvenanceRecord {
            token: Some(Token {
                source: TokenSource::Audit,
                rights: vec!["from-before".into()],
                ..verified.clone()
            }),
            ..build(Inputs::new(CHANGE, 1))
        };
        let token = |t: Option<Token>, s: Option<&Submission>, a: bool, f: bool| {
            build(Inputs {
                token: t,
                submission: s,
                audit: a.then(|| audit.clone()),
                task: Some("sms"),
                from: f.then(|| earlier.clone()),
                ..Inputs::new(CHANGE, 5)
            })
        };
        let r = token(Some(verified.clone()), Some(&sub), true, true);
        assert_eq!(r.token.as_ref().unwrap().source, TokenSource::Token);
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains(secret), "the token itself is never kept");
        assert!(json.contains(&fingerprint(secret)));
        let r = token(None, Some(&sub), true, true);
        let t = r.token.unwrap();
        assert_eq!((t.source, t.expires), (TokenSource::Submission, Some(50)));
        let r = token(None, None, true, true);
        let t = r.token.unwrap();
        assert_eq!(t.source, TokenSource::Audit);
        assert_eq!(t.rights, ["write:path:a/**", "write:path:b/**"]);
        assert_eq!(t.expires, Some(40));
        let r = token(None, None, false, true);
        assert_eq!(r.token.unwrap().rights, ["from-before"]);
        let r = token(None, None, false, false);
        assert!(r.token.is_none());
        assert!(r.gaps.iter().any(|g| g.field == "token"));
    }

    fn request(task: &str, scope: &str) -> Request {
        let mut r = Request {
            id: String::new(),
            task: task.into(),
            kind: EscalationKind::Permission,
            scopes: vec![scope.parse().unwrap()],
            evidence: vec![Proof::parse("rationale:needs it").unwrap()],
            reason: "needs it".into(),
            blast_radius: 0,
            sensitive: vec![],
            owners: vec![],
            at: 1,
        };
        r.id = r.content_id();
        r
    }

    fn entry(log: &AuditLog, actor: &str, action: &str, subject: &str, decision: &str, at: u64) {
        log.append(
            Record {
                actor: actor.into(),
                action: action.into(),
                subject: subject.into(),
                decision: decision.into(),
                reason: format!("{action} {decision}"),
                details: serde_json::Value::Null,
            },
            at,
        )
        .unwrap();
    }

    #[test]
    fn escalations_join_requests_and_decisions() {
        let granted = request("sms", "write:path:orders/**");
        let denied = request("sms", "write:path:auth/**");
        let open = request("sms", "write:path:web/**");
        let automatic = request("sms", "write:path:docs/**");
        let other = request("other", "write:path:x/**");
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "sms", "escalate", &granted.id, "needs-person", 1);
        entry(&log, "@lead", "grant", &granted.id, "granted", 2);
        entry(&log, "@lead", "deny", &denied.id, "denied", 3);
        entry(&log, "sms", "escalate", &automatic.id, "granted", 4);
        let mut sub = submission();
        sub.escalations = vec![granted.id.clone()];
        let requests = [
            granted.clone(),
            denied.clone(),
            open.clone(),
            automatic.clone(),
            other,
        ];
        let r = build(Inputs {
            submission: Some(&sub),
            requests: &requests,
            audit: Some(read_audit(log.path()).unwrap()),
            task: Some("sms"),
            ..Inputs::new(CHANGE, 5)
        });
        let ids: Vec<&str> = r.escalations.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, [&granted.id, &denied.id, &open.id, &automatic.id]);
        let e = &r.escalations[0];
        assert_eq!(e.decision, "granted");
        assert_eq!(e.by.as_deref(), Some("@lead"));
        assert_eq!(e.kind.as_deref(), Some("permission"));
        assert_eq!(e.scopes, ["write:path:orders/**"]);
        assert_eq!(e.best_grade, Some(5));
        assert_eq!(e.at, Some(2));
        assert_eq!(r.escalations[1].decision, "denied");
        assert_eq!(r.escalations[2].decision, "open");
        assert_eq!(r.escalations[2].by, None);
        assert_eq!(r.escalations[3].by.as_deref(), Some("policy"));
        assert!(!r.gaps.iter().any(|g| g.field == "escalations"));
    }

    fn decisions(json: serde_json::Value) -> Vec<EscalationDecision> {
        let read = parse_decisions(&json.to_string()).unwrap();
        assert_eq!(read.unread, Vec::<String>::new());
        read.list
    }

    #[test]
    fn cloud_decisions_fill_escalations_and_the_token() {
        let local = request("sms", "write:path:orders/**");
        let mut sub = submission();
        sub.escalations = vec![local.id.clone()];
        let bare = sub.clone();
        sub.scope = Some(ScopeUsed {
            task: "sms".into(),
            rights: vec!["write:path:orders/**".into()],
            attenuations: vec![],
            expires: Some(50),
            fingerprint: None,
        });
        let inspected =
            serde_json::json!({ "task": "sms", "rights": ["write:path:a/**"], "expires": 99 });
        let cloud = decisions(serde_json::json!([
            {
                "id": "uuid-1", "clientRequestId": local.id, "kind": "permission",
                "scopes": ["write:path:orders/**"], "state": "granted",
                "decidedBy": "user:7", "decidedByName": "@lead",
                "decidedAt": "2026-10-12T10:00:00.000Z", "reason": "needs it",
                "token": inspected,
            },
            {
                "id": "uuid-2", "kind": "permission", "scopes": ["write:path:auth/**"],
                "state": "denied", "decidedBy": "user:8", "decidedAt": 1700,
                "answer": "not in this task", "task": "sms",
            },
            {
                "id": "uuid-3", "kind": "permission", "scopes": ["write:path:docs/**"],
                "state": "granted", "decidedBy": "system:policy", "task": "sms",
            },
            { "id": "uuid-4", "kind": "broken-test", "state": "waiting", "bestGrade": 1, "task": "sms" },
        ]));
        let build_with = |sub: Option<&Submission>, inspection: Option<TokenInspection>| {
            build(Inputs {
                submission: sub,
                requests: std::slice::from_ref(&local),
                decisions: Some(&cloud),
                inspection,
                ..Inputs::new(CHANGE, 5)
            })
        };
        let r = build_with(Some(&sub), None);
        let ids: Vec<(&str, Option<&str>)> = r
            .escalations
            .iter()
            .map(|e| (e.id.as_str(), e.cloud_id.as_deref()))
            .collect();
        assert_eq!(
            ids,
            [
                (local.id.as_str(), Some("uuid-1")),
                ("uuid-2", Some("uuid-2")),
                ("uuid-3", Some("uuid-3")),
                ("uuid-4", Some("uuid-4")),
            ]
        );
        let e = &r.escalations[0];
        assert_eq!(e.decision, "granted");
        assert_eq!(e.by.as_deref(), Some("@lead"));
        assert_eq!(e.at, Some(1_791_799_200));
        // The request kept here is the binary's own: its grade stands.
        assert_eq!(e.best_grade, Some(5));
        let e = &r.escalations[1];
        assert_eq!(
            (e.decision.as_str(), e.by.as_deref(), e.at),
            ("denied", Some("user:8"), Some(1700))
        );
        assert_eq!(e.reason.as_deref(), Some("not in this task"));
        assert_eq!(e.scopes, ["write:path:auth/**"]);
        assert_eq!(r.escalations[2].by.as_deref(), Some("policy"));
        let e = &r.escalations[3];
        assert_eq!(
            (e.decision.as_str(), e.by.as_deref(), e.best_grade),
            ("needs-person", None, Some(1))
        );
        // The submission's verified scope comes before the door's word.
        assert_eq!(r.token.as_ref().unwrap().source, TokenSource::Submission);
        assert!(!r.gaps.iter().any(|g| g.field == "escalations"));

        // In CI, with no submission token: the inspection Onus Cloud kept,
        // widened by what it granted to the task.
        let r = build_with(Some(&bare), None);
        let t = r.token.as_ref().unwrap();
        assert_eq!(t.source, TokenSource::Inspection);
        assert_eq!(t.task, "sms");
        assert_eq!(
            t.rights,
            [
                "write:path:a/**",
                "write:path:orders/**",
                "write:path:docs/**"
            ]
        );
        assert_eq!((t.expires, t.fingerprint.as_deref()), (Some(99), None));
        assert!(!r.gaps.iter().any(|g| g.field == "token"));
        assert!(digest_ok(&r));
        // A given inspection comes first.
        let given =
            parse_inspection(r#"{"task":"sms","rights":["write:path:b/**"],"expires":0}"#).unwrap();
        assert_eq!(given.expires, None);
        let t = build_with(Some(&bare), Some(given)).token.unwrap();
        assert_eq!(t.rights[0], "write:path:b/**");

        // Rebuilt from the record alone: the facts are kept.
        let again = build(Inputs {
            from: Some(r.clone()),
            ..Inputs::new(CHANGE, 9)
        });
        assert_eq!(
            (&again.token, &again.escalations),
            (&r.token, &r.escalations)
        );
    }

    #[test]
    fn an_empty_list_from_onus_cloud_is_on_record() {
        let none = build(Inputs {
            decisions: Some(&[]),
            ..Inputs::new(CHANGE, 1)
        });
        assert!(none.escalations.is_empty());
        assert!(!none.gaps.iter().any(|g| g.field == "escalations"));
        let kept = build(Inputs {
            from: Some(none),
            ..Inputs::new(CHANGE, 2)
        });
        assert!(!kept.gaps.iter().any(|g| g.field == "escalations"));
        // Never given: still a gap, and a rebuild keeps it.
        let unknown = build(Inputs::new(CHANGE, 1));
        let kept = build(Inputs {
            from: Some(unknown),
            ..Inputs::new(CHANGE, 2)
        });
        assert!(kept.gaps.iter().any(|g| g.field == "escalations"));
    }

    #[test]
    fn decisions_read_as_onus_cloud_keeps_and_shows_them() {
        // The API's views: the decision is nested.
        let views = decisions(serde_json::json!({ "escalations": [{
            "id": "uuid-1", "kind": "permission", "state": "denied",
            "scopes": ["write:path:auth/**"], "reason": "needs it", "pull": 7,
            "token": { "task": "sms", "rights": [], "expires": 99 },
            "decision": { "by": { "id": "user:8", "name": "Grace" },
                          "at": "2026-10-12T10:00:00Z", "answer": "no" },
        }]}));
        let d = &views[0];
        assert_eq!(d.decided_by.as_deref(), Some("user:8"));
        assert_eq!(d.decided_by_name.as_deref(), Some("Grace"));
        assert_eq!(d.answer.as_deref(), Some("no"));
        assert_eq!(d.token.as_ref().unwrap().task, "sms");
        // A stored row: `status`, and what was sent under `input`.
        let rows = decisions(serde_json::json!([{
            "id": "uuid-2", "kind": "broken-test", "status": "resolved",
            "input": { "scopes": [], "reason": "the test is wrong", "evidence": [] },
            "decidedBy": "user:8", "decidedAt": null, "answer": "skip it", "token": null,
        }]));
        assert_eq!(rows[0].state, CloudState::Resolved);
        assert_eq!(rows[0].reason.as_deref(), Some("the test is wrong"));
        // A token is never taken in place of its inspection.
        let err =
            parse_decisions(r#"[{"id":"u","state":"waiting","token":"EpYBCisK"}]"#).unwrap_err();
        assert!(err.downcast_ref::<HoldsToken>().is_some());
        assert!(err.to_string().contains("never the token"), "{err}");
        for raw in ["EpYBCisKBHRhc2sYAyIJ\n", r#""EpYBCisKBHRhc2sYAyIJ""#] {
            let err = parse_inspection(raw).unwrap_err();
            assert!(err.downcast_ref::<HoldsToken>().is_some(), "{err}");
        }
        assert!(
            parse_inspection("{")
                .unwrap_err()
                .downcast_ref::<HoldsToken>()
                .is_none()
        );
        assert!(parse_decisions(r#"{"id":"u"}"#).is_err());
        // One escalation that cannot be read is left out, with a note naming
        // it; the rest are read.
        let read = parse_decisions(
            r#"[{"id":"when","state":"granted","decidedAt":"yesterday"},
                {"id":"what","state":"weird"},
                {"id":"rights","state":"granted","scopes":[1]},
                7,
                {"id":"ok","state":"denied","token":{"task":"t","rights":[],"expires":-1}},
                {"id":"half","state":"denied","token":{"task":"t","rights":[],"expires":1.5}}]"#,
        )
        .unwrap();
        let ids: Vec<&str> = read.list.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["ok", "half"]);
        // A door may send any number: anything but a positive whole one is
        // no expiry.
        assert!(
            read.list
                .iter()
                .all(|d| d.token.as_ref().unwrap().expires.is_none())
        );
        assert_eq!(read.unread.len(), 4, "{:?}", read.unread);
        for (note, id) in read.unread.iter().zip(["when", "what", "rights", "#4"]) {
            assert!(note.starts_with(&format!("escalation {id}:")), "{note}");
        }
    }

    #[test]
    fn a_decision_made_here_stands_against_onus_clouds() {
        let granted = request("sms", "write:path:orders/**");
        let denied = request("sms", "write:path:auth/**");
        let waiting = request("sms", "write:path:web/**");
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "@lead", "grant", &granted.id, "granted", 2);
        entry(&log, "@lead", "deny", &denied.id, "denied", 3);
        entry(&log, "sms", "escalate", &waiting.id, "needs-person", 4);
        let cloud = decisions(serde_json::json!([
            // Less final than the door log: the log stands.
            { "id": "uuid-1", "clientRequestId": granted.id, "state": "waiting", "task": "sms" },
            // The same decision, with nothing to add: the log's actor and time stay.
            { "id": "uuid-2", "clientRequestId": denied.id, "state": "denied", "task": "sms" },
            // Decided in Onus Cloud after the door asked.
            {
                "id": "uuid-3", "clientRequestId": waiting.id, "state": "granted",
                "decidedBy": "user:9", "decidedAt": 9, "task": "sms",
                "reason": "the agent's own words", "answer": "fine",
            },
        ]));
        let r = build(Inputs {
            requests: &[granted, denied, waiting],
            audit: Some(read_audit(log.path()).unwrap()),
            decisions: Some(&cloud),
            task: Some("sms"),
            ..Inputs::new(CHANGE, 5)
        });
        let got: Vec<_> = r
            .escalations
            .iter()
            .map(|e| {
                (
                    e.decision.as_str(),
                    e.by.as_deref(),
                    e.at,
                    e.cloud_id.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("granted", Some("@lead"), Some(2), Some("uuid-1")),
                ("denied", Some("@lead"), Some(3), Some("uuid-2")),
                ("granted", Some("user:9"), Some(9), Some("uuid-3")),
            ]
        );
        // The reason is the decider's, never why the agent asked.
        assert_eq!(r.escalations[2].reason.as_deref(), Some("fine"));
    }

    #[test]
    fn only_this_changes_escalations_and_task_are_kept() {
        let cloud = decisions(serde_json::json!({ "escalations": [
            { "id": "a", "state": "granted", "task": "sms", "pull": 7, "repository": "acme/shop",
              "scopes": ["write:path:a/**"],
              "token": { "task": "sms", "rights": [], "expires": 0 } },
            { "id": "b", "state": "granted", "task": "sms", "pull": 8, "scopes": ["write:path:b/**"] },
            { "id": "c", "state": "granted", "task": "sms", "repositoryFullName": "acme/web" },
            { "id": "d", "state": "granted", "task": "other", "scopes": ["write:path:d/**"] },
            // No pull request, but the task the one on it names.
            { "id": "e", "state": "denied", "task": "sms" },
            // Nothing ties it to this change.
            { "id": "f", "state": "denied" },
        ]}));
        let r = build(Inputs {
            decisions: Some(&cloud),
            ..Inputs::new(CHANGE, 5)
        });
        let ids: Vec<&str> = r.escalations.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["a", "e"]);
        let t = r.token.as_ref().unwrap();
        assert_eq!(t.rights, ["write:path:a/**"]);
        // 0 is no expiry, not the epoch.
        assert_eq!(t.expires, None);
        // A given inspection for another task than --task is left out.
        let other = parse_inspection(r#"{"task":"other","rights":[]}"#).unwrap();
        let r = build(Inputs {
            inspection: Some(other),
            task: Some("sms"),
            ..Inputs::new(CHANGE, 5)
        });
        assert!(r.token.is_none());
    }

    #[test]
    fn every_escalation_on_this_pull_request_is_kept_in_any_order() {
        let one = serde_json::json!({ "id": "one", "state": "granted", "task": "one", "pull": 7,
            "repository": "acme/shop", "token": { "task": "one", "rights": [] } });
        let two = serde_json::json!({ "id": "two", "state": "denied", "task": "two", "pull": 7,
            "repositoryFullName": "Acme/Shop" });
        for list in [[&one, &two], [&two, &one]] {
            let cloud = decisions(serde_json::json!(list));
            let r = build(Inputs {
                decisions: Some(&cloud),
                ..Inputs::new(CHANGE, 5)
            });
            let mut ids: Vec<&str> = r.escalations.iter().map(|e| e.id.as_str()).collect();
            ids.sort();
            assert_eq!(ids, ["one", "two"]);
            assert!(!r.gaps.iter().any(|g| g.field == "escalations"));
            // Two tasks: neither inspection is the token.
            assert!(r.token.is_none());
        }
    }

    #[test]
    fn a_local_change_keeps_onus_clouds_escalations() {
        // A head commit names no repository or pull request: nothing is
        // left out for naming them.
        let cloud = decisions(serde_json::json!([
            { "id": "a", "state": "granted", "task": "sms", "pull": 7, "repository": "acme/shop" },
            { "id": "b", "state": "waiting", "task": "sms", "repositoryFullName": "acme/shop" },
        ]));
        let r = build(Inputs {
            decisions: Some(&cloud),
            ..Inputs::new("0123456789abcdef0123456789abcdef01234567", 5)
        });
        let ids: Vec<&str> = r.escalations.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert!(!r.gaps.iter().any(|g| g.field == "escalations"));
        // A list that is all another change's is no record of this one's.
        let other = decisions(serde_json::json!([
            { "id": "a", "state": "granted", "pull": 8, "repository": "acme/shop" },
        ]));
        let r = build(Inputs {
            decisions: Some(&other),
            ..Inputs::new(CHANGE, 5)
        });
        assert!(r.escalations.is_empty());
        let g = r.gaps.iter().find(|g| g.field == "escalations").unwrap();
        assert!(g.reason.contains("other changes"), "{}", g.reason);
    }

    #[test]
    fn an_unread_escalation_keeps_the_gap() {
        let cloud = decisions(serde_json::json!([{ "id": "a", "state": "granted", "pull": 7 }]));
        let unread = ["escalation b: unknown variant `weird`".to_string()];
        let r = build(Inputs {
            decisions: Some(&cloud),
            unread: &unread,
            ..Inputs::new(CHANGE, 5)
        });
        assert_eq!(r.escalations.len(), 1);
        let g = r.gaps.iter().find(|g| g.field == "escalations").unwrap();
        assert!(g.reason.contains("escalation b"), "{}", g.reason);
        // A rebuild keeps the gap and why.
        let again = build(Inputs {
            from: Some(r.clone()),
            ..Inputs::new(CHANGE, 6)
        });
        assert_eq!(
            again.gaps.iter().find(|g| g.field == "escalations"),
            Some(g)
        );
    }

    #[test]
    fn a_stored_row_waiting_past_its_expiry_has_expired() {
        let cloud = decisions(serde_json::json!([
            { "id": "a", "status": "waiting", "pullNumber": 7, "expiresAt": "2026-10-12T10:00:00.000Z" },
            { "id": "b", "status": "waiting", "pullNumber": 7, "expiresAt": 4_000_000_000_u64 },
        ]));
        let r = build(Inputs {
            decisions: Some(&cloud),
            ..Inputs::new(CHANGE, 1_791_799_201)
        });
        let got: Vec<&str> = r.escalations.iter().map(|e| e.decision.as_str()).collect();
        assert_eq!(got, ["expired", "needs-person"]);
    }

    #[test]
    fn onus_clouds_list_alone_updates_the_escalations_kept() {
        let req = request("sms", "write:path:orders/**");
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "sms", "escalate", &req.id, "needs-person", 4);
        let open = build(Inputs {
            requests: std::slice::from_ref(&req),
            audit: Some(read_audit(log.path()).unwrap()),
            task: Some("sms"),
            ..Inputs::new(CHANGE, 5)
        });
        assert_eq!(open.escalations.len(), 1);
        let cloud = decisions(serde_json::json!([
            { "id": "uuid-9", "state": "denied", "task": "sms", "pull": 7, "decidedBy": "user:1" },
        ]));
        let r = build(Inputs {
            from: Some(open.clone()),
            decisions: Some(&cloud),
            ..Inputs::new(CHANGE, 6)
        });
        // The door's request stays, and Onus Cloud's own is added.
        let got: Vec<(&str, &str)> = r
            .escalations
            .iter()
            .map(|e| (e.id.as_str(), e.decision.as_str()))
            .collect();
        assert_eq!(
            got,
            [(req.id.as_str(), "needs-person"), ("uuid-9", "denied")]
        );
        // And a later list updates the one it added, by Onus Cloud's id.
        let later = decisions(serde_json::json!([
            { "id": "uuid-9", "state": "denied", "task": "sms", "decidedAt": 8 },
        ]));
        let r = build(Inputs {
            from: Some(r),
            decisions: Some(&later),
            ..Inputs::new(CHANGE, 9)
        });
        assert_eq!(r.escalations.len(), 2);
        assert_eq!(r.escalations[1].at, Some(8));
    }

    #[test]
    fn a_refresh_keeps_a_verified_token_over_the_doors_word() {
        let mut sub = submission();
        sub.scope = Some(ScopeUsed {
            task: "sms".into(),
            rights: vec!["write:path:orders/**".into()],
            attenuations: vec![],
            expires: Some(50),
            fingerprint: Some("00000000000000ab".into()),
        });
        let open = build(Inputs {
            submission: Some(&sub),
            ..Inputs::new(CHANGE, 1)
        });
        let cloud = decisions(serde_json::json!([{
            "id": "a", "state": "granted", "task": "sms",
            "token": { "task": "sms", "rights": ["write:path:x/**"], "expires": 9 },
        }]));
        let refreshed = build(Inputs {
            from: Some(open.clone()),
            decisions: Some(&cloud),
            ..Inputs::new(CHANGE, 2)
        });
        assert_eq!(refreshed.token, open.token);
        assert_eq!(refreshed.escalations[0].cloud_id.as_deref(), Some("a"));
        // An inspection kept earlier gives way to a fresh one.
        let inspected = build(Inputs {
            decisions: Some(&cloud),
            ..Inputs::new(CHANGE, 1)
        });
        let fresh = parse_inspection(r#"{"task":"sms","rights":["write:path:y/**"]}"#).unwrap();
        let t = build(Inputs {
            from: Some(inspected),
            inspection: Some(fresh),
            ..Inputs::new(CHANGE, 2)
        })
        .token
        .unwrap();
        assert_eq!(t.rights, ["write:path:y/**"]);
    }

    #[test]
    fn doors_entries_are_the_tasks_and_chain_intact() {
        let req = request("sms", "write:path:orders/**");
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "operator", "mint", "sms", "granted", 1);
        entry(&log, "operator", "mint", "other", "granted", 2);
        entry(&log, "sms", "mcp:onus_find", "billing", "allowed", 3);
        entry(&log, "other", "push", "refs/heads/x", "allowed", 4);
        entry(&log, "sms", "escalate", &req.id, "needs-person", 5);
        entry(&log, "@lead", "grant", &req.id, "granted", 6);
        let build_with = |audit| {
            build(Inputs {
                requests: std::slice::from_ref(&req),
                audit: Some(audit),
                task: Some("sms"),
                ..Inputs::new(CHANGE, 9)
            })
        };
        let r = build_with(read_audit(log.path()).unwrap());
        let doors = r.doors.unwrap();
        assert!(doors.intact);
        let seqs: Vec<u64> = doors.entries.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [1, 3, 5, 6]);
        assert_eq!(doors.entries[0].action, "mint");
        assert_eq!(doors.entries[3].actor, "@lead");
        assert!(!r.gaps.iter().any(|g| g.field == "doors"));
        // An entry changed after it was written breaks the chain.
        let text = std::fs::read_to_string(log.path()).unwrap();
        std::fs::write(log.path(), text.replace("mcp:onus_find", "mcp:onus_file")).unwrap();
        let r = build_with(read_audit(log.path()).unwrap());
        let doors = r.doors.unwrap();
        assert!(!doors.intact);
        assert_eq!(doors.entries.len(), 4);
        assert!(
            r.gaps
                .iter()
                .any(|g| g.field == "doors" && g.reason.contains("broken"))
        );
    }

    #[test]
    fn gaps_name_what_is_missing() {
        let r = build(Inputs::new(CHANGE, 1));
        let fields: Vec<&str> = r.gaps.iter().map(|g| g.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "intent",
                "token",
                "evidence",
                "escalations",
                "judge",
                "doors"
            ]
        );
        assert!(r.gaps[1].reason.contains("--token/--key-public"));
        assert!(r.gaps[1].reason.contains("--token-inspection"));
        assert!(r.gaps[3].reason.contains("Onus Cloud"));
        assert!(r.gaps[4].reason.contains("--judge"));
        assert_eq!(r.agent.tool, "unknown");
        assert_eq!(r.rollout.result, "open");
        // Merged by a person nobody recorded (a backfill).
        let p = pull(Lane::Human, None);
        let lines = [outcome(&p, "merged", 3)];
        let r = build(Inputs {
            pulls: std::slice::from_ref(&p),
            outcomes: &lines,
            ..Inputs::new(CHANGE, 4)
        });
        assert_eq!(r.approval.by, ApprovedBy::Person);
        assert!(
            r.gaps
                .iter()
                .any(|g| g.field == "approval" && g.reason == "who merged it was not recorded")
        );
        // A submission with everything fills them.
        let sub = submission();
        let j = judgment(Verdict::Approve, Lane::Judge);
        let r = build(Inputs {
            submission: Some(&sub),
            judgment: Some(&j),
            ..Inputs::new(CHANGE, 4)
        });
        let fields: Vec<&str> = r.gaps.iter().map(|g| g.field.as_str()).collect();
        assert_eq!(fields, ["token", "escalations", "doors"]);
    }

    #[test]
    fn digest_round_trip() {
        let sub = submission();
        let r = build(Inputs {
            submission: Some(&sub),
            ..Inputs::new(CHANGE, 4)
        });
        assert_eq!(r.digest.len(), 64);
        assert!(digest_ok(&r));
        let json = serde_json::to_string_pretty(&r).unwrap();
        let back: ProvenanceRecord = serde_json::from_str(&json).unwrap();
        assert!(digest_ok(&back));
        let mut changed = back.clone();
        changed.stage = Stage::Merged;
        assert!(!digest_ok(&changed));
        let mut changed = back.clone();
        changed.approval.people.push("@someone".into());
        assert!(!digest_ok(&changed));
        let mut changed = back.clone();
        changed.evidence[0].exit_code = 1;
        assert!(!digest_ok(&changed));
        let mut resealed = changed;
        seal(&mut resealed);
        assert!(digest_ok(&resealed));
    }

    fn stage(
        id: &str,
        kind: SignalKind,
        at: u64,
        percent: Option<u8>,
        decision: Option<&str>,
    ) -> ProductionSignal {
        let mut s: ProductionSignal =
            serde_json::from_value(serde_json::json!({ "id": id, "kind": kind })).unwrap();
        s.at = Some(at);
        s.change = Some(CHANGE.into());
        s.source = "argo".into();
        s.rollout = percent.map(|percent| onus_core::signal::RolloutStage {
            percent,
            decision: decision.map(str::to_string),
            flag: None,
        });
        s
    }

    fn status(results: &[&str], signals: &[ProductionSignal]) -> (RolloutStatus, Option<u8>) {
        let p = pull(Lane::Judge, Some("approve"));
        let lines: Vec<Outcome> = results
            .iter()
            .enumerate()
            .map(|(n, r)| outcome(&p, r, 10 + n as u64))
            .collect();
        let r = build(Inputs {
            pulls: std::slice::from_ref(&p),
            outcomes: &lines,
            signals,
            ..Inputs::new(CHANGE, 100)
        });
        (r.rollout.status.unwrap(), r.rollout.percent)
    }

    #[test]
    fn rollout_status_follows_stages_and_outcomes() {
        use RolloutStatus::*;
        let deploy = |id: &str, at: u64, percent: Option<u8>| {
            stage(id, SignalKind::Deploy, at, percent, None)
        };
        let decided = |id: &str, at: u64, percent: u8, decision: &str| {
            stage(id, SignalKind::Rollout, at, Some(percent), Some(decision))
        };
        let mut staging = deploy("d:staging", 30, None);
        staging.environment = Some("staging".into());
        let at25 = deploy("d:25", 20, Some(25));
        let one = std::slice::from_ref(&at25);
        assert_eq!(status(&[], one), (Pending, None));
        assert_eq!(status(&["closed"], one), (NotShipped, None));
        assert_eq!(
            status(&["merged", "rolled-back"], one),
            (RolledBack, Some(25))
        );
        assert_eq!(status(&["merged", "reverted"], &[]), (Reverted, None));
        // A staging deploy is listed and does not count.
        assert_eq!(status(&["merged"], &[]), (AwaitingDeploy, None));
        assert_eq!(
            status(&["merged"], &[staging.clone()]),
            (AwaitingDeploy, None)
        );
        assert_eq!(
            status(
                &["merged"],
                &[at25.clone(), decided("r:1", 25, 25, "rollback")]
            ),
            (RolledBack, Some(25))
        );
        assert_eq!(
            status(&["merged"], &[decided("r:1", 25, 5, "hold")]),
            (Holding, Some(5))
        );
        assert_eq!(
            status(&["merged"], &[decided("r:1", 25, 100, "complete")]),
            (Complete, Some(100))
        );
        // A deploy that names no share reached all of production.
        assert_eq!(
            status(&["merged"], &[deploy("d:all", 20, None)]),
            (Complete, Some(100))
        );
        assert_eq!(
            status(&["merged", "incident"], &[at25.clone(), staging.clone()]),
            (InProgress, Some(25))
        );
        assert_eq!(
            status(&["merged"], &[decided("r:1", 25, 5, "promote")]),
            (InProgress, Some(5))
        );
        // In time order, whatever the file order.
        assert_eq!(
            status(&["merged"], &[deploy("d:100", 40, Some(100)), at25.clone()]),
            (Complete, Some(100))
        );
        let p = pull(Lane::Judge, Some("approve"));
        let lines = [outcome(&p, "merged", 10)];
        let r = build(Inputs {
            pulls: std::slice::from_ref(&p),
            outcomes: &lines,
            signals: &[at25, staging],
            ..Inputs::new(CHANGE, 100)
        });
        let kinds: Vec<(&str, Option<&str>)> = r
            .rollout
            .stages
            .iter()
            .map(|e| (e.kind.as_str(), e.environment.as_deref()))
            .collect();
        assert_eq!(kinds, [("deploy", None), ("deploy", Some("staging"))]);
        assert!(digest_ok(&r));
    }

    #[test]
    fn signals_are_listed_on_their_change_only() {
        let mut incident = stage("pd:1", SignalKind::Incident, 50, None, None);
        incident.severity = Some(onus_core::signal::Severity::High);
        incident.note = Some("checkout 500s".into());
        let mut other = incident.clone();
        other.id = "pd:2".into();
        other.change = Some("acme/shop#8".into());
        let mut loose = incident.clone();
        loose.id = "pd:3".into();
        loose.change = None;
        let deploy = stage("d:1", SignalKind::Deploy, 40, Some(10), None);
        let mut p = pull(Lane::Judge, Some("approve"));
        p.rollout = Some(crate::rollout::plan(
            Lane::Judge,
            &submission().report,
            Some(CHANGE),
            None,
            &Default::default(),
            90,
        ));
        let lines = [outcome(&p, "merged", 10)];
        let r = build(Inputs {
            pulls: std::slice::from_ref(&p),
            outcomes: &lines,
            signals: &[incident, other, loose, deploy],
            ..Inputs::new(CHANGE, 100)
        });
        let ids: Vec<&str> = r.rollout.signals.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["pd:1"]);
        assert_eq!(r.rollout.signals[0].severity.as_deref(), Some("high"));
        assert_eq!(r.rollout.stages.len(), 1);
        assert_eq!(
            (r.rollout.status, r.rollout.percent),
            (Some(RolloutStatus::InProgress), Some(10))
        );
        let plan = r.rollout.plan.as_ref().unwrap();
        assert_eq!(
            (plan.strategy.as_str(), plan.steps.as_slice()),
            ("canary", &[5, 25, 100][..])
        );
        // Kept from an earlier record when the pull record has none.
        let rebuilt = build(Inputs {
            from: Some(r.clone()),
            outcomes: &lines,
            ..Inputs::new(CHANGE, 200)
        });
        assert_eq!(rebuilt.rollout.plan, r.rollout.plan);
        // A record from before these fields still reads.
        let mut json = serde_json::to_value(&r).unwrap();
        for k in ["status", "percent", "plan", "stages", "signals"] {
            json["rollout"].as_object_mut().unwrap().remove(k);
        }
        let old: ProvenanceRecord = serde_json::from_value(json).unwrap();
        assert!(old.rollout.status.is_none() && old.rollout.stages.is_empty());
    }

    #[test]
    fn an_escalation_naming_no_pull_request_is_not_another_changes() {
        // Probe: one escalation of the repository with no pull request, and a
        // change it has nothing to do with.
        let cloud = decisions(serde_json::json!([{
            "id": "r1", "state": "granted", "task": "sms", "pull": null, "ref": "codex/sms",
            "repository": "acme/shop", "scopes": ["write:path:orders/**"],
            "decidedBy": "user:1", "decidedByName": "Lead",
            "token": { "task": "sms", "rights": ["write:path:notifications/**"] },
        }]));
        let r = build(Inputs {
            decisions: Some(&cloud),
            ..Inputs::new("acme/shop#99", 5)
        });
        assert!(r.escalations.is_empty());
        assert!(r.token.is_none());
        assert!(r.gaps.iter().any(|g| g.field == "token"));
        let g = r.gaps.iter().find(|g| g.field == "escalations").unwrap();
        assert!(g.reason.contains("other changes"), "{}", g.reason);
        // Several tasks and none known: none is picked, and none is kept.
        let several = decisions(serde_json::json!([
            { "id": "x", "state": "granted", "task": "alpha" },
            { "id": "y", "state": "denied", "task": "beta" },
        ]));
        let r = build(Inputs {
            decisions: Some(&several),
            ..Inputs::new(CHANGE, 5)
        });
        assert!(r.escalations.is_empty());
        // The task known without Onus Cloud's list ties those of the task.
        let r = build(Inputs {
            decisions: Some(&several),
            task: Some("beta"),
            ..Inputs::new(CHANGE, 5)
        });
        let ids: Vec<&str> = r.escalations.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["y"]);
        // On the change's branch, or at one of its commits, it is the change's.
        let r = build(Inputs {
            decisions: Some(&cloud),
            branch: Some("codex/sms"),
            ..Inputs::new("acme/shop#99", 5)
        });
        assert_eq!(r.escalations.len(), 1);
        let t = r.token.as_ref().unwrap();
        assert_eq!(
            (t.task.as_str(), t.source),
            ("sms", TokenSource::Inspection)
        );
        let mut at_commit = cloud.clone();
        at_commit[0].reference = Some("ABC123".into());
        let commits = ["abc123".to_string()];
        let r = build(Inputs {
            decisions: Some(&at_commit),
            commits: &commits,
            ..Inputs::new("acme/shop#99", 5)
        });
        assert_eq!(r.escalations.len(), 1);
        // A commit elsewhere outweighs the branch's name.
        let mut elsewhere = cloud.clone();
        elsewhere[0].reference = Some("refs/heads/codex/sms".into());
        elsewhere[0].commit = Some("def456".into());
        let r = build(Inputs {
            decisions: Some(&elsewhere),
            branch: Some("codex/sms"),
            commits: &commits,
            ..Inputs::new("acme/shop#99", 5)
        });
        assert!(r.escalations.is_empty());
        elsewhere[0].commit = Some("abc123".into());
        let r = build(Inputs {
            decisions: Some(&elsewhere),
            commits: &commits,
            ..Inputs::new("acme/shop#99", 5)
        });
        assert_eq!(r.escalations.len(), 1);
        // So is one filed at the pull request's head ref.
        let mut pull_head = cloud.clone();
        pull_head[0].reference = Some("refs/pull/99/head".into());
        let r = build(Inputs {
            decisions: Some(&pull_head),
            ..Inputs::new("acme/shop#99", 5)
        });
        assert_eq!(r.escalations.len(), 1);
        // A pull record's head is one of the change's commits too.
        let mut head = pull(Lane::Judge, None);
        head.change = "acme/shop#99".into();
        head.head = "abc123".into();
        let r = build(Inputs {
            decisions: Some(&at_commit),
            pulls: std::slice::from_ref(&head),
            ..Inputs::new("acme/shop#99", 5)
        });
        assert_eq!(r.escalations.len(), 1);
    }

    #[test]
    fn onus_clouds_final_decision_is_recorded_when_it_differs() {
        // Probe: the submission lists a grant that Onus Cloud denied.
        let local = request("sms", "write:path:orders/**");
        let mut sub = submission();
        sub.escalations = vec![local.id.clone()];
        let cloud = decisions(serde_json::json!([{
            "id": "uuid-1", "clientRequestId": local.id, "state": "denied",
            "decidedBy": "user:8", "decidedByName": "Grace", "decidedAt": 9, "answer": "no",
        }]));
        let r = build(Inputs {
            submission: Some(&sub),
            requests: std::slice::from_ref(&local),
            decisions: Some(&cloud),
            ..Inputs::new(CHANGE, 10)
        });
        let e = &r.escalations[0];
        assert_eq!(
            (
                e.decision.as_str(),
                e.by.as_deref(),
                e.at,
                e.reason.as_deref()
            ),
            ("denied", Some("Grace"), Some(9), Some("no"))
        );
        let g = r.gaps.iter().find(|g| g.field == "escalations").unwrap();
        assert!(
            g.reason.contains(&format!(
                "escalation {} is denied in Onus Cloud but was granted",
                local.id
            )),
            "{}",
            g.reason
        );
    }

    #[test]
    fn notes_never_echo_what_was_given() {
        let secret = "EpYBCisKBHRhc2sYAyIJ";
        let read = parse_decisions(
            &serde_json::json!([
                { "id": "a", "state": "granted", "scopes": secret },
                { "id": "b", "state": "granted", "token": { "task": "t", "rights": secret } },
                { "id": "c", "state": secret },
                { "id": "d", "state": "granted", "decidedAt": secret },
                { "id": "e", "state": "granted", "scopes": format!("{secret}, expected {secret}") },
                { "id": secret.repeat(4), "state": "granted", "scopes": 1 },
                { "id": "f" },
            ])
            .to_string(),
        )
        .unwrap();
        assert!(read.list.is_empty());
        let notes = read.unread.join("\n");
        assert!(!notes.contains(secret), "{notes}");
        assert_eq!(
            read.unread,
            [
                "escalation a: `scopes` cannot be read (expected a sequence)",
                "escalation b: `token` cannot be read (expected a sequence)",
                "escalation c: `state` cannot be read (expected one of `assessing`, `waiting`, `granted`, `denied`, `resolved`, `withdrawn`, `expired`, `failed`)",
                "escalation d: a time is neither Unix seconds nor an RFC 3339 date in UTC",
                "escalation e: `scopes` cannot be read (expected a sequence)",
                "escalation #6: `scopes` cannot be read (expected a sequence)",
                "escalation f: missing field `state`",
            ]
        );
        let err = parse_inspection(&format!(r#"{{"task":"t","rights":"{secret}"}}"#)).unwrap_err();
        assert!(!err.to_string().contains(secret), "{err}");
        assert!(err.to_string().contains("`rights`"), "{err}");
    }

    #[test]
    fn an_inspection_that_could_not_be_read_is_named_in_the_token_gap() {
        let r = build(Inputs {
            inspection_unread: Some("token.json is not a token inspection"),
            ..Inputs::new(CHANGE, 1)
        });
        let g = r.gaps.iter().find(|g| g.field == "token").unwrap();
        assert!(
            g.reason.contains("could not be read: token.json"),
            "{}",
            g.reason
        );
    }
}
