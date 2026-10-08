//! The mirror's hooks. `pre-receive` decides whether a push may land: every
//! ref it updates must be in the push scope, and every commit it adds must
//! change only writable paths, as plain files, on top of the snapshot. A
//! refusal names each path and says how to ask for it. `post-receive`
//! replays what landed onto the real repository.

use std::collections::BTreeMap;
use std::io::BufRead;

use anyhow::{Result, bail};

use super::{Gateway, TaskState, ZERO, changes, git_str, ls_tree, now};
use crate::audit::{AuditLog, Record};
use crate::scope::{Kind, Right};

/// One line of a hook's input: `<old> <new> <ref>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub old: String,
    pub new: String,
    pub refname: String,
}

pub fn read_updates(input: impl BufRead) -> Result<Vec<Update>> {
    let mut out = Vec::new();
    for line in input.lines() {
        let line = line?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let [old, new, refname] = parts.as_slice() {
            out.push(Update {
                old: old.to_string(),
                new: new.to_string(),
                refname: refname.to_string(),
            });
        }
    }
    Ok(out)
}

/// Why a push is refused, one reason per line, or nothing.
pub fn check_push(gw: &Gateway, state: &TaskState, updates: &[Update]) -> Result<Vec<String>> {
    let mirror = gw.mirror_path(&state.task);
    let mut problems = Vec::new();
    // The token decides, not the gateway's copy of its scope.
    let token = match gw.verify(&state.token) {
        Ok(t) if t.task == state.task => t,
        Ok(_) | Err(_) => {
            problems.push(format!("the token for task `{}` is not valid", state.task));
            return Ok(problems);
        }
    };
    let at = now();
    if at > token.expires {
        problems.push(format!("the token for task `{}` has expired", state.task));
        return Ok(problems);
    }
    let can = |kind: Kind, value: &str| token.authorize(kind, value, at).is_ok();
    // Every path of the real base, by lower case, to catch a path that
    // differs from one outside the scope only by case.
    let mut by_lower: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in ls_tree(&gw.repo, &state.base)? {
        by_lower
            .entry(e.path.to_lowercase())
            .or_default()
            .push(e.path);
    }
    for u in updates {
        if !can(Kind::Push, &u.refname) {
            problems.push(format!(
                "`{}` is not a ref this task may push; push to one of {}",
                u.refname,
                list(state.scope.of(Kind::Push))
            ));
            continue;
        }
        if u.new == ZERO {
            continue;
        }
        let commits = git_str(
            &mirror,
            &[
                "rev-list",
                "--reverse",
                "--topo-order",
                &u.new,
                "--not",
                "--all",
            ],
        )?;
        for c in commits.lines().filter(|l| !l.is_empty()) {
            let parents = git_str(&mirror, &["rev-list", "--parents", "-n", "1", c])?;
            let parents: Vec<&str> = parents.split_whitespace().skip(1).collect();
            let short = &c[..c.len().min(10)];
            let parent = match parents.as_slice() {
                [] => {
                    problems.push(format!(
                        "commit {short} starts a new history; commit on top of the snapshot"
                    ));
                    continue;
                }
                [p] => p.to_string(),
                _ => {
                    problems.push(format!(
                        "commit {short} is a merge; rebase onto the snapshot instead"
                    ));
                    continue;
                }
            };
            for ch in changes(&mirror, &parent, c)? {
                if ch.new_mode == "120000" {
                    problems.push(format!("commit {short} adds a symbolic link `{}`", ch.path));
                    continue;
                }
                if ch.new_mode == "160000" {
                    problems.push(format!("commit {short} adds a submodule `{}`", ch.path));
                    continue;
                }
                if !can(Kind::Write, &ch.path) {
                    problems.push(format!("`{}` is outside your write scope", ch.path));
                    continue;
                }
                let lower = ch.path.to_lowercase();
                if let Some(other) = by_lower
                    .get(&lower)
                    .into_iter()
                    .flatten()
                    .find(|p| **p != ch.path && !can(Kind::Write, p))
                {
                    problems.push(format!(
                        "`{}` differs only by case from `{other}`, which is outside your write scope",
                        ch.path
                    ));
                }
            }
        }
    }
    problems.sort();
    problems.dedup();
    Ok(problems)
}

