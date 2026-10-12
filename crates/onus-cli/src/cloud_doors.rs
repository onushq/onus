//! Where an escalation goes, for `onus mcp` and `onus cloud escalate`: to Onus Cloud
//! when this process is signed in to it, else to the local policy when the door holds
//! the root key, else into the escalations folder for `onus ui`.
//!
//! Onus Cloud only says yes, no or here is an answer. Every fact (blast radius,
//! sensitive labels, owners, reproduction) is the binary's, and every token is minted
//! here, by the door that holds the root key, from the request file it kept: the
//! original rights plus exactly the request's scopes, never anything the cloud's answer
//! names. A request file changed after it was filed, or whose scopes are not the ones
//! Onus Cloud shows as granted, mints nothing. The cloud never sees the task token,
//! only its inspection (task, rights, expiry).
//!
//! The local policy decides only from a trusted checkout (`onus mcp --trusted-repo`):
//! the agent can edit its own worktree, and with it the map, the sensitive labels and
//! the test command a decision rests on.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use onus_cli::cloud::{self, CloudDoor};
use onus_doors::audit::{AuditLog, Record};
use onus_doors::escalation::{EscalationKind, Request, verify_granted};
use onus_doors::gateway::now;
use onus_doors::token::{self, PublicKey, Verified};
use serde_json::{Value, json};

use crate::doors::{self, check_intact};
use crate::ui::{State, api};

/// What `onus_blocked` tells the agent to do once it has reported.
pub const BLOCKED_NEXT: &str = "Stop here. Do not work around it, weaken the test or skip it: that is what the judge rejects. A person answers; check with onus_escalation. Your report and its evidence are kept with the task.";

/// The door an escalation goes through.
pub struct EscalationDoor<'a> {
    pub state: &'a State,
    /// The agent's task token (`onus mcp --token`): verified here, never sent.
    pub token: Option<String>,
    /// The root public key that verifies it.
    pub public: Option<PublicKey>,
    /// The root private key this door holds (`onus mcp --key`): grants are minted here.
    pub key: Option<&'a Path>,
    /// A checkout whose files the agent cannot edit (`onus mcp --trusted-repo`): the local policy
    /// takes the request's facts and the test command from it.
    pub trusted: Option<&'a Path>,
    pub cloud: Option<&'a CloudDoor>,
    /// `owner/name` for Onus Cloud.
    pub repository: Option<String>,
    /// The pull request the request is about, if any.
    pub pull: Option<u64>,
}

/// Where a request goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Filed in Onus Cloud, which assesses it on the default branch; the local policy
    /// does not run.
    Cloud,
    /// Decided here by policy from the trusted checkout (reproducing failing tests),
    /// and minted on a grant.
    Policy,
    /// Kept in the escalations folder, for a person in `onus ui`.
    File,
}

/// Signed in: Onus Cloud. Else with a root key, a task token and a trusted checkout:
/// the local policy. Else: the file only, for a person.
pub fn plan(has_cloud: bool, has_key: bool, has_token: bool, has_trusted: bool) -> Route {
    match (has_cloud, has_key && has_token && has_trusted) {
        (true, _) => Route::Cloud,
        (false, true) => Route::Policy,
        (false, false) => Route::File,
    }
}

/// A cloud answer must be about the request kept here.
pub fn check_answer(id: &str, escalation: &Value) -> Result<()> {
    match escalation["clientRequestId"].as_str() {
        Some(c) if c == id => Ok(()),
        other => bail!(
            "Onus Cloud answered about request {} when {id} was asked for; the answer is refused",
            other.unwrap_or("(none)")
        ),
    }
}

/// The scopes Onus Cloud shows for a request.
fn shown_scopes(escalation: &Value) -> Result<Vec<&str>> {
    Ok(escalation["scopes"]
        .as_array()
        .or(escalation["input"]["scopes"].as_array())
        .context("Onus Cloud's grant names no scopes, so nothing is minted")?
        .iter()
        .filter_map(Value::as_str)
        .collect())
}

