//! Builds a change's provenance record (`onus_core::provenance`) from the
//! facts Onus keeps about it: the submission, classification and judgment
//! from while it was open, the pull and outcome records, the token, the door
//! log (or the task's slice of it, attached to the work) and escalation
//! requests, the escalations Onus Cloud decided with the door's inspection
//! of the token, and Onus Cloud's log of the calls made under the token.
//! Every field is computed here, the approver included, so a host only
//! stores and shows the result.
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
    Agent, Approval, ApprovedBy, CallDecision, CallDenial, CallsSource, CloudCall, CloudCalls,
    CloudState, Commits, DoorEntry, DoorSource, Doors, Escalation, EscalationDecision, Evidence,
    Gap, Intent, IntentSource, Judge, JudgeStep, LaneFacts, LaneReason, Moment, PlanFacts,
    ProvenanceRecord, ReportFacts, Rollout, RolloutEvent, RolloutStatus, RouteCalls, RowApproval,
    SCHEMA, SignalRef, Stage, StageEvent, Token, TokenInspection, TokenSource, TrackRecord,
};
use onus_core::signal::{ProductionSignal, SignalKind};
use onus_doors::audit::Entry;
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
    /// The agent's own submission of a pull request, handed in beside the
    /// run's: only its token and the escalations it lists are taken. The
    /// caller checks it is for one of the change's commits
    /// ([`agents_submission_fits`]).
    pub agents: Option<&'a Submission>,
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
    /// When `audit` is a task's slice attached to its work in Onus Cloud:
    /// the task it names ([`DoorSource::Attached`]).
    pub audit_slice: Option<&'a str>,
    /// The commit the slice was cut at ([`DoorLog::head`]). A slice cut at
    /// a commit that is not one of the change's gives neither the task nor
    /// the token, and the `doors` gap says so.
    pub audit_head: Option<&'a str>,
    /// Why the door log does not verify, or could not be read: the doors
    /// gap says so.
    pub audit_why: Option<&'a str>,
    /// Onus Cloud's log of the calls made under the task token. Given and
    /// empty, no call was made there.
    pub calls: Option<&'a [CloudCall]>,
    /// What of Onus Cloud's calls could not be read, one note each.
    pub calls_unread: &'a [String],
    /// The door's inspection of the token sent with an attached slice
    /// ([`DoorLog::inspection`]): [`TokenSource::Attached`] when the slice
    /// verifies.
    pub attached_inspection: Option<TokenInspection>,
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
            agents: None,
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
            audit_slice: None,
            audit_head: None,
            audit_why: None,
            calls: None,
            calls_unread: &[],
            attached_inspection: None,
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

/// Whether the agent's own submission is for one of the change's commits:
/// its head, one of `commits` (base excluded), or a head the pull records
/// classified for it. Only then are its token and escalations the change's.
pub fn agents_submission_fits(
    s: &Submission,
    change: &str,
    head: &str,
    commits: &[String],
    pulls: &[PullRecord],
) -> bool {
    is_change_commit(&s.head, change, head, commits, pulls)
}

/// Whether `at` is one of the change's commits: its head, one of
/// `commits`, or the head of one of its pull records.
fn is_change_commit(
    at: &str,
    change: &str,
    head: &str,
    commits: &[String],
    pulls: &[PullRecord],
) -> bool {
    !at.is_empty()
        && (at.eq_ignore_ascii_case(head)
            || commits.iter().any(|c| c.eq_ignore_ascii_case(at))
            || pulls
                .iter()
                .any(|p| p.change == change && p.head.eq_ignore_ascii_case(at)))
}

/// Reads a door log. A log whose hash chain does not verify is still read,
/// line by line, and reported as not intact.
pub fn read_audit(path: &Path) -> Result<(Vec<Entry>, bool)> {
    let log = read_door_log(path)?;
    Ok((log.entries, log.intact))
}

/// A door log as it was read: the whole log (audit.jsonl), or a task's
/// slice attached to its work in Onus Cloud.
#[derive(Debug, Clone, Default)]
pub struct DoorLog {
    pub entries: Vec<Entry>,
    /// Its hash chain verified: from entry 1 for a log, from its first
    /// entry for a slice.
    pub intact: bool,
    /// The task a slice names; none for a whole log.
    pub slice: Option<String>,
    /// The commit a slice was cut at, when it says.
    pub head: Option<String>,
    /// Why it does not verify.
    pub why: Option<String>,
    /// The door's inspection of the token it held, sent with a slice for
    /// the slice's task; never the token.
    pub inspection: Option<TokenInspection>,
}

/// Reads a door log from a file: [`parse_door_log`].
pub fn read_door_log(path: &Path) -> Result<DoorLog> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the audit log {}", path.display()))?;
    parse_door_log(&text).with_context(|| format!("{} is not a door log", path.display()))
}

/// Reads a door log: the lines of an audit log, or a task's slice
/// (`schemas/door-log.schema.json`, also inside `{"doorLog": …}` as it
/// was attached). Entries that cannot be read are left out, and the log is
/// then not intact. A whole log's chain is checked from entry 1; a slice's
/// from its first entry, which is all a slice can show.
pub fn parse_door_log(text: &str) -> Result<DoorLog> {
    let entry = |v: &serde_json::Value| serde_json::from_value::<Entry>(v.clone()).ok();
    let mut inspection = None;
    let mut head = None;
    let (entries, slice, unread) = match serde_json::from_str::<serde_json::Value>(text.trim()) {
        // One line of a log is an object too: a slice holds `entries`.
        Ok(mut v) if v.get("doorLog").is_some() || v.get("entries").is_some() => {
            if let Some(inner) = v.get_mut("doorLog").filter(|d| d.is_object()) {
                v = inner.take();
            }
            let Some(task) = v["task"].as_str().filter(|t| !t.trim().is_empty()) else {
                bail!("a door log slice names its task");
            };
            let Some(items) = v["entries"].as_array() else {
                bail!("a door log slice holds its entries");
            };
            // The inspection of the slice's task, never a token: one that
            // cannot be read, or is another task's, is left out.
            match &v["inspection"] {
                serde_json::Value::Null => {}
                serde_json::Value::String(_) => {
                    return Err(HoldsToken("the slice's inspection is a token".into()).into());
                }
                i => {
                    inspection = serde_json::from_value::<TokenInspection>(i.clone())
                        .ok()
                        .filter(|i| i.task == task);
                }
            }
            head = v["head"]
                .as_str()
                .filter(|h| !h.trim().is_empty())
                .map(str::to_string);
            let read: Vec<Entry> = items.iter().filter_map(entry).collect();
            let unread = items.len() - read.len();
            (read, Some(task.to_string()), unread)
        }
        _ => {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let read: Vec<Entry> = lines
                .iter()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            let unread = lines.len() - read.len();
            if !lines.is_empty() && read.is_empty() {
                bail!("not an audit log: no line is an entry");
            }
            (read, None, unread)
        }
    };
    let why = if unread > 0 {
        Some(format!("{unread} entries cannot be read"))
    } else if entries.is_empty() {
        slice
            .as_ref()
            .map(|_| "the slice holds no entries".to_string())
    } else if slice.is_none() && entries[0].seq != 1 {
        Some(format!(
            "it starts at entry {}: entries before it were removed",
            entries[0].seq
        ))
    } else {
        onus_doors::audit::verify_run(&entries)
            .err()
            .map(|e| e.to_string())
    };
    Ok(DoorLog {
        intact: why.is_none(),
        entries,
        slice,
        head,
        why,
        inspection,
    })
}