/// The message a refused push shows the agent: what was refused and how to
/// ask for it.
pub fn refusal_message(state: &TaskState, problems: &[String]) -> String {
    let mut out = format!("onus: push refused for task `{}`:\n", state.task);
    for p in problems {
        out.push_str(&format!("  - {p}\n"));
    }
    out.push_str(&format!(
        "Your write scope: {}\n",
        list(state.scope.of(Kind::Write))
    ));
    if let Some(path) = problems.iter().find_map(|p| {
        let rest = p.strip_prefix('`')?;
        let (path, tail) = rest.split_once('`')?;
        tail.contains("outside your write scope").then_some(path)
    }) {
        let folder = path.rsplit_once('/').map_or("", |(d, _)| d);
        let asked = if folder.is_empty() {
            path.to_string()
        } else {
            format!("{folder}/**")
        };
        out.push_str(&format!(
            "To change it, ask with evidence (a failing test is the strongest):\n  onus escalate --task {} --scope write:path:{asked} --evidence failing-test:<test file> --reason \"…\"\n",
            state.task
        ));
    }
    out
}

fn list<'a>(rights: impl Iterator<Item = &'a Right>) -> String {
    let all: Vec<String> = rights.map(|r| format!("`{r}`")).collect();
    if all.is_empty() {
        "none".into()
    } else {
        all.join(", ")
    }
}

/// `onus gateway hook pre-receive`: exits non-zero (refusing the push) with
/// the refusal on stderr, which git shows the pusher.
pub fn pre_receive(gw: &Gateway, task: &str, input: impl BufRead) -> Result<()> {
    let state = gw.load_task(task)?;
    let updates = read_updates(input)?;
    let problems = check_push(gw, &state, &updates)?;
    let log = AuditLog::new(gw.audit_path());
    let refs: Vec<&str> = updates.iter().map(|u| u.refname.as_str()).collect();
    if problems.is_empty() {
        log.append(
            Record {
                actor: task.into(),
                action: "push".into(),
                subject: refs.join(" "),
                decision: "allowed".into(),
                reason: "every changed path is in the write scope".into(),
                details: serde_json::json!({ "updates": updates.iter().map(|u| [&u.old, &u.new, &u.refname]).collect::<Vec<_>>() }),
            },
            now(),
        )?;
        return Ok(());
    }
    log.append(
        Record {
            actor: task.into(),
            action: "push".into(),
            subject: refs.join(" "),
            decision: "refused".into(),
            reason: problems.join("; "),
            details: serde_json::Value::Null,
        },
        now(),
    )?;
    bail!("{}", refusal_message(&state, &problems))
}

/// `onus gateway hook post-receive`: replays the landed commits onto the
/// real repository and pushes them.
pub fn post_receive(gw: &Gateway, task: &str, input: impl BufRead) -> Result<Vec<String>> {
    let state = gw.load_task(task)?;
    let log = AuditLog::new(gw.audit_path());
    let mut notes = Vec::new();
    for u in read_updates(input)? {
        let outcome = if u.new == ZERO {
            super::replay::delete_upstream(gw, &u.refname)
                .map(|_| format!("deleted {} upstream", u.refname))
        } else {
            super::replay::replay_and_push(gw, &state, &u.refname).map(|real| {
                format!(
                    "{} pushed upstream as {} (from {})",
                    u.refname,
                    &real[..12],
                    &u.new[..12]
                )
            })
        };
        let (decision, reason) = match &outcome {
            Ok(note) => ("replayed", note.clone()),
            Err(e) => ("failed", format!("{e:#}")),
        };
        log.append(
            Record {
                actor: task.into(),
                action: "replay".into(),
                subject: u.refname.clone(),
                decision: decision.into(),
                reason: reason.clone(),
                details: serde_json::Value::Null,
            },
            now(),
        )?;
        notes.push(format!("onus: {reason}"));
    }
    Ok(notes)
}