/// A grant in Onus Cloud must be of exactly the scopes the request kept here asks for.
pub fn check_scopes(req: &Request, escalation: &Value) -> Result<()> {
    doors::same_scopes(req, shown_scopes(escalation)?).map_err(|e| anyhow!("in Onus Cloud, {e}"))
}

/// Wraps `s` in single quotes for a shell.
fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Who decided in Onus Cloud: `system:policy` for the policy, whatever name it
/// carries, so a policy grant is never logged as a person's; else their name, else
/// their id.
fn decided_by(escalation: &Value) -> String {
    let by = &escalation["decision"]["by"];
    if by["id"] == "system:policy" {
        return "system:policy".into();
    }
    by["name"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .or(by["id"].as_str().filter(|n| !n.trim().is_empty()))
        .unwrap_or("a person")
        .to_string()
}

/// What an agent reads for a cloud state.
fn cloud_label(state: &str) -> &str {
    match state {
        "assessing" => "being checked by Onus Cloud",
        "waiting" => "waiting for a person in Onus Cloud",
        "resolved" => "answered",
        other => other,
    }
}

/// States after which nothing changes.
pub fn is_final(state: &str) -> bool {
    matches!(
        state,
        "granted" | "denied" | "answered" | "withdrawn" | "expired" | "failed"
    )
}

impl EscalationDoor<'_> {
    fn verified(&self) -> Result<Option<Verified>> {
        match (&self.token, self.public) {
            (Some(t), Some(k)) => Ok(Some(token::verify(t, &k)?)),
            (Some(_), None) => bail!("a task token needs the root public key (--key-public)"),
            _ => Ok(None),
        }
    }

    fn append(&self, record: Record) -> Result<()> {
        if let Some(d) = self.state.audit.parent() {
            std::fs::create_dir_all(d)?;
        }
        AuditLog::new(&self.state.audit).append(record, now())?;
        Ok(())
    }
}

fn cloud_file(request: &Path) -> PathBuf {
    request.with_extension("cloud.json")
}

/// Where request `id` was filed in Onus Cloud (`<id>.cloud.json`), if it was.
pub fn filed(state: &State, id: &str) -> Option<Value> {
    let path = api::request_path(state, id).ok()?;
    serde_json::from_str(&std::fs::read_to_string(cloud_file(&path)).ok()?).ok()
}

/// Logs a withdrawal made from the command line, so the audit log has it at once
/// rather than at the next look. Nothing is logged for a request not kept here, or
/// for an answer that is not this request's withdrawal.
pub fn log_withdrawn(state: &State, id: &str, escalation: &Value) -> Result<()> {
    let Ok(path) = api::request_path(state, id) else {
        return Ok(());
    };
    let Ok(req) = doors::load_request(&path) else {
        return Ok(());
    };
    if escalation["state"] != "withdrawn" {
        return Ok(());
    }
    check_answer(&req.id, escalation)?;
    if let Some(d) = state.audit.parent() {
        std::fs::create_dir_all(d)?;
    }
    AuditLog::new(&state.audit).append(
        Record {
            actor: req.task.clone(),
            action: "escalate".into(),
            subject: req.id.clone(),
            decision: "withdrawn".into(),
            reason: "withdrawn in Onus Cloud".into(),
            details: json!({ "escalationId": escalation["id"], "webUrl": escalation["webUrl"] }),
        },
        now(),
    )?;
    Ok(())
}

