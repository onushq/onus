//! The MCP door: every call an agent makes through `onus mcp` is checked
//! against the task token the server holds, then written to the audit log
//! with who called, what they asked for, what they got and why.
//!
//! The policy is one pure function, [`authorize`], over the token, the
//! call's arguments and a few facts about the world ([`Facts`]): which task
//! made an environment, a run or a submission, and which hosted environments
//! and runs a task made (its ledger, read back from the audit log). A tool
//! without a rule is closed to a token, so a tool added later is checked
//! before it is opened. Without a token nothing is refused that was allowed
//! before, and every call is still logged, as `local`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use anyhow::{Context, Result, anyhow, bail};
use onus_doors::audit::{AuditLog, Record, args_digest, redact};
use onus_doors::scope::{Kind, Right};
use onus_doors::token::{self, AgentClaim, PublicKey, Verified, token_id};
use serde_json::{Value, json};

/// The map tools: they answer questions, filtered to what the token may read.
pub const QUERIES: &[&str] = &[
    "onus_status",
    "onus_find",
    "onus_symbol",
    "onus_dependents",
    "onus_dependencies",
    "onus_tests_for",
    "onus_owners",
    "onus_impact",
    "onus_invariants",
    "onus_component",
    "onus_file",
    "onus_check",
];

pub fn is_query(tool: &str) -> bool {
    QUERIES.contains(&tool)
}

/// The token a server holds, as given and as verified.
pub struct Held {
    pub raw: String,
    pub verified: Verified,
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the token itself.
        f.debug_struct("Held")
            .field("task", &self.verified.task)
            .field("id", &token_id(&self.raw))
            .finish()
    }
}

/// Where calls are checked and logged.
pub struct Door {
    root: PathBuf,
    log: AuditLog,
    public: Option<PublicKey>,
    held: RwLock<Option<Held>>,
    door: &'static str,
}

impl std::fmt::Debug for Door {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Door")
            .field("root", &self.root)
            .field("log", &self.log.path())
            .field("task", &self.task())
            .finish_non_exhaustive()
    }
}

/// A refused call: why, what the token holds of that kind, the rights
/// missing and how to get them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Denial {
    pub reason: String,
    /// The token's rights of the kind asked for, as a sentence.
    pub have: String,
    pub missing: Vec<Right>,
    pub next: String,
}

impl Denial {
    fn new(reason: impl Into<String>, next: impl Into<String>) -> Denial {
        Denial {
            reason: reason.into(),
            next: next.into(),
            ..Denial::default()
        }
    }

    /// What the agent reads: the reason, what it has, what to do.
    pub fn text(&self) -> String {
        let mut lines = vec![format!("denied: {}", self.reason)];
        for l in [&self.have, &self.next] {
            if !l.is_empty() {
                lines.push(l.clone());
            }
        }
        lines.join("\n")
    }
}

/// An environment as the policy needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFacts {
    /// The task whose token made it; none when made without a token.
    pub task: Option<String>,
    pub hosts: Vec<String>,
    pub secrets: Vec<String>,
}

/// What the policy needs to know besides the token. The real one asks the
/// container engine, the evidence store, the submissions and escalations
/// folders, git and the audit log; tests use a fake.
pub trait Facts {
    /// The environment named `name`; `None` when there is none.
    fn env(&self, name: &str) -> Result<Option<EnvFacts>>;
    /// The task that made environment `name`: `None` when there is no such
    /// environment, `Some(None)` when it was made without a token.
    fn env_task(&self, name: &str) -> Result<Option<Option<String>>> {
        Ok(self.env(name)?.map(|e| e.task))
    }
    /// The task whose environment recorded run `run_id`.
    fn run_task(&self, run_id: &str) -> Result<Option<String>>;
    /// The task a submission was made under.
    fn submission_task(&self, id: &str) -> Result<Option<String>>;
    /// The task an escalation request was filed for.
    fn escalation_task(&self, id: &str) -> Result<Option<String>>;
    /// The GitHub repository of this worktree (`owner/name`).
    fn worktree_repository(&self) -> Option<String>;
    /// `details.result.<field>` of the calls to `tool` that `task` made and
    /// that were done. A broken audit log is an error, never an empty set.
    fn ledger(&self, task: &str, tool: &str, field: &str) -> Result<BTreeSet<String>>;
}