/// Onus Cloud's log of a task token's calls, as far as it could be read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CloudCallsRead {
    pub list: Vec<CloudCall>,
    /// One note for each call that could not be read, naming it.
    pub unread: Vec<String>,
}

/// The fields of a call that are read. The API's rows carry more (`path`,
/// `userAgent`, `credentialName`, …), some of it set by whoever made the
/// call; those are never read, kept or checked.
const CALL_FIELDS: [&str; 11] = [
    "id",
    "at",
    "createdAt",
    "method",
    "route",
    "scope",
    "decision",
    "tokenId",
    "credentialId",
    "task",
    "reason",
];

/// Reads Onus Cloud's log of the calls made under a task token: a JSON
/// array (`schemas/cloud-calls.schema.json`), or the API's `{"calls":
/// [...]}`. Only [`CALL_FIELDS`] are read. A call that cannot be read is
/// left out with a note, and so is one with a `token`, or a field that looks
/// like one: the note names the field, never the value. A file that is not
/// such a list is an error.
pub fn parse_calls(text: &str) -> Result<CloudCallsRead> {
    let mut v: serde_json::Value = serde_json::from_str(text)
        .context("not JSON: give a JSON array of the calls Onus Cloud logged")?;
    if let Some(list) = v.get_mut("calls") {
        v = list.take();
    }
    let Some(items) = v.as_array_mut() else {
        bail!("not a JSON array of calls");
    };
    let mut read = CloudCallsRead::default();
    for (n, e) in items.iter_mut().enumerate() {
        let token = e.get("token").is_some_and(|t| !t.is_null());
        if let Some(o) = e.as_object_mut() {
            o.retain(|k, _| CALL_FIELDS.contains(&k.as_str()));
        }
        let held = CALL_FIELDS
            .into_iter()
            .find(|k| e.get(k).is_some_and(holds_token));
        // Named by its place: its id may be what holds the token.
        if let Some(k) = held.or(token.then_some("token")) {
            read.unread.push(format!(
                "call #{}: `{k}` holds what looks like a token",
                n + 1
            ));
            continue;
        }
        let e = &*e;
        let name = name_of(e, n);
        let base = serde_json::json!({
            "at": 1, "method": "GET", "route": "/", "decision": "allowed",
        });
        match serde_json::from_value::<CloudCall>(e.clone()) {
            Ok(c) if seconds(&c.at).is_none() => read.unread.push(format!(
                "call {name}: `at` is neither Unix seconds nor an RFC 3339 date in UTC"
            )),
            Ok(c) => read.list.push(c),
            Err(err) => read.unread.push(format!(
                "call {name}: {}",
                why_unread::<CloudCall>(e, base, &err)
            )),
        }
    }
    Ok(read)
}

/// Whether any text in `v` looks like a token: Onus Cloud's
/// (`onus_tok_<base62>`, `onus_cli_<base62>`, an `onus_<env>_<base62>` API
/// key), a PASETO, or a run of base64url (no `/`, `+` or `=`) as long as a
/// Biscuit. Ids, routes, paths, scopes, tool names and a token's shown
/// prefix are none of these.
fn holds_token(v: &serde_json::Value) -> bool {
    let word = |w: &str| {
        let w = w.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')));
        let parts: Vec<&str> = w.split('_').collect();
        parts.len() >= 3
            && parts[0] == "onus"
            && parts
                .last()
                .is_some_and(|k| k.len() >= 20 && k.chars().all(|c| c.is_ascii_alphanumeric()))
            || w.starts_with("v4.public.")
            || w.starts_with("v4.local.")
            || w.len() >= 100
                && w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    };
    match v {
        serde_json::Value::String(t) => t.split_whitespace().any(word),
        serde_json::Value::Array(a) => a.iter().any(holds_token),
        serde_json::Value::Object(o) => o.values().any(holds_token),
        _ => false,
    }
}

/// The most refusals a record lists; the counts hold them all.
const DENIALS: usize = 100;