/// Files a request: keeps it in the escalations folder, then sends it where
/// [`plan`] says. Returns what the agent reads.
pub fn route(door: &EscalationDoor, req: Request) -> Result<Value> {
    let verified = door.verified()?;
    let route = plan(
        door.cloud.is_some(),
        door.key.is_some(),
        verified.is_some(),
        door.trusted.is_some(),
    );
    // The policy decides on facts from the trusted checkout, not the agent's worktree.
    let req = match (route, door.trusted) {
        (Route::Policy, Some(trusted)) => doors::request_at(trusted, &req)?,
        _ => req,
    };
    std::fs::create_dir_all(&door.state.escalations)?;
    let path = api::request_path(door.state, &req.id)?;
    std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&req)?))
        .with_context(|| format!("cannot write {}", path.display()))?;
    let mut out = json!({
        "id": req.id,
        "task": req.task,
        "kind": req.kind,
        "blastRadius": req.blast_radius,
        "sensitive": req.sensitive,
        "owners": req.owners,
        "evidence": req.evidence,
    });
    match route {
        Route::Cloud => {
            let cloud = door.cloud.context("no cloud door")?;
            let kept = |e: anyhow::Error| {
                e.context(format!(
                    "escalation {} is kept in {}, but it could not be filed in Onus Cloud",
                    req.id,
                    path.display()
                ))
            };
            let client = cloud.client().map_err(kept)?;
            let repository = door.repository.clone().or_else(|| cloud.repository()).context(
                "this worktree has no GitHub remote: name the repository (--cloud-repo owner/name, or ONUS_CLOUD_REPO)",
            )?;
            let reference = cloud::pushed_head(&door.state.root).map_err(kept)?;
            let mut body = json!({
                "repository": repository,
                "ref": reference,
                "kind": req.kind,
                "reason": req.reason,
                "scopes": req.scopes.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                "evidence": req.evidence.iter().map(|e| format!("{}:{}", e.kind, e.reference)).collect::<Vec<_>>(),
                "task": req.task,
                "clientRequestId": req.id,
                // What the task token holds, never the token.
                "token": verified.as_ref().map(|v| json!({
                    "task": v.task,
                    "rights": v.rights.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                    "expires": v.expires,
                })),
            });
            if let Some(n) = door.pull {
                body["pull"] = json!(n);
            }
            let answer = cloud::file_escalation(&client, &body).map_err(kept)?;
            let e = &answer["escalation"];
            check_answer(&req.id, e)?;
            let filed = json!({
                "url": client.url,
                "organisation": client.org,
                "repository": repository,
                "escalationId": e["id"],
                "webUrl": e["webUrl"],
                "filedAt": now(),
            });
            std::fs::write(
                cloud_file(&path),
                format!("{}\n", serde_json::to_string_pretty(&filed)?),
            )?;
            door.append(Record {
                actor: req.task.clone(),
                action: "escalate".into(),
                subject: req.id.clone(),
                decision: "sent-to-onus-cloud".into(),
                reason: "filed in Onus Cloud".into(),
                details: json!({ "escalationId": e["id"], "webUrl": e["webUrl"] }),
            })?;
            out["state"] = json!(cloud_label(e["state"].as_str().unwrap_or("assessing")));
            out["url"] = e["webUrl"].clone();
            out["escalationId"] = e["id"].clone();
            if e["owners"].as_array().is_some_and(|o| !o.is_empty()) {
                out["owners"] = e["owners"].clone();
            }
            out["next"] = match e["next"].as_str() {
                Some(n) => json!(n),
                None => json!(
                    "Onus Cloud checks it against the default branch, then a person (or the policy) decides. Check with onus_escalation."
                ),
            };
        }
        Route::Policy => {
            let (v, key, trusted) = (
                verified.context("no task token")?,
                door.key.context("no root key")?,
                door.trusted.context("no trusted checkout")?,
            );
            // The agent's commit, run with the trusted checkout's test command; the
            // commit resolves there when the checkout shares the repository.
            let has_test = req.evidence.iter().any(|e| e.grade == 1);
            let (mut reproduce, mut why_not) = doors::trusted_reproduce(
                trusted,
                cloud::commit_of(&door.state.root, "HEAD"),
                None,
                None,
                None,
                has_test,
            )?;
            if reproduce.is_some() && onus_doors::runner::engine().is_err() {
                reproduce = None;
                why_not = Some("no container engine here: failing tests are not reproduced".into());
            }
            if let Some(d) = door.state.audit.parent() {
                std::fs::create_dir_all(d)?;
            }
            let decided = doors::decide_request(
                req,
                &v,
                key,
                reproduce.as_ref(),
                20,
                Some(&door.state.audit),
            )?;
            if let Some(t) = decided["token"].as_str() {
                doors::keep_token(&path, t)?;
            }
            let granted = decided["decision"] == "granted";
            let mut reasons = decided["reasons"].as_array().cloned().unwrap_or_default();
            reasons.extend(why_not.map(Value::String));
            out["evidence"] = decided["evidence"].clone();
            out["reasons"] = json!(reasons);
            out["state"] = json!(if granted {
                "granted"
            } else {
                "waiting for a person"
            });
            out["next"] = json!(if granted {
                "Granted by policy. Call onus_escalation to use it: your old rights plus exactly what you asked for."
            } else {
                "A person decides (onus ui, or onus escalation grant). Check with onus_escalation."
            });
        }
        Route::File => {
            out["state"] = json!("open");
            out["next"] = json!("A person decides (onus ui). Check with onus_escalation.");
        }
    }
    Ok(out)
}