/// What a task did through a door, from the audit log: `details.result.<field>`
/// of its `done` calls to `tool`.
pub fn ledger(log: &AuditLog, task: &str, tool: &str, field: &str) -> Result<BTreeSet<String>> {
    if !log.path().exists() {
        return Ok(BTreeSet::new());
    }
    let entries = log.verify().map_err(|e| {
        anyhow!(
            "the audit log is broken ({e:#}); a person must check it with onus audit {}",
            log.path().display()
        )
    })?;
    let action = format!("mcp:{tool}");
    Ok(entries
        .iter()
        .filter(|e| e.actor == task && e.action == action && e.decision == "done")
        .filter_map(|e| match &e.details["result"][field] {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .collect())
}

/// The facts of a worktree on this machine.
#[derive(Debug, Clone)]
pub struct LocalFacts {
    pub root: PathBuf,
    pub log: PathBuf,
}

impl Facts for LocalFacts {
    fn env(&self, name: &str) -> Result<Option<EnvFacts>> {
        // Without a container engine there are no environments.
        if onus_doors::runner::engine().is_err() {
            return Ok(None);
        }
        Ok(onus_env::env::list()?
            .into_iter()
            .find(|l| l.env.name == name)
            .map(|l| EnvFacts {
                task: l.env.task,
                hosts: l.env.hosts,
                secrets: l.env.secrets,
            }))
    }

    fn run_task(&self, run_id: &str) -> Result<Option<String>> {
        let store = onus_env::store::Store::for_repo(&self.root)?;
        // A run that is not in the store (or is ambiguous) belongs to no task.
        Ok(store
            .manifest(run_id)
            .ok()
            .and_then(|(_, m)| m.environment.task))
    }

    fn submission_task(&self, id: &str) -> Result<Option<String>> {
        if id.len() < 6 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let path = self
            .root
            .join(".onus/submissions")
            .join(format!("{id}.json"));
        Ok(read_json(&path).and_then(|v| v["scope"]["task"].as_str().map(str::to_string)))
    }

    fn escalation_task(&self, id: &str) -> Result<Option<String>> {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            return Ok(None);
        }
        let path = self
            .root
            .join(".onus/escalations")
            .join(format!("{id}.json"));
        Ok(read_json(&path).and_then(|v| v["task"].as_str().map(str::to_string)))
    }

    fn worktree_repository(&self) -> Option<String> {
        crate::cloud::repository_of(&self.root)
    }

    fn ledger(&self, task: &str, tool: &str, field: &str) -> Result<BTreeSet<String>> {
        ledger(&AuditLog::new(&self.log), task, tool, field)
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key].as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn strings(args: &Value, key: &str) -> Vec<String> {
    args[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `2026-10-12T18:00:00Z` for Unix seconds.
pub fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The token's rights of `kind` as a sentence (path writes count as reads).
fn have(v: &Verified, kind: Kind) -> String {
    let list: Vec<String> = v
        .rights
        .iter()
        .filter(|r| r.kind == kind || (kind == Kind::Read && r.kind == Kind::Write))
        .map(|r| format!("`{r}`"))
        .collect();
    format!(
        "Your {} rights: {}.",
        kind.as_str(),
        if list.is_empty() {
            "none".to_string()
        } else {
            list.join(", ")
        }
    )
}

/// The denial for a right the token does not allow.
fn missing(v: &Verified, right: Right, evidence: &str) -> Denial {
    Denial {
        reason: format!("`{right}` is not in the token of task `{}`.", v.task),
        have: have(v, right.kind),
        next: format!(
            "To get it: onus_escalate with scopes [\"{right}\"] and evidence ({evidence})."
        ),
        missing: vec![right],
    }
}

/// The denial for a file outside the read scope.
pub fn read_denial(v: &Verified, path: &str) -> Denial {
    let folder = match path.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/**"),
        None => path.to_string(),
    };
    Denial {
        reason: format!("`{path}` is outside the read scope of task `{}`.", v.task),
        have: have(v, Kind::Read),
        next: format!(
            "To get it: onus_escalate with scopes [\"read:path:{folder}\"] and evidence (map-path or a failing test)."
        ),
        missing: vec![Right::new(Kind::Read, path)],
    }
}

fn expired(v: &Verified) -> Denial {
    Denial::new(
        format!(
            "the token of task `{}` expired at {}.",
            v.task,
            utc(v.expires)
        ),
        "Nothing more can be done under it; ask whoever runs the task for a new one (onus token mint).",
    )
}

/// A fact the policy could not read: the call is refused, not allowed.
fn unknown(e: anyhow::Error) -> Denial {
    Denial::new(format!("{e:#}"), "")
}

fn allow(v: &Verified, kind: Kind, value: &str, now: u64, evidence: &str) -> Result<(), Denial> {
    v.authorize(kind, value, now)
        .map_err(|_| missing(v, Right::new(kind, value), evidence))
}

/// A repository named in the call must be this worktree's: the token is for it.
fn same_repository(args: &Value, facts: &dyn Facts) -> Result<(), Denial> {
    let Some(asked) = str_arg(args, "repository") else {
        return Ok(());
    };
    match facts.worktree_repository() {
        Some(r) if r.eq_ignore_ascii_case(asked) => Ok(()),
        Some(r) => Err(Denial::new(
            format!("your token is for this worktree's repository `{r}`, not `{asked}`."),
            "Leave repository out to use this worktree's.",
        )),
        None => Err(Denial::new(
            format!(
                "your token is for this worktree's repository, and this worktree has no GitHub remote, so `{asked}` cannot be checked."
            ),
            "",
        )),
    }
}

/// The environment `name` must have been made by the task, and its hosts and
/// secrets must still be in the token.
fn own_env(
    v: &Verified,
    name: &str,
    env: Option<EnvFacts>,
    now: u64,
    must_exist: bool,
) -> Result<(), Denial> {
    let make =
        "Make your own with onus_env_create; it gets the hosts and secrets your token names.";
    let Some(env) = env else {
        return if must_exist {
            Err(Denial::new(
                format!("there is no environment `{name}`."),
                "Make one with onus_env_create.",
            ))
        } else {
            Ok(())
        };
    };
    match &env.task {
        Some(t) if *t == v.task => {}
        Some(t) => {
            return Err(Denial::new(
                format!(
                    "environment `{name}` was made for task `{t}`, not `{}`.",
                    v.task
                ),
                make,
            ));
        }
        None => {
            return Err(Denial::new(
                format!(
                    "environment `{name}` was made without a token, not for task `{}`.",
                    v.task
                ),
                make,
            ));
        }
    }
    let rights = env
        .hosts
        .iter()
        .map(|h| Right::new(Kind::Net, h.clone()))
        .chain(
            env.secrets
                .iter()
                .map(|s| Right::new(Kind::Secret, s.clone())),
        );
    for r in rights {
        if v.authorize(r.kind, &r.pattern, now).is_err() {
            return Err(Denial {
                reason: format!(
                    "environment `{name}` has `{r}`, which the token of task `{}` no longer allows.",
                    v.task
                ),
                have: have(v, r.kind),
                next: "Destroy it and create a new one with onus_env_create; it gets what your token allows now.".into(),
                missing: vec![r],
            });
        }
    }
    Ok(())
}

fn own_run(v: &Verified, id: &str, facts: &dyn Facts) -> Result<(), Denial> {
    match facts.run_task(id).map_err(unknown)? {
        Some(t) if t == v.task => Ok(()),
        Some(_) => Err(Denial::new(
            format!("run `{id}` was recorded in another task's environment."),
            "Use runs from your own environments (onus_env_run).",
        )),
        None => Err(Denial::new(
            format!(
                "run `{id}` was not recorded in an environment of task `{}` (no such run, or one made without a token).",
                v.task
            ),
            "Use runs from your own environments (onus_env_run).",
        )),
    }
}

/// Whether `tool` may be called with `args` under the held token at `now`.
/// Without a token every call is allowed.
pub fn authorize(
    tool: &str,
    args: &Value,
    held: Option<&Verified>,
    now: u64,
    facts: &dyn Facts,
) -> Result<(), Denial> {
    let Some(v) = held else {
        return Ok(());
    };
    if now > v.expires {
        return Err(expired(v));
    }
    let task = v.task.as_str();
    let name = str_arg(args, "name").unwrap_or_default();
    match tool {
        // Answers are filtered to the read scope after the map answers.
        t if is_query(t) => Ok(()),
        "onus_lanes" | "onus_outcomes" | "onus_cloud_status" => Ok(()),
        // The token decides the hosts and secrets; the task is forced.
        "onus_env_create" | "onus_escalate" | "onus_blocked" => Ok(()),
        // A narrower Onus Cloud token than the one this server holds: it can only narrow.
        "onus_cloud_token" => Ok(()),
        // Answers are filtered to the read scope, as the map tools' are.
        "onus_cloud_map" => same_repository(args, facts),
        // Filtered to the task's environments afterwards.
        "onus_envs" | "onus_cloud_envs" => Ok(()),
        "onus_submit" => {
            for id in strings(args, "evidence") {
                own_run(v, &id, facts)?;
            }
            // Escalation ids are verified by the submission itself.
            Ok(())
        }
        "onus_judge" => {
            let id = str_arg(args, "id").unwrap_or_default();
            match facts.submission_task(id).map_err(unknown)? {
                Some(t) if t == task => Ok(()),
                Some(t) => Err(Denial::new(
                    format!("submission `{id}` belongs to task `{t}`."),
                    "Judge your own submissions (onus_submit gives their ids).",
                )),
                None => Err(Denial::new(
                    format!("submission `{id}` was not made under the token of task `{task}`."),
                    "Judge your own submissions (onus_submit gives their ids).",
                )),
            }
        }
        "onus_env_run" => own_env(v, name, facts.env(name).map_err(unknown)?, now, true),
        "onus_env_destroy" => own_env(v, name, facts.env(name).map_err(unknown)?, now, false),
        "onus_evidence" => match str_arg(args, "id") {
            Some(id) => own_run(v, id, facts),
            None => Ok(()),
        },
        "onus_run_test" => {
            if str_arg(args, "setup").is_none() {
                return Ok(());
            }
            let nets: Vec<String> = v
                .rights
                .iter()
                .filter(|r| r.kind == Kind::Net)
                .map(|r| format!("`{r}`"))
                .collect();
            Err(Denial::new(
                format!(
                    "`setup` runs with open network, and your token allows {}.",
                    if nets.is_empty() {
                        "no network".to_string()
                    } else {
                        format!("only {}", nets.join(", "))
                    }
                ),
                "Use onus_env_create, whose network your token decides, or run onus_run_test without setup.",
            ))
        }
        "onus_escalation" => {
            let id = str_arg(args, "id").unwrap_or_default();
            match facts.escalation_task(id).map_err(unknown)? {
                Some(t) if t != task => Err(Denial::new(
                    format!("escalation `{id}` belongs to another task."),
                    "Check your own requests (onus_escalate gives their ids).",
                )),
                _ => Ok(()),
            }
        }
        "onus_cloud_env_create" => {
            for h in strings(args, "hosts") {
                allow(v, Kind::Net, &h, now, "a failing test is strongest")?;
            }
            same_repository(args, facts)
        }
        "onus_cloud_env_run" | "onus_cloud_env_destroy" => {
            let made = facts
                .ledger(task, "onus_cloud_env_create", "name")
                .map_err(unknown)?;
            if made.contains(name) {
                Ok(())
            } else {
                Err(Denial::new(
                    format!("hosted environment `{name}` was not made by task `{task}`."),
                    "Make your own with onus_cloud_env_create.",
                ))
            }
        }
        "onus_cloud_pulls" | "onus_cloud_pull" => same_repository(args, facts),
        "onus_cloud_attach_evidence" => {
            let run = str_arg(args, "run").unwrap_or_default();
            let made = facts
                .ledger(task, "onus_cloud_env_run", "runId")
                .map_err(unknown)?;
            if !made.contains(run) {
                return Err(Denial::new(
                    format!("run `{run}` was not made by task `{task}`."),
                    "Attach runs you made with onus_cloud_env_run.",
                ));
            }
            same_repository(args, facts)
        }
        other => Err(Denial::new(
            format!("`{other}` is not a door this server checks."),
            "",
        )),
    }
}

/// What a call is about, for the log: an environment, a run, a submission,
/// a target, a path or a pull request.
pub fn subject(tool: &str, args: &Value) -> String {
    match tool {
        "onus_cloud_pull" | "onus_cloud_attach_evidence" => match args["number"].as_u64() {
            Some(n) => format!("{}#{n}", str_arg(args, "repository").unwrap_or_default()),
            None => str_arg(args, "pull").unwrap_or_default().to_string(),
        },
        "onus_submit" => format!(
            "{}..{}",
            str_arg(args, "base").unwrap_or_default(),
            str_arg(args, "head").unwrap_or("HEAD")
        ),
        "onus_escalate" | "onus_cloud_token" => strings(args, "scopes").join(" "),
        "onus_blocked" => str_arg(args, "kind").unwrap_or_default().to_string(),
        "onus_cloud_map" => {
            let q = &args["question"];
            let at = match (str_arg(args, "commit"), args["pull"].as_u64()) {
                (Some(c), _) => format!(" @{c}"),
                (None, Some(n)) => format!(" #{n}"),
                (None, None) => String::new(),
            };
            let target = ["target", "id", "path", "text"]
                .iter()
                .find_map(|k| str_arg(q, k))
                .map(|t| format!(" {t}"))
                .unwrap_or_default();
            format!("{}{target}{at}", str_arg(q, "query").unwrap_or_default())
        }
        _ => [
            "name", "id", "run", "target", "path", "text", "ref", "image",
        ]
        .iter()
        .find_map(|k| str_arg(args, k))
        .unwrap_or_default()
        .to_string(),
    }
}

/// The fields of an answer the log keeps, per tool: what a task's ledger
/// is read from. `subject` is what the call is about ([`Door::subject`]).
pub fn result_fields(tool: &str, args: &Value, subject: &str, answer: &Value) -> Value {
    match tool {
        "onus_submit" => json!({
            "id": answer["id"], "lane": answer["lane"],
            "escalations": answer["escalations"], "agent": answer["agent"],
        }),
        "onus_judge" => json!({ "verdict": answer["judgment"]["verdict"] }),
        "onus_env_create" => {
            json!({ "name": answer["name"], "hosts": answer["hosts"], "secrets": answer["secrets"] })
        }
        "onus_env_run" => {
            json!({ "runId": answer["id"], "exitCode": answer["manifest"]["exitCode"] })
        }
        "onus_escalate" => json!({ "id": answer["id"], "state": answer["state"] }),
        "onus_blocked" => json!({ "id": answer["id"], "state": answer["state"] }),
        // Never the token itself.
        "onus_cloud_token" => json!({
            "tokenId": answer["tokenId"], "scopes": answer["scopes"],
            "repositories": answer["repositories"], "expiresAt": answer["expiresAt"],
        }),
        "onus_cloud_map" => {
            json!({ "repository": answer["map"]["repository"], "commit": answer["map"]["commit"] })
        }
        "onus_escalation" => json!({ "state": answer["state"], "adopted": answer["adopted"] }),
        "onus_cloud_env_create" => json!({ "name": answer["environment"]["name"] }),
        "onus_cloud_env_run" => {
            json!({ "runId": answer["run"]["id"], "state": answer["run"]["state"] })
        }
        "onus_cloud_attach_evidence" => json!({
            "pull": subject,
            "run": args["run"],
        }),
        _ => json!({}),
    }
}

/// Keeps a log inside the worktree out of commits: an agent that runs
/// `git add -A` must not commit it (a path outside its write scope). The
/// log and its lock go in the repository's own exclude file
/// (`.git/info/exclude`), which is never committed. Best effort: outside a
/// git repository nothing is written.
fn exclude_from_git(root: &Path, log: &Path) {
    let Ok(rel) = log.strip_prefix(root) else {
        return;
    };
    let lock = log.with_extension("lock");
    let Ok(lock) = lock.strip_prefix(root) else {
        return;
    };
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<String> = [rel, lock]
        .iter()
        .map(|p| format!("/{}", p.to_string_lossy().replace('\\', "/")))
        .filter(|l| !have.lines().any(|h| h.trim() == l))
        .collect();
    if missing.is_empty() {
        return;
    }
    let mut text = have;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# The Onus audit log (onus mcp)\n");
    for l in missing {
        text.push_str(&l);
        text.push('\n');
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, text);
}

impl Door {
    /// A door for the worktree `root` that logs to `log`. With a token, the
    /// token is verified now (its expiry on every call).
    pub fn new(
        root: PathBuf,
        log: PathBuf,
        public: Option<PublicKey>,
        token: Option<String>,
    ) -> Result<Door> {
        let held = match token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
        {
            Some(raw) => {
                let key =
                    public.context("a task token needs the root public key (--key-public)")?;
                let verified = token::verify(&raw, &key)?;
                Some(Held { raw, verified })
            }
            None => None,
        };
        exclude_from_git(&root, &log);
        Ok(Door {
            root,
            log: AuditLog::new(log),
            public,
            held: RwLock::new(held),
            door: "mcp",
        })
    }

    pub fn log_path(&self) -> &Path {
        self.log.path()
    }

    /// The root public key granted tokens are verified with, if any.
    pub fn public(&self) -> Option<PublicKey> {
        self.public
    }

    /// What a call is about ([`subject`]); a pull request named by number
    /// alone is this worktree's, and is logged as `<repo>#<n>`.
    pub fn subject(&self, tool: &str, args: &Value) -> String {
        if matches!(tool, "onus_cloud_pull" | "onus_cloud_attach_evidence")
            && str_arg(args, "repository").is_none()
            && args["number"].is_u64()
            && let Some(r) = self.facts().worktree_repository()
        {
            let mut args = args.clone();
            args["repository"] = json!(r);
            return subject(tool, &args);
        }
        subject(tool, args)
    }

    /// The facts of this door's worktree.
    pub fn facts(&self) -> LocalFacts {
        LocalFacts {
            root: self.root.clone(),
            log: self.log.path().to_path_buf(),
        }
    }

    fn with_held<T>(&self, f: impl FnOnce(Option<&Held>) -> T) -> T {
        match self.held.read() {
            Ok(g) => f(g.as_ref()),
            Err(p) => f(p.into_inner().as_ref()),
        }
    }

    pub fn task(&self) -> Option<String> {
        self.with_held(|h| h.map(|h| h.verified.task.clone()))
    }

    /// The token as given, for submissions and environments.
    pub fn token(&self) -> Option<String> {
        self.with_held(|h| h.map(|h| h.raw.clone()))
    }

    /// The agent the token names, if it names one.
    pub fn agent(&self) -> Option<AgentClaim> {
        self.with_held(|h| h.and_then(|h| h.verified.agent.clone()))
    }

    /// Whether the token may read `path` now (always, without a token).
    pub fn readable(&self, path: &str) -> bool {
        self.with_held(|h| {
            h.is_none_or(|h| {
                h.verified
                    .authorize(Kind::Read, path, onus_doors::gateway::now())
                    .is_ok()
            })
        })
    }

    /// The teaching denial for a file outside the read scope.
    pub fn read_denial(&self, path: &str) -> Denial {
        self.with_held(|h| match h {
            Some(h) => read_denial(&h.verified, path),
            None => Denial::new(format!("`{path}` is outside the read scope."), ""),
        })
    }

    /// Checks a call against the held token now.
    pub fn authorize(&self, tool: &str, args: &Value, facts: &dyn Facts) -> Result<(), Denial> {
        self.with_held(|h| {
            authorize(
                tool,
                args,
                h.map(|h| &h.verified),
                onus_doors::gateway::now(),
                facts,
            )
        })
    }

    fn actor(&self) -> String {
        self.task().unwrap_or_else(|| "local".into())
    }

    fn token_json(&self) -> Value {
        self.with_held(|h| match h {
            Some(h) => json!({
                "task": h.verified.task,
                "id": token_id(&h.raw),
                "expires": h.verified.expires,
                "attenuations": h.verified.attenuations.len(),
            }),
            None => Value::Null,
        })
    }

    /// Who the agent is, and where that came from: the token, or what the
    /// agent declared when it submitted.
    fn agent_json(&self, tool: &str, args: &Value) -> Value {
        if let Some(a) = self.agent() {
            return json!({ "tool": a.tool, "model": a.model, "config": a.config, "team": a.team, "from": "token" });
        }
        if tool == "onus_submit" {
            let s = |k: &str| str_arg(args, k).unwrap_or_default().to_string();
            return json!({ "tool": s("agent_tool"), "model": s("agent_model"), "config": s("agent_config"), "team": "", "from": "declared" });
        }
        Value::Null
    }

    /// Logs one decision on a call. `extra` adds to `details`
    /// (`missing`, `hidden`, `result`, `error`).
    pub fn record(
        &self,
        tool: &str,
        args: &Value,
        subject: &str,
        decision: &str,
        reason: &str,
        extra: Value,
    ) -> Result<()> {
        let mut details = json!({
            "door": self.door,
            "argsDigest": args_digest(args),
            "args": redact(args),
            "token": self.token_json(),
            "agent": self.agent_json(tool, args),
        });
        if let (Some(d), Value::Object(more)) = (details.as_object_mut(), extra) {
            d.extend(more);
        }
        self.append(Record {
            actor: self.actor(),
            action: format!("{}:{tool}", self.door),
            subject: subject.to_string(),
            decision: decision.to_string(),
            reason: reason.to_string(),
            details,
        })
    }

    /// Appends to the log. The error names the log; the caller says what
    /// became of the call.
    fn append(&self, record: Record) -> Result<()> {
        self.log
            .append(record, onus_doors::gateway::now())
            .map(|_| ())
            .map_err(|e| {
                anyhow!(
                    "the audit log {} cannot be written: {e:#}",
                    self.log.path().display()
                )
            })
    }

    /// Uses a granted escalation's token from now on: it must verify with
    /// the root key, be for the same task, expire no later, hold every
    /// right of the token held now and keep every attenuation of it. A grant
    /// is minted from a token's first block, so a grant for a narrowed
    /// (sub-agent) token would drop the narrowing; it is not adopted.
    pub fn adopt(&self, granted: &str) -> Result<()> {
        let granted = granted.trim();
        let public = self
            .public
            .context("this server has no root public key to verify a granted token")?;
        let new = token::verify(granted, &public)
            .map_err(|_| anyhow!("the granted token does not verify with the root key"))?;
        let mut held = self
            .held
            .write()
            .map_err(|_| anyhow!("the door is poisoned"))?;
        let Some(old) = held.as_ref() else {
            bail!("this server holds no task token; start it with --token to use a granted one");
        };
        if old.raw == granted {
            return Ok(());
        }
        let extends = new.task == old.verified.task
            && new.expires <= old.verified.expires
            && old.verified.rights.iter().all(|r| new.rights.contains(r));
        if !extends {
            bail!("the granted token does not extend yours");
        }
        // Every narrowing of the held token must still be there.
        if old
            .verified
            .attenuations
            .iter()
            .any(|a| !new.attenuations.contains(a))
        {
            bail!(
                "your token was narrowed (onus token attenuate), and the granted token was minted without that narrowing, so it would give you more than your old rights plus what you asked for; ask whoever runs the task to narrow the granted token the same way and to restart onus mcp with it"
            );
        }
        let added: Vec<String> = new
            .rights
            .iter()
            .filter(|r| !old.verified.rights.contains(r))
            .map(|r| r.to_string())
            .collect();
        self.append(Record {
            actor: new.task.clone(),
            action: "adopt-token".into(),
            subject: new.task.clone(),
            decision: "allowed".into(),
            reason: "the granted token holds every right of the old one, for the same task, until no later".into(),
            details: json!({
                "door": self.door,
                "from": token_id(&old.raw),
                "to": token_id(granted),
                "added": added,
            }),
        })?;
        *held = Some(Held {
            raw: granted.to_string(),
            verified: new,
        });
        Ok(())
    }

    /// Leaves out of a list what the task did not make: its environments
    /// (`onus_envs`), its runs (`onus_evidence`) and its hosted environments
    /// (`onus_cloud_envs`). Returns how many items it left out.
    pub fn filter_list(&self, tool: &str, answer: &mut Value, facts: &dyn Facts) -> usize {
        let Some(task) = self.task() else {
            return 0;
        };
        let keep = |items: Option<&mut Vec<Value>>, ok: &dyn Fn(&Value) -> bool| -> usize {
            let Some(items) = items else { return 0 };
            let before = items.len();
            items.retain(|i| ok(i));
            before - items.len()
        };
        match tool {
            "onus_envs" => keep(answer.as_array_mut(), &|e| e["task"] == task.as_str()),
            "onus_evidence" => keep(answer["runs"].as_array_mut(), &|r| {
                r["task"] == task.as_str()
            }),
            "onus_cloud_envs" => {
                let made = facts
                    .ledger(&task, "onus_cloud_env_create", "name")
                    .unwrap_or_default();
                keep(answer["environments"].as_array_mut(), &|e| {
                    e["name"].as_str().is_some_and(|n| made.contains(n))
                })
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_doors::token::{Grant, RootKey, mint};
    use std::collections::BTreeMap;

    const NOW: u64 = 1_791_500_000;

    #[derive(Default)]
    struct Fake {
        envs: BTreeMap<String, EnvFacts>,
        runs: BTreeMap<String, String>,
        submissions: BTreeMap<String, String>,
        escalations: BTreeMap<String, String>,
        repository: Option<String>,
        cloud_envs: BTreeSet<String>,
        cloud_runs: BTreeSet<String>,
        broken: bool,
    }

    impl Facts for Fake {
        fn env(&self, name: &str) -> Result<Option<EnvFacts>> {
            Ok(self.envs.get(name).cloned())
        }
        fn run_task(&self, id: &str) -> Result<Option<String>> {
            Ok(self.runs.get(id).cloned())
        }
        fn submission_task(&self, id: &str) -> Result<Option<String>> {
            Ok(self.submissions.get(id).cloned())
        }
        fn escalation_task(&self, id: &str) -> Result<Option<String>> {
            Ok(self.escalations.get(id).cloned())
        }
        fn worktree_repository(&self) -> Option<String> {
            self.repository.clone()
        }
        fn ledger(&self, _task: &str, tool: &str, _field: &str) -> Result<BTreeSet<String>> {
            if self.broken {
                bail!(
                    "the audit log is broken (line 3: the chain is broken); a person must check it with onus audit a.jsonl"
                );
            }
            Ok(match tool {
                "onus_cloud_env_create" => self.cloud_envs.clone(),
                _ => self.cloud_runs.clone(),
            })
        }
    }

    fn rights(list: &[&str]) -> Vec<Right> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    fn mint_for(root: &RootKey, task: &str, list: &[&str], expires: u64) -> String {
        mint(
            root,
            &Grant {
                task: task.into(),
                rights: rights(list),
                expires,
                agent: None,
            },
        )
        .unwrap()
    }

    fn token(task: &str) -> Verified {
        let root = RootKey::generate();
        let t = mint_for(
            &root,
            task,
            &[
                "write:path:services/notifications/**",
                "net:host:sms.test.example",
                "secret:SMS_KEY",
            ],
            NOW + 3600,
        );
        token::verify(&t, &root.public()).unwrap()
    }

    fn facts() -> Fake {
        let env = |task: Option<&str>, hosts: &[&str]| EnvFacts {
            task: task.map(str::to_string),
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            secrets: vec![],
        };
        Fake {
            envs: [
                ("sms-1".to_string(), env(Some("sms"), &["sms.test.example"])),
                ("pay-1".to_string(), env(Some("billing-fix"), &[])),
                ("bare".to_string(), env(None, &[])),
                ("wide".to_string(), env(Some("sms"), &["evil.example"])),
            ]
            .into(),
            runs: [
                ("run-a".to_string(), "sms".to_string()),
                ("run-b".to_string(), "billing-fix".to_string()),
            ]
            .into(),
            submissions: [
                ("aaaaaa".to_string(), "sms".to_string()),
                ("bbbbbb".to_string(), "billing-fix".to_string()),
            ]
            .into(),
            escalations: [
                ("e1".to_string(), "sms".to_string()),
                ("e2".to_string(), "billing-fix".to_string()),
            ]
            .into(),
            repository: Some("acme/shop".into()),
            cloud_envs: ["c-1".to_string()].into(),
            cloud_runs: ["r-1".to_string()].into(),
            broken: false,
        }
    }

    fn check(tool: &str, args: Value) -> Result<(), Denial> {
        authorize(tool, &args, Some(&token("sms")), NOW, &facts())
    }

    fn denied(tool: &str, args: Value) -> String {
        check(tool, args).unwrap_err().text()
    }

    #[test]
    fn queries_and_reads_are_allowed_while_the_token_lives() {
        for t in QUERIES
            .iter()
            .chain(&["onus_lanes", "onus_outcomes", "onus_cloud_status"])
        {
            assert!(check(t, json!({})).is_ok(), "{t}");
        }
        assert!(check("onus_env_create", json!({ "ref": "HEAD" })).is_ok());
        assert!(check("onus_escalate", json!({ "task": "other" })).is_ok());
        assert!(check("onus_blocked", json!({ "kind": "broken-test" })).is_ok());
        assert!(check("onus_cloud_token", json!({ "scopes": ["pulls:read"] })).is_ok());
        assert!(
            check(
                "onus_cloud_map",
                json!({ "question": { "query": "status" } })
            )
            .is_ok()
        );
        assert!(
            denied(
                "onus_cloud_map",
                json!({ "repository": "acme/other", "question": { "query": "status" } })
            )
            .contains("your token is for this worktree's repository `acme/shop`")
        );
        assert!(check("onus_envs", json!({})).is_ok());
        assert!(check("onus_cloud_envs", json!({})).is_ok());
    }

    #[test]
    fn submissions_and_judgments_use_the_tasks_own_runs() {
        assert!(check("onus_submit", json!({ "evidence": ["run-a"] })).is_ok());
        let text = denied("onus_submit", json!({ "evidence": ["run-a", "run-b"] }));
        assert!(
            text.contains("run `run-b` was recorded in another task's environment"),
            "{text}"
        );
        assert!(denied("onus_submit", json!({ "evidence": ["nope"] })).contains("no such run"));
        assert!(check("onus_judge", json!({ "id": "aaaaaa" })).is_ok());
        assert!(
            denied("onus_judge", json!({ "id": "bbbbbb" }))
                .contains("submission `bbbbbb` belongs to task `billing-fix`")
        );
    }

    #[test]
    fn environments_belong_to_the_task_that_made_them() {
        assert!(
            check(
                "onus_env_run",
                json!({ "name": "sms-1", "command": "npm test" })
            )
            .is_ok()
        );
        let text = denied("onus_env_run", json!({ "name": "pay-1" }));
        assert!(
            text.starts_with(
                "denied: environment `pay-1` was made for task `billing-fix`, not `sms`."
            ),
            "{text}"
        );
        assert!(text.contains("onus_env_create"), "{text}");
        assert!(denied("onus_env_run", json!({ "name": "bare" })).contains("without a token"));
        assert!(denied("onus_env_run", json!({ "name": "gone" })).contains("no environment"));
        // A host the token no longer allows: make a new environment.
        let text = denied("onus_env_run", json!({ "name": "wide" }));
        assert!(text.contains("`net:host:evil.example`"), "{text}");
        assert!(text.contains("create a new one"), "{text}");
        let err = check("onus_env_run", json!({ "name": "wide" })).unwrap_err();
        assert_eq!(err.missing, rights(&["net:host:evil.example"]));

        // Destroying: someone else's is refused, a missing one is allowed.
        assert!(check("onus_env_destroy", json!({ "name": "sms-1" })).is_ok());
        assert!(check("onus_env_destroy", json!({ "name": "gone" })).is_ok());
        assert!(denied("onus_env_destroy", json!({ "name": "pay-1" })).contains("billing-fix"));
    }

    #[test]
    fn evidence_and_escalations_are_the_tasks_own() {
        assert!(check("onus_evidence", json!({})).is_ok());
        assert!(check("onus_evidence", json!({ "id": "run-a" })).is_ok());
        assert!(denied("onus_evidence", json!({ "id": "run-b" })).contains("run `run-b`"));
        assert!(check("onus_escalation", json!({ "id": "e1" })).is_ok());
        assert!(check("onus_escalation", json!({ "id": "unknown" })).is_ok());
        assert!(
            denied("onus_escalation", json!({ "id": "e2" }))
                .contains("escalation `e2` belongs to another task")
        );
    }

    #[test]
    fn tests_with_open_network_are_refused_under_a_token() {
        assert!(
            check(
                "onus_run_test",
                json!({ "image": "node:22", "command": "npm test" })
            )
            .is_ok()
        );
        assert!(check("onus_run_test", json!({ "setup": " ", "command": "x" })).is_ok());
        let text = denied(
            "onus_run_test",
            json!({ "setup": "npm ci", "command": "x" }),
        );
        assert!(text.contains("`setup` runs with open network"), "{text}");
        assert!(text.contains("only `net:host:sms.test.example`"), "{text}");
        assert!(text.contains("onus_env_create"), "{text}");
    }

    #[test]
    fn hosted_environments_and_runs_come_from_the_tasks_ledger() {
        assert!(
            check(
                "onus_cloud_env_create",
                json!({ "hosts": ["sms.test.example"] })
            )
            .is_ok()
        );
        let err = check(
            "onus_cloud_env_create",
            json!({ "hosts": ["evil.example"] }),
        )
        .unwrap_err();
        assert_eq!(err.missing, rights(&["net:host:evil.example"]));
        let text = err.text();
        assert_eq!(
            text,
            "denied: `net:host:evil.example` is not in the token of task `sms`.\n\
             Your net rights: `net:host:sms.test.example`.\n\
             To get it: onus_escalate with scopes [\"net:host:evil.example\"] and evidence (a failing test is strongest)."
        );
        assert!(
            check(
                "onus_cloud_env_create",
                json!({ "repository": "acme/shop" })
            )
            .is_ok()
        );
        assert!(
            denied(
                "onus_cloud_env_create",
                json!({ "repository": "acme/other" })
            )
            .contains("your token is for this worktree's repository `acme/shop`")
        );
        for t in ["onus_cloud_env_run", "onus_cloud_env_destroy"] {
            assert!(check(t, json!({ "name": "c-1" })).is_ok(), "{t}");
            assert!(
                denied(t, json!({ "name": "c-2" }))
                    .contains("hosted environment `c-2` was not made by task `sms`"),
                "{t}"
            );
        }
        assert!(check("onus_cloud_pulls", json!({})).is_ok());
        assert!(
            check(
                "onus_cloud_pull",
                json!({ "number": 3, "repository": "acme/shop" })
            )
            .is_ok()
        );
        assert!(
            check(
                "onus_cloud_pull",
                json!({ "number": 3, "repository": "x/y" })
            )
            .is_err()
        );
        assert!(
            check(
                "onus_cloud_attach_evidence",
                json!({ "number": 3, "run": "r-1" })
            )
            .is_ok()
        );
        assert!(
            denied(
                "onus_cloud_attach_evidence",
                json!({ "number": 3, "run": "r-2" })
            )
            .contains("run `r-2` was not made by task `sms`")
        );
        // A broken log is a denial, never a pass.
        let broken = Fake {
            broken: true,
            ..facts()
        };
        let err = authorize(
            "onus_cloud_env_run",
            &json!({ "name": "c-1" }),
            Some(&token("sms")),
            NOW,
            &broken,
        )
        .unwrap_err();
        assert!(err.reason.contains("the audit log is broken"), "{err:?}");
    }

    #[test]
    fn an_expired_token_and_unknown_tools_are_refused() {
        let v = token("sms");
        for t in QUERIES
            .iter()
            .chain(&["onus_submit", "onus_lanes", "onus_env_run"])
        {
            let text = authorize(t, &json!({}), Some(&v), v.expires + 1, &facts())
                .unwrap_err()
                .text();
            assert!(
                text.contains("the token of task `sms` expired at "),
                "{t}: {text}"
            );
            assert!(text.contains("onus token mint"), "{text}");
        }
        assert_eq!(utc(1_791_828_000), "2026-10-12T18:00:00Z");
        assert!(
            denied("onus_new_tool", json!({}))
                .contains("`onus_new_tool` is not a door this server checks")
        );
        // Without a token nothing is refused.
        for t in ["onus_new_tool", "onus_run_test", "onus_env_run"] {
            assert!(
                authorize(
                    t,
                    &json!({ "setup": "npm ci", "name": "pay-1" }),
                    None,
                    NOW,
                    &facts()
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn denials_name_the_missing_right_and_how_to_ask() {
        let v = token("sms");
        let d = read_denial(&v, "services/billing/src/a.ts");
        assert_eq!(d.missing, rights(&["read:path:services/billing/src/a.ts"]));
        let text = d.text();
        assert!(
            text.contains("outside the read scope of task `sms`"),
            "{text}"
        );
        assert!(
            text.contains("Your read rights: `write:path:services/notifications/**`."),
            "{text}"
        );
        assert!(
            text.contains("onus_escalate with scopes [\"read:path:services/billing/src/**\"]"),
            "{text}"
        );
    }

    fn log_entry(log: &AuditLog, actor: &str, action: &str, decision: &str, result: Value) {
        log.append(
            Record {
                actor: actor.into(),
                action: action.into(),
                subject: String::new(),
                decision: decision.into(),
                reason: String::new(),
                details: json!({ "result": result }),
            },
            1,
        )
        .unwrap();
    }

    #[test]
    fn the_ledger_holds_only_the_tasks_done_calls() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        assert!(
            ledger(&log, "sms", "onus_cloud_env_create", "name")
                .unwrap()
                .is_empty()
        );
        let create = "mcp:onus_cloud_env_create";
        log_entry(&log, "sms", create, "allowed", json!(null));
        log_entry(&log, "sms", create, "done", json!({ "name": "c-1" }));
        log_entry(
            &log,
            "billing-fix",
            create,
            "done",
            json!({ "name": "c-2" }),
        );
        log_entry(&log, "sms", create, "denied", json!({ "name": "c-3" }));
        log_entry(&log, "sms", create, "failed", json!({ "name": "c-4" }));
        log_entry(
            &log,
            "sms",
            "mcp:onus_cloud_env_run",
            "done",
            json!({ "runId": "r-1" }),
        );
        let made = ledger(&log, "sms", "onus_cloud_env_create", "name").unwrap();
        assert_eq!(made, ["c-1".to_string()].into());
        // An edited log fails closed.
        let text = std::fs::read_to_string(log.path()).unwrap();
        std::fs::write(log.path(), text.replace("c-2", "c-9")).unwrap();
        let err = ledger(&log, "sms", "onus_cloud_env_create", "name").unwrap_err();
        assert!(err.to_string().contains("the audit log is broken"), "{err}");
        assert!(err.to_string().contains("onus audit"), "{err}");
    }

    #[test]
    fn a_granted_token_is_adopted_only_when_it_extends_the_held_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = RootKey::generate();
        let held = mint_for(&root, "sms", &["write:path:a/**"], NOW + 3600);
        let door = |held: &str| {
            Door::new(
                dir.path().to_path_buf(),
                dir.path().join("audit.jsonl"),
                Some(root.public()),
                Some(held.to_string()),
            )
            .unwrap()
        };
        let d = door(&held);
        assert_eq!(d.task().as_deref(), Some("sms"));
        // Another task, a later expiry, a missing right, another key.
        let other = mint_for(
            &root,
            "other",
            &["write:path:a/**", "write:path:b/**"],
            NOW + 3600,
        );
        let later = mint_for(
            &root,
            "sms",
            &["write:path:a/**", "write:path:b/**"],
            NOW + 7200,
        );
        let narrower = mint_for(&root, "sms", &["write:path:b/**"], NOW + 3600);
        let stranger = RootKey::generate();
        let foreign = mint_for(
            &stranger,
            "sms",
            &["write:path:a/**", "write:path:b/**"],
            NOW + 3600,
        );
        for t in [&other, &later, &narrower] {
            let err = d.adopt(t).unwrap_err();
            assert!(err.to_string().contains("does not extend yours"), "{err}");
        }
        assert!(
            d.adopt(&foreign)
                .unwrap_err()
                .to_string()
                .contains("root key")
        );
        assert_eq!(d.token().as_deref(), Some(held.as_str()));
        assert!(!d.readable("b/x.ts"));

        let wider = mint_for(
            &root,
            "sms",
            &["write:path:a/**", "write:path:b/**"],
            NOW + 3600,
        );
        d.adopt(&wider).unwrap();
        assert_eq!(d.token().as_deref(), Some(wider.as_str()));
        let entries = AuditLog::new(dir.path().join("audit.jsonl"))
            .verify()
            .unwrap();
        let e = entries.last().unwrap();
        assert_eq!(
            (e.action.as_str(), e.decision.as_str()),
            ("adopt-token", "allowed")
        );
        assert_eq!(e.details["added"], json!(["write:path:b/**"]));
        assert_eq!(e.details["from"], token_id(&held));
        assert!(
            !std::fs::read_to_string(d.log_path())
                .unwrap()
                .contains(&wider)
        );
    }

    #[test]
    fn a_grant_does_not_undo_a_narrowed_token() {
        let dir = tempfile::tempdir().unwrap();
        let root = RootKey::generate();
        let parent = mint_for(
            &root,
            "sms",
            &["write:path:a/**", "write:path:c/**"],
            NOW + 3600,
        );
        let only_a = rights(&["write:path:a/**"]);
        let narrow = token::verify(&parent, &root.public())
            .unwrap()
            .attenuate(&only_a)
            .unwrap();
        let d = Door::new(
            dir.path().to_path_buf(),
            dir.path().join("audit.jsonl"),
            Some(root.public()),
            Some(narrow.clone()),
        )
        .unwrap();
        assert!(!d.readable("c/x.ts"));
        // A grant of `b/**` minted from the parent: its first block holds
        // every right of the narrowed token, but it drops the narrowing.
        let granted = mint_for(
            &root,
            "sms",
            &["write:path:a/**", "write:path:b/**", "write:path:c/**"],
            NOW + 3600,
        );
        let err = d.adopt(&granted).unwrap_err();
        assert!(err.to_string().contains("narrowed"), "{err}");
        assert_eq!(d.token().as_deref(), Some(narrow.as_str()));
        assert!(!d.readable("c/x.ts"));
        // The same grant, narrowed the same way, is adopted: it is no wider.
        let renarrowed = token::verify(&granted, &root.public())
            .unwrap()
            .attenuate(&only_a)
            .unwrap();
        d.adopt(&renarrowed).unwrap();
        assert_eq!(d.token().as_deref(), Some(renarrowed.as_str()));
        assert!(!d.readable("c/x.ts"));
        let entries = AuditLog::new(dir.path().join("audit.jsonl"))
            .verify()
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "adopt-token");
    }

    #[test]
    fn calls_are_logged_with_the_token_id_never_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let root = RootKey::generate();
        let raw = mint(
            &root,
            &Grant {
                task: "sms".into(),
                rights: rights(&["write:path:a/**"]),
                expires: 4_000_000_000,
                agent: Some(AgentClaim {
                    tool: "claude-code".into(),
                    model: "opus".into(),
                    ..Default::default()
                }),
            },
        )
        .unwrap();
        let d = Door::new(
            dir.path().to_path_buf(),
            dir.path().join("audit.jsonl"),
            Some(root.public()),
            Some(raw.clone()),
        )
        .unwrap();
        let args = json!({ "base": "main", "agent_tool": "codex" });
        d.record(
            "onus_submit",
            &args,
            "main..HEAD",
            "allowed",
            "ok",
            json!({ "hidden": 2 }),
        )
        .unwrap();
        let e = AuditLog::new(d.log_path()).verify().unwrap().pop().unwrap();
        assert_eq!(
            (e.actor.as_str(), e.action.as_str()),
            ("sms", "mcp:onus_submit")
        );
        assert_eq!(e.details["token"]["id"], token_id(&raw));
        assert_eq!(e.details["agent"]["tool"], "claude-code");
        assert_eq!(e.details["agent"]["from"], "token");
        assert_eq!(e.details["hidden"], 2);
        assert_eq!(e.details["argsDigest"], args_digest(&args));
        assert!(
            !std::fs::read_to_string(d.log_path())
                .unwrap()
                .contains(&raw)
        );

        // Without a token: local, and the declared agent.
        let local = Door::new(dir.path().into(), dir.path().join("b.jsonl"), None, None).unwrap();
        local
            .record("onus_submit", &args, "", "allowed", "ok", json!({}))
            .unwrap();
        let e = AuditLog::new(local.log_path())
            .verify()
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(e.actor, "local");
        assert_eq!(e.details["token"], Value::Null);
        assert_eq!(e.details["agent"]["from"], "declared");
        assert_eq!(e.details["agent"]["tool"], "codex");
        // A log that cannot be written refuses.
        std::fs::create_dir_all(dir.path().join("dir.jsonl")).unwrap();
        let stuck = Door::new(dir.path().into(), dir.path().join("dir.jsonl"), None, None).unwrap();
        let err = stuck
            .record("onus_find", &json!({}), "", "allowed", "ok", json!({}))
            .unwrap_err();
        assert!(err.to_string().contains("cannot be written"), "{err}");
    }

    #[test]
    fn a_log_in_the_worktree_is_kept_out_of_commits() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q"]).status.success());
        let log = dir.path().join(".onus/audit.jsonl");
        for _ in 0..2 {
            let d = Door::new(dir.path().into(), log.clone(), None, None).unwrap();
            d.record("onus_find", &json!({}), "", "allowed", "ok", json!({}))
                .unwrap();
        }
        let exclude = std::fs::read_to_string(dir.path().join(".git/info/exclude")).unwrap();
        assert_eq!(
            exclude.matches("/.onus/audit.jsonl").count(),
            1,
            "{exclude}"
        );
        assert!(exclude.contains("/.onus/audit.lock"), "{exclude}");
        let status = git(&["status", "--porcelain", "--untracked-files=all"]);
        assert_eq!(String::from_utf8_lossy(&status.stdout), "");
    }

    #[test]
    fn lists_keep_only_what_the_task_made() {
        let dir = tempfile::tempdir().unwrap();
        let root = RootKey::generate();
        let raw = mint_for(&root, "sms", &["write:path:a/**"], 4_000_000_000);
        let d = Door::new(
            dir.path().into(),
            dir.path().join("audit.jsonl"),
            Some(root.public()),
            Some(raw),
        )
        .unwrap();
        let mut envs =
            json!([{ "name": "a", "task": "sms" }, { "name": "b", "task": "x" }, { "name": "c" }]);
        assert_eq!(d.filter_list("onus_envs", &mut envs, &facts()), 2);
        assert_eq!(envs, json!([{ "name": "a", "task": "sms" }]));
        let mut runs =
            json!({ "runs": [{ "id": "1", "task": "sms" }, { "id": "2", "task": null }] });
        assert_eq!(d.filter_list("onus_evidence", &mut runs, &facts()), 1);
        let mut cloud =
            json!({ "environments": [{ "name": "c-1" }, { "name": "c-2" }], "allowance": {} });
        assert_eq!(d.filter_list("onus_cloud_envs", &mut cloud, &facts()), 1);
        assert_eq!(cloud["environments"], json!([{ "name": "c-1" }]));
        // Without a token, everything.
        let local = Door::new(dir.path().into(), dir.path().join("b.jsonl"), None, None).unwrap();
        let mut envs = json!([{ "name": "b", "task": "x" }]);
        assert_eq!(local.filter_list("onus_envs", &mut envs, &facts()), 0);
    }
}