/// Onus Cloud's calls, summed up by route and scope, with the refusals.
fn calls_summary(calls: &[&CloudCall]) -> CloudCalls {
    let mut sorted: Vec<(&CloudCall, u64)> = calls
        .iter()
        .map(|c| (*c, seconds(&c.at).unwrap_or(0)))
        .collect();
    sorted.sort_by_key(|(_, at)| *at);
    let mut out = CloudCalls {
        source: CallsSource::OnusCloud,
        total: 0,
        allowed: 0,
        denied: 0,
        failed: 0,
        untasked: 0,
        first_at: sorted.first().map(|(_, at)| *at),
        last_at: sorted.last().map(|(_, at)| *at),
        tokens: Vec::new(),
        routes: Vec::new(),
        denials: Vec::new(),
    };
    for (c, at) in sorted {
        out.total += 1;
        if c.task.is_none() {
            out.untasked += 1;
        }
        let at_route = match out
            .routes
            .iter()
            .position(|r| r.method == c.method && r.route == c.route && r.scope == c.scope)
        {
            Some(n) => n,
            None => {
                out.routes.push(RouteCalls {
                    method: c.method.clone(),
                    route: c.route.clone(),
                    scope: c.scope.clone(),
                    allowed: 0,
                    denied: 0,
                    failed: 0,
                });
                out.routes.len() - 1
            }
        };
        let route = &mut out.routes[at_route];
        match c.decision {
            CallDecision::Allowed => {
                out.allowed += 1;
                route.allowed += 1;
            }
            CallDecision::Denied => {
                out.denied += 1;
                route.denied += 1;
                if out.denials.len() < DENIALS {
                    out.denials.push(CallDenial {
                        at,
                        method: c.method.clone(),
                        route: c.route.clone(),
                        scope: c.scope.clone(),
                        reason: c
                            .reason
                            .as_deref()
                            .filter(|r| !r.is_empty())
                            .map(|r| r.chars().take(500).collect()),
                        token_id: c.token_id.clone(),
                    });
                }
            }
            CallDecision::Failed => {
                out.failed += 1;
                route.failed += 1;
            }
        }
        if let Some(t) = &c.token_id
            && !out.tokens.contains(t)
        {
            out.tokens.push(t.clone());
        }
    }
    out.routes
        .sort_by(|a, b| (&a.route, &a.method, &a.scope).cmp(&(&b.route, &b.method, &b.scope)));
    out
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
        // A submission lists only escalations verified as granted when it
        // was made. The agent's own, handed in beside the run, is on its
        // word: the escalations gap says so.
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
/// A decision that is only the agent's word (`on_word`: the door log it
/// attached, or its own submission) stands against nothing: Onus Cloud's is
/// recorded whatever its state, with a note when they differ. A stored row
/// still waiting past its expiry at `now` has expired, as Onus Cloud shows
/// it.
fn decide_from_cloud(
    e: &mut Escalation,
    d: &EscalationDecision,
    now: u64,
    on_word: bool,
) -> Option<String> {
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
    if on_word && e.decision != decision {
        note = Some(format!(
            "escalation {} is {decision} in Onus Cloud but {} on the agent's word (the door log it attached, or its own submission); Onus Cloud's decision is recorded",
            e.id, e.decision
        ));
        e.decision = decision.to_string();
        e.by = by;
        e.at = at;
        e.reason = reason;
    } else if on_word {
        e.by = by.or(e.by.take());
        e.at = at.or(e.at);
        e.reason = reason.or(e.reason.take());
    } else if is_final(&e.decision) && is_final(decision) && e.decision != decision {
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
    // The token and escalations come from the agent's own submission when
    // one was handed in beside the run's.
    let held = i.agents.or(sub);
    // A door log that verifies, and whether it is a slice attached to the
    // work rather than one this run read.
    let intact = i.audit.as_ref().is_some_and(|(_, ok)| *ok);
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
    // A slice cut at a commit that is not one of the change's may be
    // another change's: it says nothing of this one's task, token or
    // escalations. Nor does one that does not say where it was cut (the
    // commit, or "" for none). Unchecked when the change's commits are not
    // known.
    let commits_known = !commits.head.is_empty() || !i.commits.is_empty();
    let foreign_slice: Option<&str> = i.audit_slice.filter(|_| commits_known).and_then(|_| match i
        .audit_head
    {
        Some(h) if is_change_commit(h, i.change, &commits.head, i.commits, i.pulls) => None,
        Some(h) => Some(h),
        None => Some(""),
    });
    let slice_task = i.audit_slice.filter(|_| intact && foreign_slice.is_none());

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
    let listed: Vec<&str> = held
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
    // The agent's own submission ranks below the door's word.
    let scope_task =
        |s: Option<&Submission>| s.and_then(|s| s.scope.as_ref()).map(|s| s.task.clone());
    let task: Option<String> = i
        .task
        .map(str::to_string)
        .or_else(|| i.token.as_ref().map(|t| t.task.clone()))
        .or_else(|| scope_task(sub.filter(|_| i.agents.is_none())))
        .or_else(|| i.inspection.as_ref().map(|t| t.task.clone()))
        .or_else(|| scope_task(i.agents))
        .or_else(|| from.and_then(|f| f.token.as_ref()).map(|t| t.task.clone()))
        // The task whose door log was attached to this change's work, when
        // its chain verifies.
        .or_else(|| slice_task.map(str::to_string))
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
    // Only a door log that verifies says what the token held.
    let local_entries: &[Entry] = if intact && i.audit_slice.is_none() {
        entries
    } else {
        &[]
    };
    // A slice that verifies, was cut at one of the change's commits, and is
    // of the change's task: the only one whose decisions are read.
    let slice_ok = slice_task.is_some_and(|s| task.as_deref().is_none_or(|t| t == s));
    // Where escalations are decided: the door log this run read, or an
    // attached slice that is the change's. A slice is the agent's word.
    let decided: &[Entry] = if i.audit_slice.is_none() || slice_ok {
        entries
    } else {
        &[]
    };
    let attached = || -> Option<Token> {
        let t = slice_task.filter(|t| task.as_deref().is_none_or(|task| task == *t))?;
        let mut token = match i.attached_inspection.clone().filter(|x| x.task == t) {
            Some(x) => token_from_inspection(x, decisions),
            None => token_from_audit(entries, t)?,
        };
        token.source = TokenSource::Attached;
        Some(token)
    };
    let held_token = held.and_then(|s| s.scope.as_ref()).map(|s| Token {
        task: s.task.clone(),
        rights: s.rights.clone(),
        attenuations: s.attenuations.clone(),
        expires: s.expires,
        fingerprint: s.fingerprint.clone(),
        source: match i.agents {
            Some(_) => TokenSource::AgentSubmission,
            None => TokenSource::Submission,
        },
    });
    // The agent's own submission is on its word: it ranks below the door's.
    let (own_token, agents_token) = match i.agents {
        Some(_) => (None, held_token),
        None => (held_token, None),
    };
    let token = i
        .token
        .clone()
        .or(own_token)
        .or_else(|| {
            task.as_deref()
                .and_then(|t| token_from_audit(local_entries, t))
        })
        // A token verified earlier outranks the door's word on a refresh.
        .or_else(|| {
            from.and_then(|f| f.token.clone()).filter(|t| {
                !matches!(
                    t.source,
                    TokenSource::Inspection | TokenSource::AgentSubmission | TokenSource::Attached
                )
            })
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
        .or(agents_token)
        // What the door log attached to the work says, on its word.
        .or_else(attached)
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
    let local_escalations = held.is_some() || !i.requests.is_empty() || i.audit.is_some();
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
        // Whether a decision is only the agent's word: the door log it
        // attached, or the escalations its own submission lists.
        let on_word = |id: &str| match decision_of(decided, id) {
            Some(_) => i.audit_slice.is_some(),
            None => i.agents.is_some() && listed.contains(&id),
        };
        // With only Onus Cloud's list fresh, it updates the escalations kept
        // earlier, which may hold some only the door knew.
        let mut out: Vec<(Escalation, bool)> = if local_escalations {
            ids.into_iter()
                .map(|id| {
                    let e = escalation(id, i.requests, decided, listed.contains(&id));
                    let word = on_word(id) && is_final(&e.decision);
                    (e, word)
                })
                .collect()
        } else {
            from.map(|f| f.escalations.iter().map(|e| (e.clone(), false)).collect())
                .unwrap_or_default()
        };
        // Onus Cloud's decisions: on the request they are about, else as
        // escalations of their own. Onus Cloud's word outranks the agent's.
        for d in decisions {
            let at = match out.iter().position(|(e, _)| is_about(d, e)) {
                Some(n) => n,
                None => {
                    let id = d
                        .client_request_id
                        .as_deref()
                        .or(d.request_id.as_deref())
                        .unwrap_or(&d.id);
                    let e = escalation(id, i.requests, decided, false);
                    let word = on_word(id) && is_final(&e.decision);
                    out.push((e, word));
                    out.len() - 1
                }
            };
            let (e, word) = &mut out[at];
            disagreements.extend(decide_from_cloud(e, d, i.now, *word));
            *word = false;
        }
        // What only the agent says was decided, and nothing here confirms.
        let unconfirmed: Vec<&str> = out
            .iter()
            .filter(|(_, word)| *word)
            .map(|(e, _)| e.id.as_str())
            .collect();
        if !unconfirmed.is_empty() {
            disagreements.push(format!(
                "the decisions on {} are the agent's word: its own submission or the door log it attached says so, and neither Onus Cloud nor a door log this run read confirms it",
                unconfirmed.join(", ")
            ));
        }
        out.into_iter().map(|(e, _)| e).collect()
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
    // The door log: read here, or the slice attached to the work; else the
    // one kept earlier. Onus Cloud's calls sit beside it, never in it.
    let kept_doors = from.and_then(|f| f.doors.as_ref());
    let log: Option<(bool, Vec<DoorEntry>, Option<DoorSource>)> = match &i.audit {
        Some((all, intact)) => {
            // A slice keeps the entries of its own task, the change's: none
            // when it is another task's or may be another change's. One that
            // does not verify keeps them, as not intact, even when nothing
            // else names the task. Decisions on the change's escalations
            // come only from a log whose decisions are read.
            let slice_of_change = i
                .audit_slice
                .is_none_or(|s| foreign_slice.is_none() && task.as_deref().is_none_or(|t| t == s));
            let of_task = task
                .as_deref()
                .or(i.audit_slice)
                .filter(|_| slice_of_change);
            let ids: BTreeSet<&str> = escalations.iter().map(|e| e.id.as_str()).collect();
            let mut picked: Vec<&Entry> = all
                .iter()
                .filter(|e| {
                    of_task.is_some_and(|t| e.actor == t || (e.action == "mint" && e.subject == t))
                        || (!decided.is_empty()
                            && matches!(e.action.as_str(), "grant" | "deny")
                            && ids.contains(e.subject.as_str()))
                })
                .collect();
            picked.sort_by_key(|e| e.seq);
            let entries = picked
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
                .collect();
            Some((
                *intact,
                entries,
                i.audit_slice.map(|_| DoorSource::Attached),
            ))
        }
        None => kept_doors
            .filter(|d| d.source != Some(DoorSource::OnusCloud))
            .map(|d| (d.intact, d.entries.clone(), d.source)),
    };
    // Onus Cloud's calls under the change's task, when it is known. They
    // never say the task: Onus Cloud's word is not the door's.
    let calls = match i.calls {
        Some(all) => {
            let mine: Vec<&CloudCall> = all
                .iter()
                .filter(|c| {
                    task.as_deref()
                        .is_none_or(|t| c.task.as_deref().is_none_or(|ct| ct == t))
                })
                .collect();
            Some(calls_summary(&mine))
        }
        None => kept_doors.and_then(|d| d.calls.clone()),
    };
    let doors = match (log, calls) {
        (Some((intact, entries, source)), calls) => Some(Doors {
            intact,
            entries,
            source,
            calls,
        }),
        (None, Some(calls)) => Some(Doors {
            intact: false,
            entries: Vec::new(),
            source: Some(DoorSource::OnusCloud),
            calls: Some(calls),
        }),
        (None, None) => None,
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
                "no task token reached this run: give --token/--key-public, --submission, --token-inspection, --audit with --task, or --audit with the door log the agent attached through onus mcp",
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
    // What the doors cannot vouch for, in one gap.
    let because = i.audit_why.map(|w| format!(" ({w})")).unwrap_or_default();
    let mut door_doubts: Vec<String> = Vec::new();
    let other_task = i
        .audit_slice
        .zip(task.as_deref())
        .filter(|(slice, t)| slice != t);
    match &doors {
        None if i.audit_why.is_some() => door_doubts.push(format!(
            "the door log could not be read{because}: give --audit with the door log, or the slice the agent attached in Onus Cloud"
        )),
        None => door_doubts.push(
            "no door log reached this run: give --audit with --task (the door log, or the slice the agent attached in Onus Cloud), --submission-id in the repository that keeps it, or --calls with Onus Cloud's calls".into(),
        ),
        Some(d) if d.source == Some(DoorSource::OnusCloud) => door_doubts.push(format!(
            "only Onus Cloud's calls are on record, on its word: the agent's own door log (its MCP and gateway calls) did not reach this run{because}. The agent attaches it with onus cloud evidence --door-log or onus_cloud_attach_evidence; give it with --audit"
        )),
        Some(d) if !d.intact && d.source == Some(DoorSource::Attached) => door_doubts.push(format!(
            "the door log attached to the work does not verify{because}: its hash chain is broken, so an entry was changed or removed"
        )),
        Some(d) if !d.intact => door_doubts.push(format!(
            "the audit log's hash chain is broken{because}: an entry was changed or removed"
        )),
        // A log that verifies but holds nothing of the change vouches for
        // nothing.
        Some(d) if i.audit.is_some() && task.is_none() => door_doubts.push(
            if d.entries.is_empty() {
                "a door log reached this run, but the change's task is not known, so none of its entries is on record: give --task".into()
            } else {
                "a door log reached this run, but the change's task is not known, so only the decisions on its escalations are on record: give --task".into()
            },
        ),
        Some(d) if d.entries.is_empty() && other_task.is_none() && foreign_slice.is_none() => door_doubts.push(match &task {
            Some(t) => format!("the door log holds no entry of task {t}: nothing it did through a door is on record"),
            None => "the door log holds no entry of the change's task".into(),
        }),
        Some(_) => {}
    }
    if i.calls.is_some_and(|c| !c.is_empty()) && task.is_none() {
        door_doubts.push(
            "the change's task is not known, so every call Onus Cloud logged that was given is counted as its: give --task".into(),
        );
    }
    match foreign_slice {
        Some("") => door_doubts.push(
            "the door log attached does not say which commit it was cut at, so it may be another change's: it gives neither the task, the token nor any decision, and none of its entries is kept".into(),
        ),
        Some(h) => {
            // The commit is named only when it looks like one.
            let at = if (7..=64).contains(&h.len()) && h.chars().all(|c| c.is_ascii_hexdigit()) {
                format!(" {h}")
            } else {
                String::new()
            };
            door_doubts.push(format!(
                "the door log attached was cut at commit{at}, which is not one of this change's, so it gives neither the task, the token nor any decision, and none of its entries is kept"
            ));
        }
        None => {}
    }
    // A slice that verifies is still the agent's word.
    if doors
        .as_ref()
        .is_some_and(|d| d.intact && d.source == Some(DoorSource::Attached))
    {
        door_doubts.push(
            "the door log is the slice the agent attached, on its word: its chain shows that nothing in it was changed after it was cut, not who cut it, so what it gives (its entries, and the task and token when nothing else does) is the agent's account".into(),
        );
    }
    if let Some((slice, t)) = other_task {
        door_doubts.push(format!(
            "the door log attached is task {slice}'s, not this change's task {t}: its entries are left out"
        ));
    }
    if !i.calls_unread.is_empty() {
        door_doubts.push(format!(
            "some of Onus Cloud's calls could not be read, so the counts may be short: {}",
            i.calls_unread.join("; ")
        ));
    }
    if !door_doubts.is_empty() {
        gap("doors", &door_doubts.join("; "));
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
    use onus_doors::audit::{AuditLog, Record};
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

    /// A task's slice of `log`, as `onus cloud evidence --door-log` sends it.
    fn slice_of(log: &AuditLog, task: &str) -> String {
        let all = log.verify().unwrap();
        let entries = onus_doors::audit::slice(&all, task);
        serde_json::json!({ "doorLog": { "task": task, "at": 9, "entries": entries } }).to_string()
    }

    #[test]
    fn an_attached_door_log_is_checked_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "local", "mcp:onus_status", "", "allowed", 1);
        entry(&log, "operator", "mint", "sms", "granted", 2);
        entry(&log, "sms", "mcp:onus_find", "billing", "allowed", 3);
        entry(&log, "other", "mcp:onus_find", "orders", "allowed", 4);
        entry(
            &log,
            "sms",
            "mcp:onus_cloud_attach_evidence",
            "acme/shop#7",
            "allowed",
            5,
        );
        let text = slice_of(&log, "sms");
        let read = parse_door_log(&text).unwrap();
        assert!(read.intact, "{:?}", read.why);
        assert_eq!(read.slice.as_deref(), Some("sms"));
        let seqs: Vec<u64> = read.entries.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [2, 3, 4, 5]);
        let build_with = |read: &DoorLog, task: Option<&'static str>| {
            build(Inputs {
                audit: Some((read.entries.clone(), read.intact)),
                audit_slice: read.slice.as_deref(),
                audit_why: read.why.as_deref(),
                task,
                ..Inputs::new(CHANGE, 9)
            })
        };
        // The slice names the task, and only the task's entries are kept.
        let r = build_with(&read, None);
        let doors = r.doors.clone().unwrap();
        assert!(doors.intact);
        assert_eq!(doors.source, Some(DoorSource::Attached));
        let seqs: Vec<u64> = doors.entries.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [2, 3, 5]);
        // A slice that verifies is still the agent's word: the gap narrows.
        let on_word = |r: &ProvenanceRecord| {
            r.gaps
                .iter()
                .filter(|g| g.field == "doors")
                .map(|g| g.reason.clone())
                .collect::<Vec<_>>()
                == [
                    "the door log is the slice the agent attached, on its word: its chain shows that nothing in it was changed after it was cut, not who cut it, so what it gives (its entries, and the task and token when nothing else does) is the agent's account",
                ]
        };
        assert!(on_word(&r), "{:?}", r.gaps);
        // A rebuild keeps it.
        let again = build(Inputs {
            from: Some(r.clone()),
            ..Inputs::new(CHANGE, 10)
        });
        assert_eq!(again.doors, r.doors);
        assert!(on_word(&again), "{:?}", again.gaps);

        // An entry changed after it was cut does not verify.
        let edited = text.replace("billing", "payments");
        let read = parse_door_log(&edited).unwrap();
        assert!(!read.intact);
        let r = build_with(&read, None);
        assert!(!r.doors.as_ref().unwrap().intact);
        // It still keeps the entries of the task it names, with no --task.
        let kept = &r.doors.as_ref().unwrap().entries;
        assert!(
            !kept.is_empty()
                && kept
                    .iter()
                    .all(|e| e.actor == "sms" || (e.action == "mint" && e.subject == "sms")),
            "{kept:?}"
        );
        let gap = r.gaps.iter().find(|g| g.field == "doors").unwrap();
        assert!(gap.reason.contains("does not verify"), "{}", gap.reason);
        assert!(
            gap.reason.contains("changed after it was written"),
            "{}",
            gap.reason
        );
        // Another task's log names the mismatch, and keeps none of its entries.
        let read = parse_door_log(&slice_of(&log, "other")).unwrap();
        assert!(read.intact);
        let r = build_with(&read, Some("sms"));
        assert!(
            r.doors
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .all(|e| e.actor != "other")
        );
        let gap = r.gaps.iter().find(|g| g.field == "doors").unwrap();
        assert!(gap.reason.contains("task other's"), "{}", gap.reason);

        // The lines of a whole log read as before; one cut at the front does not verify.
        let lines = std::fs::read_to_string(log.path()).unwrap();
        let whole = parse_door_log(&lines).unwrap();
        assert!(whole.intact && whole.slice.is_none());
        let one = parse_door_log(lines.lines().next().unwrap()).unwrap();
        assert!(one.intact && one.entries.len() == 1);
        let cut: Vec<&str> = lines.lines().skip(1).collect();
        let cut = parse_door_log(&cut.join("\n")).unwrap();
        assert!(!cut.intact);
        assert!(cut.why.unwrap().contains("starts at entry 2"));
        assert!(parse_door_log(r#"{"entries": []}"#).is_err());
        assert!(
            !parse_door_log(r#"{"task": "sms", "entries": [1]}"#)
                .unwrap()
                .intact
        );
    }

    fn calls(json: serde_json::Value) -> Vec<CloudCall> {
        let read = parse_calls(&json.to_string()).unwrap();
        assert_eq!(read.unread, Vec::<String>::new());
        read.list
    }

    #[test]
    fn onus_clouds_calls_are_summed_up_on_its_word() {
        let list = calls(serde_json::json!({ "calls": [
            { "at": 30, "method": "POST", "route": "/v1/orgs/:org/environments/:env/runs",
              "scope": "env:run", "decision": "denied", "tokenId": "tok-1", "task": "sms",
              "reason": "this token cannot run commands (env:run)" },
            { "createdAt": "1970-01-01T00:00:10.000Z", "method": "GET",
              "route": "/v1/orgs/:org/environments", "scope": "env:read",
              "decision": "allowed", "credentialId": "tok-1", "task": "sms" },
            { "at": 20, "method": "GET", "route": "/v1/orgs/:org/environments",
              "scope": "env:read", "decision": "allowed", "tokenId": "tok-2", "task": "sms" },
            { "at": 25, "method": "GET", "route": "/v1/orgs/:org/environments",
              "scope": "env:read", "decision": "failed", "tokenId": "tok-2", "task": "sms" },
            { "at": 5, "method": "GET", "route": "/v1/orgs/:org/pulls", "scope": "pulls:read",
              "decision": "denied", "tokenId": "tok-9", "task": "other" },
        ]}));
        assert_eq!(list.len(), 5);
        let r = build(Inputs {
            calls: Some(&list),
            task: Some("sms"),
            ..Inputs::new(CHANGE, 40)
        });
        let doors = r.doors.clone().unwrap();
        assert_eq!(doors.source, Some(DoorSource::OnusCloud));
        assert!(!doors.intact && doors.entries.is_empty());
        let c = doors.calls.unwrap();
        assert_eq!(c.source, CallsSource::OnusCloud);
        assert_eq!((c.total, c.allowed, c.denied, c.failed), (4, 2, 1, 1));
        assert_eq!((c.first_at, c.last_at), (Some(10), Some(30)));
        assert_eq!(c.tokens, ["tok-1", "tok-2"]);
        assert_eq!(c.routes.len(), 2);
        let read = &c.routes[0];
        assert_eq!(read.route, "/v1/orgs/:org/environments");
        assert_eq!((read.allowed, read.denied, read.failed), (2, 0, 1));
        assert_eq!(c.routes[1].scope.as_deref(), Some("env:run"));
        assert_eq!(c.denials.len(), 1);
        assert_eq!(c.denials[0].at, 30);
        assert_eq!(c.denials[0].token_id.as_deref(), Some("tok-1"));
        assert!(c.denials[0].reason.as_deref().unwrap().contains("env:run"));
        // Onus Cloud's word is not the door log: the gap stays, and says so.
        let gap = r.gaps.iter().find(|g| g.field == "doors").unwrap();
        assert!(
            gap.reason.contains("only Onus Cloud's calls"),
            "{}",
            gap.reason
        );
        assert!(gap.reason.contains("--door-log"), "{}", gap.reason);
        // Calls never say the task.
        assert!(r.token.is_none());

        // With the door log too, both are on record, and the gap says only
        // that the slice is the agent's word.
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "operator", "mint", "sms", "granted", 1);
        entry(&log, "sms", "mcp:onus_cloud_envs", "", "allowed", 2);
        let read = parse_door_log(&slice_of(&log, "sms")).unwrap();
        let r = build(Inputs {
            audit: Some((read.entries.clone(), read.intact)),
            audit_slice: read.slice.as_deref(),
            calls: Some(&list),
            ..Inputs::new(CHANGE, 40)
        });
        let doors = r.doors.clone().unwrap();
        assert_eq!(doors.source, Some(DoorSource::Attached));
        assert_eq!(doors.entries.len(), 2);
        assert_eq!(doors.calls.as_ref().unwrap().total, 4);
        let doubts: Vec<&str> = r
            .gaps
            .iter()
            .filter(|g| g.field == "doors")
            .map(|g| g.reason.as_str())
            .collect();
        assert!(
            doubts.len() == 1
                && doubts[0]
                    .starts_with("the door log is the slice the agent attached, on its word"),
            "{:?}",
            r.gaps
        );
        // A rebuild with fresh calls keeps the door log, and the other way round.
        let fresh = calls(serde_json::json!([
            { "at": 50, "method": "GET", "route": "/v1/orgs/:org/pulls", "decision": "allowed" },
        ]));
        let again = build(Inputs {
            from: Some(r.clone()),
            calls: Some(&fresh),
            ..Inputs::new(CHANGE, 60)
        });
        let d = again.doors.unwrap();
        assert_eq!(d.entries, doors.entries);
        assert_eq!(d.source, Some(DoorSource::Attached));
        assert_eq!(d.calls.unwrap().total, 1);
        // An empty list is on record: no call was made there.
        let r = build(Inputs {
            calls: Some(&[]),
            ..Inputs::new(CHANGE, 60)
        });
        assert_eq!(r.doors.unwrap().calls.unwrap().total, 0);
    }

    #[test]
    fn calls_that_cannot_be_read_are_named_never_echoed() {
        let secret = "s3cr3t-value-never-echoed";
        let read = parse_calls(
            &serde_json::json!([
                { "at": 1, "method": "GET", "route": "/", "decision": "allowed" },
                { "at": 1, "method": "GET", "route": "/", "decision": secret },
                { "at": secret, "method": "GET", "route": "/", "decision": "allowed" },
                { "method": "GET", "route": "/", "decision": "allowed" },
                7,
            ])
            .to_string(),
        )
        .unwrap();
        assert_eq!(read.list.len(), 1);
        assert_eq!(read.unread.len(), 4, "{:?}", read.unread);
        let notes = read.unread.join("\n");
        assert!(!notes.contains(secret), "{notes}");
        assert!(notes.contains("`decision`"), "{notes}");
        assert!(notes.contains("RFC 3339"), "{notes}");
        assert!(notes.contains("missing field `at`"), "{notes}");
        let r = build(Inputs {
            calls: Some(&read.list),
            calls_unread: &read.unread,
            ..Inputs::new(CHANGE, 2)
        });
        let gap = r.gaps.iter().find(|g| g.field == "doors").unwrap();
        assert!(gap.reason.contains("could not be read"), "{}", gap.reason);
        // A call with a token is left out, never taken.
        let read = parse_calls(
            r#"[{"at":1,"method":"GET","route":"/","decision":"allowed","token":"onus_tok_x"}]"#,
        )
        .unwrap();
        assert!(read.list.is_empty());
        assert!(read.unread[0].contains("`token`"), "{:?}", read.unread);
        assert!(parse_calls("{}").is_err());
    }

    #[test]
    fn an_attached_door_log_gives_the_token_only_when_it_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        // Through onus mcp, no mint is in the log: the door sends its
        // inspection of the token it holds with the slice.
        entry(&log, "sms", "mcp:onus_status", "", "allowed", 1);
        entry(
            &log,
            "sms",
            "mcp:onus_cloud_attach_evidence",
            "acme/shop#7",
            "allowed",
            2,
        );
        let all = log.verify().unwrap();
        let mut slice = serde_json::json!({ "doorLog": {
            "task": "sms", "at": 9, "entries": onus_doors::audit::slice(&all, "sms"),
            "inspection": { "task": "sms", "rights": ["write:path:a/**"], "expires": 99 },
        }});
        fn with(text: &str, inputs: Inputs) -> ProvenanceRecord {
            let read = parse_door_log(text).unwrap();
            build(Inputs {
                audit: Some((read.entries.clone(), read.intact)),
                audit_slice: read.slice.as_deref(),
                attached_inspection: read.inspection.clone(),
                ..inputs
            })
        }
        let r = with(&slice.to_string(), Inputs::new(CHANGE, 9));
        let t = r.token.clone().unwrap();
        assert_eq!(t.source, TokenSource::Attached);
        assert_eq!((t.task.as_str(), t.expires), ("sms", Some(99)));
        assert_eq!(t.rights, ["write:path:a/**"]);
        assert!(!r.gaps.iter().any(|g| g.field == "token"), "{:?}", r.gaps);
        // Onus Cloud's own inspection of the token outranks it.
        let kept = TokenInspection {
            task: "sms".into(),
            rights: vec!["write:path:b/**".into()],
            attenuations: vec![],
            expires: None,
        };
        let r = with(
            &slice.to_string(),
            Inputs {
                inspection: Some(kept),
                ..Inputs::new(CHANGE, 9)
            },
        );
        assert_eq!(r.token.unwrap().source, TokenSource::Inspection);
        // A rebuild takes a fresh one over it.
        let again = build(Inputs {
            from: Some(with(&slice.to_string(), Inputs::new(CHANGE, 9))),
            inspection: Some(TokenInspection {
                task: "sms".into(),
                rights: vec![],
                attenuations: vec![],
                expires: None,
            }),
            ..Inputs::new(CHANGE, 10)
        });
        assert_eq!(again.token.unwrap().source, TokenSource::Inspection);

        // A slice that does not verify gives neither the token nor the task.
        let edited = slice.to_string().replace("acme/shop#7", "acme/shop#8");
        let r = with(&edited, Inputs::new(CHANGE, 9));
        assert!(!r.doors.as_ref().unwrap().intact);
        assert!(r.token.is_none(), "{:?}", r.token);
        assert!(r.gaps.iter().any(|g| g.field == "token"));
        // Nor does one cut at a commit that is not the change's.
        let mut cut = slice.clone();
        cut["doorLog"]["head"] = "f00d".repeat(10).into();
        let at = |head: &str| {
            with(
                &cut.to_string(),
                Inputs {
                    audit_head: parse_door_log(&cut.to_string()).unwrap().head.as_deref(),
                    commits: &[head.to_string()],
                    ..Inputs::new(CHANGE, 9)
                },
            )
        };
        let r = at(&"f00d".repeat(10));
        assert_eq!(r.token.unwrap().source, TokenSource::Attached);
        let r = at(&"beef".repeat(10));
        assert!(r.token.is_none(), "{:?}", r.token);
        let why = &r.gaps.iter().find(|g| g.field == "doors").unwrap().reason;
        assert!(
            why.contains(&format!("cut at commit {}", "f00d".repeat(10))),
            "{why}"
        );
        // Nor does its mint entry, or a whole log's that does not verify.
        let log2 = AuditLog::new(dir.path().join("minted.jsonl"));
        entry(&log2, "operator", "mint", "sms", "granted", 1);
        entry(&log2, "sms", "mcp:onus_status", "", "allowed", 2);
        let lines = std::fs::read_to_string(log2.path()).unwrap();
        let ok = parse_door_log(&lines).unwrap();
        let tampered = parse_door_log(&lines.replace("onus_status", "onus_find")).unwrap();
        assert!(ok.intact && !tampered.intact);
        let local = |read: &DoorLog| {
            build(Inputs {
                audit: Some((read.entries.clone(), read.intact)),
                task: Some("sms"),
                ..Inputs::new(CHANGE, 9)
            })
            .token
        };
        assert_eq!(local(&ok).unwrap().source, TokenSource::Audit);
        assert!(local(&tampered).is_none());
        // An inspection of another task, or a token in its place, is never taken.
        slice["doorLog"]["inspection"]["task"] = "other".into();
        assert!(
            parse_door_log(&slice.to_string())
                .unwrap()
                .inspection
                .is_none()
        );
        slice["doorLog"]["inspection"] = "EoQBChoKBHRhc2sYAyIQCg4IBBIKCggIgQgSAxiACBIk".into();
        let err = parse_door_log(&slice.to_string()).unwrap_err();
        assert!(err.downcast_ref::<HoldsToken>().is_some());
    }

    #[test]
    fn a_slice_that_does_not_say_where_it_was_cut_gives_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "operator", "mint", "sms", "granted", 1);
        entry(&log, "sms", "mcp:onus_status", "", "allowed", 2);
        let mut slice: serde_json::Value = serde_json::from_str(&slice_of(&log, "sms")).unwrap();
        let head = "c0ffee".repeat(6);
        let with = |slice: &serde_json::Value| {
            let read = parse_door_log(&slice.to_string()).unwrap();
            build(Inputs {
                audit: Some((read.entries.clone(), read.intact)),
                audit_slice: read.slice.as_deref(),
                audit_head: read.head.as_deref(),
                commits: std::slice::from_ref(&head),
                ..Inputs::new(CHANGE, 9)
            })
        };
        let r = with(&slice);
        assert!(r.token.is_none(), "{:?}", r.token);
        assert!(r.doors.as_ref().unwrap().entries.is_empty());
        let why = &r.gaps.iter().find(|g| g.field == "doors").unwrap().reason;
        assert!(why.contains("does not say which commit"), "{why}");
        slice["doorLog"]["head"] = head.clone().into();
        let r = with(&slice);
        assert_eq!(r.token.unwrap().source, TokenSource::Attached);
        assert_eq!(r.doors.unwrap().entries.len(), 2);
    }

    #[test]
    fn the_agents_word_never_decides_against_onus_cloud() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "sms", "escalate", "e-1", "needs-person", 1);
        entry(&log, "@lead", "grant", "e-1", "granted", 2);
        entry(&log, "sms", "mcp:onus_status", "", "allowed", 3);
        let text = slice_of(&log, "sms");
        let cloud = |state: &str| {
            decisions(serde_json::json!([{
                "id": "cl-1", "clientRequestId": "e-1", "state": state, "task": "sms",
                "repository": "acme/shop", "pull": 7,
            }]))
        };
        let (waiting, granted) = (cloud("waiting"), cloud("granted"));
        let mut asked = request("sms", "write:path:a/**");
        asked.id = "e-1".into();
        let with = |text: &str, cloud: Option<&[EscalationDecision]>| {
            let read = parse_door_log(text).unwrap();
            build(Inputs {
                audit: Some((read.entries.clone(), read.intact)),
                audit_slice: read.slice.as_deref(),
                audit_why: read.why.as_deref(),
                requests: std::slice::from_ref(&asked),
                decisions: cloud,
                ..Inputs::new(CHANGE, 9)
            })
        };
        let gap = |r: &ProvenanceRecord| {
            r.gaps
                .iter()
                .find(|g| g.field == "escalations")
                .map(|g| g.reason.clone())
        };

        // A slice that does not verify decides nothing: Onus Cloud's stands,
        // and no grant of it is kept among the doors' entries.
        let forged = text.replace("grant granted", "looks fine");
        let r = with(&forged, Some(&waiting));
        assert!(!r.doors.as_ref().unwrap().intact);
        let e = &r.escalations[0];
        assert_eq!(
            (e.decision.as_str(), e.by.as_deref()),
            ("needs-person", None)
        );
        assert_eq!(e.cloud_id.as_deref(), Some("cl-1"));
        assert!(
            !r.doors.unwrap().entries.iter().any(|d| d.action == "grant"),
            "a broken slice's grant is no decision"
        );
        // One that verifies is the agent's word: Onus Cloud's decision is
        // recorded whatever its state, and the gap names the disagreement.
        let r = with(&text, Some(&waiting));
        assert_eq!(r.escalations[0].decision, "needs-person");
        assert_eq!(r.escalations[0].by, None);
        let why = gap(&r).unwrap();
        assert!(
            why.contains("e-1 is needs-person in Onus Cloud but granted on the agent's word"),
            "{why}"
        );
        // Alone, it is recorded on its word, and the gap says so.
        let r = with(&text, None);
        let e = &r.escalations[0];
        assert_eq!(
            (e.decision.as_str(), e.by.as_deref()),
            ("granted", Some("@lead"))
        );
        assert!(
            gap(&r)
                .unwrap()
                .contains("the decisions on e-1 are the agent's word")
        );
        // Onus Cloud agreeing confirms it.
        let r = with(&text, Some(&granted));
        assert_eq!(r.escalations[0].decision, "granted");
        assert_eq!(gap(&r), None);

        // The escalations the agent's own submission lists are its word too.
        let mut agents = submission();
        agents.escalations = vec!["e-1".into()];
        let listed = |cloud: Option<&[EscalationDecision]>| {
            build(Inputs {
                agents: Some(&agents),
                decisions: cloud,
                ..Inputs::new(CHANGE, 9)
            })
        };
        let r = listed(None);
        assert_eq!(r.escalations[0].decision, "granted");
        assert!(
            gap(&r)
                .unwrap()
                .contains("the decisions on e-1 are the agent's word")
        );
        let r = listed(Some(&waiting));
        assert_eq!(r.escalations[0].decision, "needs-person");
        assert!(gap(&r).unwrap().contains("on the agent's word"));
        assert_eq!(gap(&listed(Some(&granted))), None);
        // The run's own submission, verified when it was made, is not.
        let mut own = submission();
        own.escalations = vec!["e-1".into()];
        let r = build(Inputs {
            submission: Some(&own),
            ..Inputs::new(CHANGE, 9)
        });
        assert_eq!(r.escalations[0].decision, "granted");
        assert_eq!(gap(&r), None);
    }

    #[test]
    fn the_agents_submission_ranks_below_the_doors_word() {
        let mut agents = submission();
        agents.scope = Some(ScopeUsed {
            task: "sms".into(),
            rights: vec!["write:path:**".into()],
            attenuations: vec![],
            expires: None,
            fingerprint: None,
        });
        let inspection = TokenInspection {
            task: "sms".into(),
            rights: vec!["write:path:a/**".into()],
            attenuations: vec![],
            expires: None,
        };
        let r = build(Inputs {
            agents: Some(&agents),
            inspection: Some(inspection.clone()),
            ..Inputs::new(CHANGE, 9)
        });
        let t = r.token.clone().unwrap();
        assert_eq!(
            (t.source, t.rights),
            (TokenSource::Inspection, vec!["write:path:a/**".to_string()])
        );
        // A whole door log that verifies outranks it too.
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "operator", "mint", "sms", "granted", 1);
        let audit = read_audit(log.path()).unwrap();
        let r = build(Inputs {
            agents: Some(&agents),
            audit: Some(audit),
            task: Some("sms"),
            ..Inputs::new(CHANGE, 9)
        });
        assert_eq!(r.token.unwrap().source, TokenSource::Audit);
        // And a rebuild takes a fresh inspection over it.
        let agents_only = build(Inputs {
            agents: Some(&agents),
            ..Inputs::new(CHANGE, 9)
        });
        assert_eq!(
            agents_only.token.as_ref().unwrap().source,
            TokenSource::AgentSubmission
        );
        let again = build(Inputs {
            from: Some(agents_only),
            inspection: Some(inspection),
            ..Inputs::new(CHANGE, 10)
        });
        assert_eq!(again.token.unwrap().source, TokenSource::Inspection);
    }

    #[test]
    fn a_door_log_that_holds_nothing_of_the_task_is_a_gap() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        entry(&log, "operator", "mint", "sms", "granted", 1);
        entry(&log, "sms", "mcp:onus_find", "billing", "allowed", 2);
        let read = parse_door_log(&std::fs::read_to_string(log.path()).unwrap()).unwrap();
        let doors_gap = |task: Option<&'static str>| {
            let r = build(Inputs {
                audit: Some((read.entries.clone(), read.intact)),
                task,
                ..Inputs::new(CHANGE, 9)
            });
            r.gaps
                .iter()
                .find(|g| g.field == "doors")
                .map(|g| g.reason.clone())
        };
        // A whole log, and no task to pick its entries by.
        let why = doors_gap(None).unwrap();
        assert!(why.contains("task is not known"), "{why}");
        assert!(why.contains("--task"), "{why}");
        // A task the log holds nothing of.
        let why = doors_gap(Some("deploy")).unwrap();
        assert!(why.contains("no entry of task deploy"), "{why}");
        assert_eq!(doors_gap(Some("sms")), None);
        // A slice whose task has no entry in it.
        let other = AuditLog::new(dir.path().join("other.jsonl"));
        entry(&other, "other", "mcp:onus_find", "x", "allowed", 1);
        let all = other.verify().unwrap();
        let forged = serde_json::json!({ "task": "sms", "at": 9, "entries": all }).to_string();
        let read = parse_door_log(&forged).unwrap();
        assert!(read.intact);
        let r = build(Inputs {
            audit: Some((read.entries.clone(), read.intact)),
            audit_slice: Some("sms"),
            ..Inputs::new(CHANGE, 9)
        });
        let why = &r.gaps.iter().find(|g| g.field == "doors").unwrap().reason;
        assert!(why.contains("no entry of task sms"), "{why}");
    }

    #[test]
    fn onus_clouds_calls_say_what_they_cannot_tell() {
        let list = calls(serde_json::json!([
            { "at": 1, "method": "GET", "route": "/v1/orgs/:org/pulls", "decision": "allowed",
              "task": "sms" },
            { "at": 2, "method": "GET", "route": "/v1/orgs/:org/pulls", "decision": "allowed",
              "reason": "onus_cloud_attach_evidence is allowed", "tokenId": "onus_tok_abc123" },
        ]));
        let r = build(Inputs {
            calls: Some(&list),
            task: Some("sms"),
            ..Inputs::new(CHANGE, 9)
        });
        let c = r.doors.unwrap().calls.unwrap();
        assert_eq!((c.total, c.untasked), (2, 1));
        // With no task known, every call given is counted, and the gap says so.
        let r = build(Inputs {
            calls: Some(&list),
            ..Inputs::new(CHANGE, 9)
        });
        let why = &r.gaps.iter().find(|g| g.field == "doors").unwrap().reason;
        assert!(why.contains("every call Onus Cloud logged"), "{why}");

        // A call with a token in a field that is read is left out, and the
        // note never echoes it.
        let token = format!("onus_tok_{}", "A1b2C3d4E5".repeat(4));
        for call in [
            serde_json::json!({ "tokenId": token }),
            serde_json::json!({ "credentialId": format!("onus_live_{}", "x".repeat(32)) }),
            serde_json::json!({ "reason": format!("the token {token} was revoked") }),
            serde_json::json!({ "reason": "v4.public.eyJzdWIiOiJ4In0" }),
            serde_json::json!({ "tokenId": "E".repeat(120) }),
        ] {
            let mut c = serde_json::json!({
                "at": 1, "method": "GET", "route": "/", "decision": "allowed",
            });
            for (k, v) in call.as_object().unwrap() {
                c[k] = v.clone();
            }
            let ok = serde_json::json!({
                "at": 2, "method": "GET", "route": "/", "decision": "allowed",
            });
            let read = parse_calls(&serde_json::json!([c, ok]).to_string()).unwrap();
            assert_eq!(read.list.len(), 1, "{c}");
            assert!(read.unread[0].starts_with("call #1: "), "{:?}", read.unread);
            assert!(!read.unread[0].contains(&token));
        }

        // The API's rows carry more than is read: those fields are never
        // checked, so a long path, or a user agent or token label anyone
        // can set, is no reason to leave a call out.
        let id = "0b9c6a52-6f0e-4b8e-9d55-0d6c3c1f2a7e";
        let row = serde_json::json!({
            "id": "12",
            "createdAt": "2026-10-14T09:00:00Z",
            "credentialId": id,
            "credentialName": "onus_cloud_notificationsworker2026",
            "method": "POST",
            "route": "/v1/orgs/:org/environments/:env/runs/:run",
            "path": format!("/v1/orgs/{id}/environments/{id}/runs/{id}"),
            "decision": "allowed",
            "userAgent": "onus_agent_aaaaaaaaaaaaaaaaaaaaaaaa",
            "status": 200,
        });
        let read = parse_calls(&serde_json::json!({ "calls": [row] }).to_string()).unwrap();
        assert_eq!(read.list.len(), 1, "{:?}", read.unread);
        assert_eq!(read.list[0].token_id.as_deref(), Some(id));
        // A Biscuit is base64url, so a run of it with `/`, `+` or `=` inside
        // is not one.
        let mut c = ok_call();
        c["tokenId"] = format!("{}/{}", "a".repeat(60), "b".repeat(60)).into();
        assert_eq!(
            parse_calls(&serde_json::json!([c]).to_string())
                .unwrap()
                .list
                .len(),
            1
        );
    }

    fn ok_call() -> serde_json::Value {
        serde_json::json!({ "at": 1, "method": "GET", "route": "/", "decision": "allowed" })
    }

    #[test]
    fn only_the_token_and_escalations_of_the_agents_submission_are_taken() {
        let mut agents = submission();
        agents.head = "agent-head".into();
        agents.intent = Some("summary: something else".into());
        agents.evidence[0].command = "true".into();
        agents.escalations = vec!["e-1".into()];
        agents.scope = Some(ScopeUsed {
            task: "sms".into(),
            rights: vec!["write:path:a/**".into()],
            attenuations: vec![],
            expires: Some(50),
            fingerprint: None,
        });
        let mut own = submission();
        own.evidence.clear();
        own.intent = None;
        let r = build(Inputs {
            submission: Some(&own),
            agents: Some(&agents),
            ..Inputs::new(CHANGE, 9)
        });
        assert_eq!(
            r.token.as_ref().unwrap().source,
            TokenSource::AgentSubmission
        );
        assert_eq!(r.escalations.len(), 1);
        assert_eq!(r.escalations[0].decision, "granted");
        assert!(r.intent.is_none() && r.evidence.is_empty());
        // Alone, as the record rebuilt at close takes it.
        let r = build(Inputs {
            agents: Some(&agents),
            ..Inputs::new(CHANGE, 9)
        });
        assert_eq!(r.token.unwrap().task, "sms");
        assert!(r.intent.is_none() && r.report.is_none() && r.evidence.is_empty());
        assert!(r.commits.head.is_empty());
        // It is the change's only at one of its commits.
        let pull = pull(Lane::Judge, None);
        assert!(agents_submission_fits(
            &agents,
            CHANGE,
            "x",
            &["AGENT-HEAD".into()],
            &[]
        ));
        assert!(!agents_submission_fits(
            &agents,
            CHANGE,
            "x",
            &[],
            std::slice::from_ref(&pull)
        ));
        let mut at = pull;
        at.head = "agent-head".into();
        assert!(agents_submission_fits(
            &agents,
            CHANGE,
            "",
            &[],
            &[at.clone()]
        ));
        assert!(!agents_submission_fits(
            &agents,
            "acme/shop#8",
            "",
            &[],
            &[at]
        ));
    }

    #[test]
    fn the_door_log_schema_holds_every_field_of_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        log.append(
            Record {
                actor: "sms".into(),
                action: "mcp:onus_find".into(),
                subject: "billing".into(),
                decision: "allowed".into(),
                reason: "ok".into(),
                details: serde_json::json!({ "token": { "task": "sms" } }),
            },
            1,
        )
        .unwrap();
        let entries = log.verify().unwrap();
        let slice = serde_json::json!({
            "task": "sms", "at": 2, "head": "abc",
            "inspection": { "task": "sms", "rights": ["read:path:**"], "expires": 9 },
            "entries": entries,
        });
        // What the door writes reads as the published format and back, so
        // the schema drifts from the log never silently.
        let typed: onus_core::provenance::DoorLogSlice =
            serde_json::from_value(slice.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), slice);
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