/// Mirrors a cloud decision into the audit log, minting on a grant when this door
/// holds the root key and the task token.
fn mirror(
    door: &EscalationDoor,
    req: &Request,
    path: &Path,
    e: &Value,
    verified: Option<&Verified>,
) -> Result<()> {
    let by = format!("{} (Onus Cloud)", decided_by(e));
    let said = e["decision"]["answer"]
        .as_str()
        .or(e["error"].as_str())
        .unwrap_or_default()
        .to_string();
    let details = json!({ "escalationId": e["id"], "webUrl": e["webUrl"] });
    match e["state"].as_str().unwrap_or_default() {
        "granted" => {
            if let (Some(key), Some(v)) = (door.key, verified) {
                check_scopes(req, e)?;
                if let Some(d) = door.state.audit.parent() {
                    std::fs::create_dir_all(d)?;
                }
                // The original rights plus the scopes of the request kept here.
                let minted = doors::grant_request(req, v, key, &by, Some(&door.state.audit))?;
                doors::keep_token(path, &minted)?;
            }
        }
        "denied" => {
            if let Some(d) = door.state.audit.parent() {
                std::fs::create_dir_all(d)?;
            }
            doors::deny_request(req, &by, &said, Some(&door.state.audit))?;
        }
        "resolved" => door.append(Record {
            actor: by,
            action: "answer".into(),
            subject: req.id.clone(),
            decision: "answered".into(),
            reason: said,
            details,
        })?,
        s @ ("withdrawn" | "expired" | "failed") => door.append(Record {
            actor: req.task.clone(),
            action: "escalate".into(),
            subject: req.id.clone(),
            decision: s.into(),
            reason: if said.is_empty() {
                e["next"].as_str().unwrap_or(s).to_string()
            } else {
                said
            },
            details,
        })?,
        _ => {}
    }
    Ok(())
}

/// Where request `id` stands. A request filed in Onus Cloud that is not decided here
/// yet is asked about (waiting up to `wait` seconds), and a decision there is mirrored
/// into the audit log, minting the grant when this door holds the root key. A granted
/// token kept beside the request is in `token`.
pub fn follow(door: &EscalationDoor, id: &str, wait: u64) -> Result<Value> {
    let path = api::request_path(door.state, id)?;
    let req = doors::load_request(&path).map_err(|_| anyhow!("no escalation {id}"))?;
    check_intact(id, &req)?;
    let verified = door.verified()?;
    if let Some(v) = &verified
        && v.task != req.task
    {
        bail!("escalation {id} belongs to another task");
    }
    // The kept token proves a grant on its own: it is root-signed and names this
    // request, wherever the grant was logged. `None` when there is no key to check it.
    let proof = |kept: &Option<String>| match (kept, door.public) {
        (Some(t), Some(k)) => Some(verify_granted(&req, t, &k)),
        _ => None,
    };
    let proven = |kept: &Option<String>| matches!(proof(kept), Some(Ok(_)));
    let filed: Option<Value> = std::fs::read_to_string(cloud_file(&path))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let mut view = Value::Null;
    if let Some(f) = &filed {
        let decided = api::decisions(door.state)
            .get(id)
            .and_then(|d| d["decision"].as_str().map(str::to_string));
        let settled =
            proven(&api::kept_token(door.state, id)) || decided.as_deref().is_some_and(is_final);
        if !settled {
            let cloud = door.cloud.context(
                "this request was filed in Onus Cloud, and this process is not signed in to it: run onus login, or set ONUS_CLOUD_TOKEN",
            )?;
            let client = cloud.client()?;
            let remote = f["escalationId"]
                .as_str()
                .context("the request's Onus Cloud record has no escalation id")?;
            let answer = cloud::escalation(&client, remote, wait)?;
            view = answer["escalation"].clone();
            check_answer(id, &view)?;
            mirror(door, &req, &path, &view, verified.as_ref())?;
        }
    }

    let decision = api::decisions(door.state).remove(id);
    let kept = api::kept_token(door.state, id);
    let local = decision
        .as_ref()
        .and_then(|d| d["decision"].as_str())
        .unwrap_or_default();
    let remote = view["state"].as_str();
    let state = match local {
        _ if proven(&kept) => "granted",
        "granted" | "denied" | "answered" | "withdrawn" | "expired" | "failed" => local,
        "needs-person" => "waiting for a person",
        "sent-to-onus-cloud" => match remote {
            Some("assessing") => "being checked by Onus Cloud",
            Some("granted") => "granted",
            _ => "waiting for a person in Onus Cloud",
        },
        _ => "open",
    };
    let mut out = json!({ "id": id, "state": state, "request": req, "decision": decision });
    if let Some(f) = &filed {
        out["cloud"] = json!({
            "escalationId": f["escalationId"],
            "url": f["webUrl"],
            "state": view["state"],
            "owners": view["owners"],
            "next": view["next"],
        });
        out["url"] = f["webUrl"].clone();
    }
    match state {
        // A kept token is handed out only when it proves this grant, or when this door
        // has no root public key to check it with (and so no task token to widen).
        "granted" => match (&kept, proof(&kept)) {
            (Some(t), Some(Ok(_)) | None) => out["token"] = json!(t),
            (Some(_), Some(Err(why))) => {
                out["next"] = json!(format!(
                    "The token kept beside it cannot be used: {why}. The operator mints it again: onus escalation grant {} --token … --key-public … --key …",
                    path.display()
                ))
            }
            (None, _) if verified.is_none() => {
                out["next"] = json!(
                    "Granted in Onus Cloud, but this door holds no task token to widen: whoever runs the task mints one."
                )
            }
            // The command names the request and the scopes Onus Cloud granted, so the
            // operator mints only those, whatever the file holds by then.
            (None, _) => match check_scopes(&req, &view) {
                Ok(()) => {
                    let by = decided_by(&view);
                    let scopes: String = req
                        .scopes
                        .iter()
                        .map(|r| format!(" --scope {}", quoted(&r.to_string())))
                        .collect();
                    out["next"] = json!(format!(
                        "granted by {by} in Onus Cloud; the operator mints it: onus escalation grant {} --id {id}{scopes} --by {} --token … --key-public … --key …",
                        path.display(),
                        quoted(&format!("{by} (Onus Cloud)")),
                    ));
                }
                Err(why) => out["next"] = json!(format!("{why:#}")),
            },
        },
        "denied" => {
            out["reason"] = decision
                .as_ref()
                .map_or(Value::Null, |d| d["reason"].clone());
        }
        "answered" => {
            out["answer"] = decision
                .as_ref()
                .map_or(Value::Null, |d| d["reason"].clone());
        }
        "being checked by Onus Cloud" | "waiting for a person in Onus Cloud" => {
            out["next"] = match view["next"].as_str() {
                Some(n) => json!(n),
                None => json!("Ask again with onus_escalation (wait up to 50 seconds)."),
            };
        }
        _ => {}
    }
    Ok(out)
}

/// The kinds `onus_blocked` reports.
pub fn blocked_kind(kind: &str) -> Result<EscalationKind> {
    match kind {
        "broken-test" => Ok(EscalationKind::BrokenTest),
        "contradictory-spec" => Ok(EscalationKind::ContradictorySpec),
        "impossible-task" => Ok(EscalationKind::ImpossibleTask),
        "permission" => bail!(
            "onus_blocked reports broken-test, contradictory-spec or impossible-task; ask for rights with onus_escalate"
        ),
        other => bail!(
            "`{other}` is not a kind of block: broken-test, contradictory-spec or impossible-task"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_in_requests_go_to_onus_cloud_and_never_to_the_local_policy() {
        for key in [false, true] {
            for token in [false, true] {
                for trusted in [false, true] {
                    assert_eq!(
                        plan(true, key, token, trusted),
                        Route::Cloud,
                        "{key} {token} {trusted}"
                    );
                }
            }
        }
        assert_eq!(plan(false, true, true, true), Route::Policy);
        // Without a trusted checkout, the agent's worktree would give the facts.
        assert_eq!(plan(false, true, true, false), Route::File);
        assert_eq!(plan(false, true, false, true), Route::File);
        assert_eq!(plan(false, false, true, true), Route::File);
        assert_eq!(plan(false, false, false, false), Route::File);
    }

    #[test]
    fn a_cloud_answer_about_another_request_is_refused() {
        assert!(check_answer("a1b2c3", &json!({ "clientRequestId": "a1b2c3" })).is_ok());
        let err = check_answer("a1b2c3", &json!({ "clientRequestId": "ffffff" })).unwrap_err();
        assert!(err.to_string().contains("is refused"), "{err}");
        assert!(check_answer("a1b2c3", &json!({})).is_err());
    }

    fn request(scope: &str) -> Request {
        Request::new(
            "sms",
            EscalationKind::Permission,
            vec![scope.parse().unwrap()],
            vec![],
            "billing formats the SMS",
            None,
            &[],
            1,
        )
    }

    #[test]
    fn a_changed_request_or_a_grant_of_other_scopes_mints_nothing() {
        let r = request("write:path:services/billing/**");
        assert!(check_intact(&r.id, &r).is_ok());
        // A scope added in the kept file, with or without a new id.
        let mut wider = r.clone();
        wider
            .scopes
            .push("write:path:services/auth/**".parse().unwrap());
        assert!(check_intact(&r.id, &wider).is_err());
        wider.id = wider.content_id();
        let err = check_intact(&r.id, &wider).unwrap_err();
        assert!(err.to_string().contains("nothing is minted"), "{err}");

        let shown = |scopes: Value| json!({ "scopes": scopes });
        assert!(check_scopes(&r, &shown(json!(["write:path:services/billing/**"]))).is_ok());
        assert!(
            check_scopes(
                &r,
                &json!({ "input": { "scopes": ["write:path:services/billing/**"] } })
            )
            .is_ok()
        );
        let err = check_scopes(&r, &shown(json!(["write:path:services/**"]))).unwrap_err();
        assert!(err.to_string().contains("nothing is minted"), "{err}");
        assert!(check_scopes(&r, &json!({})).is_err());
    }

    #[test]
    fn the_policy_in_onus_cloud_is_logged_as_the_policy() {
        let by = |b: Value| decided_by(&json!({ "decision": { "by": b } }));
        assert_eq!(by(json!({ "id": "user:1", "name": "Ada" })), "Ada");
        assert_eq!(by(json!({ "id": "system:policy" })), "system:policy");
        assert_eq!(
            by(json!({ "id": "system:policy", "name": "policy" })),
            "system:policy"
        );
        assert_eq!(
            by(json!({ "id": "system:policy", "name": "" })),
            "system:policy"
        );
        assert_eq!(by(Value::Null), "a person");
    }

    #[test]
    fn blocked_reports_are_not_permission_requests() {
        assert_eq!(
            blocked_kind("broken-test").unwrap(),
            EscalationKind::BrokenTest
        );
        assert!(
            blocked_kind("permission")
                .unwrap_err()
                .to_string()
                .contains("onus_escalate")
        );
        assert!(blocked_kind("bored").is_err());
        assert_eq!(cloud_label("resolved"), "answered");
        assert!(is_final("answered") && !is_final("waiting for a person in Onus Cloud"));
    }
}
