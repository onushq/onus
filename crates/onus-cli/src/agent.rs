//! What `onus mcp` lets an agent do besides asking the map, over the same
//! API as `onus ui`: hand in a change, run environments, read evidence and
//! outcomes, ask for more access or report that it is blocked. Escalations go
//! through [`crate::cloud_doors`]. Nothing here lets an agent approve
//! itself: it cannot record outcomes, grant escalations or mint tokens,
//! and its task token (from `--token`) is attached by the server, not
//! chosen by the agent. Every call has already been checked at the door
//! ([`onus_cli::door`]) when it gets here.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use onus_cli::cloud::{self, CloudDoor};
use onus_cli::door::Door;
use serde_json::{Value, json};

use crate::cloud_doors::{self, EscalationDoor};
use crate::ui::{State, api};

pub struct AgentActions {
    pub state: State,
    /// The door the server checks calls at; it holds the task token (from
    /// `onus mcp --token`, or a granted escalation's since).
    pub door: Arc<Door>,
    /// The root private key the door holds (`onus mcp --key`), never shown to the
    /// agent: grants decided by policy or by a person in Onus Cloud are minted with it.
    pub key: Option<PathBuf>,
    /// A checkout whose files the agent cannot edit (`onus mcp --trusted-repo`): with the key, the
    /// local policy decides from its map and onus.yaml.
    pub trusted: Option<PathBuf>,
    /// Every Onus Cloud call goes through it: a scoped token minted from a login, or
    /// the token or API key the process was given.
    pub cloud: Option<Arc<CloudDoor>>,
}

/// What an agent may call on the API as it is (the door has decided).
const PASSTHROUGH: &[&str] = &[
    "lanes.policy",
    "envs.run",
    "envs.list",
    "envs.destroy",
    "evidence.show",
    "evidence.artifact",
    "runTest",
];

fn call(state: &State, name: &str, input: Value) -> Result<Value> {
    api::call(state, name, input).map_err(|e| match e {
        api::Error::NotFound(what) | api::Error::Conflict(what) => anyhow!(what),
        api::Error::Failed(e) => e,
    })
}

impl AgentActions {
    fn task(&self) -> Option<String> {
        self.door.task()
    }

    fn submit(&self, mut input: Value) -> Result<Value> {
        if let Some(t) = self.door.token() {
            input["token"] = json!(t);
        }
        // The token's agent, when it names one, whatever the agent declares.
        let mut note = None;
        if let Some(a) = self.door.agent() {
            let declared = &input["agent"];
            let differs = [
                ("tool", &a.tool),
                ("model", &a.model),
                ("config", &a.config),
            ]
            .iter()
            .any(|(k, v)| {
                declared[*k]
                    .as_str()
                    .is_some_and(|d| !d.is_empty() && d != v.as_str())
            });
            if differs {
                note = Some(format!(
                    "your agent identity comes from your token: {}",
                    [a.tool.as_str(), a.model.as_str(), a.config.as_str()]
                        .iter()
                        .filter(|p| !p.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join("/")
                ));
            }
            input["agent"] =
                json!({ "tool": a.tool, "model": a.model, "config": a.config, "team": a.team });
        }
        if input["agent"]["tool"].as_str().is_none_or(str::is_empty) {
            input["agent"]["tool"] = json!("mcp-agent");
        }
        let sub = call(&self.state, "lanes.submit", input)?;
        let saved = call(
            &self.state,
            "submissions.save",
            json!({ "submission": sub }),
        )?;
        let id = saved["id"].as_str().unwrap_or_default().to_string();
        let c = call(&self.state, "lanes.classify", json!({ "submission": sub }))?;
        let rows: Vec<Value> = sub["report"]["changes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| {
                        json!({
                            "title": r["title"],
                            "kind": r["kindLabel"],
                            "needsPerson": r["hints"]["needsPerson"],
                            "outsideIntent": r["hints"]["intentMismatch"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut out = json!({
            "id": id,
            "lane": c["lane"],
            "why": c["applied"],
            "summary": sub["report"]["summary"],
            "rows": rows,
            "evidence": sub["evidence"].as_array().map_or(0, Vec::len),
            "escalations": sub["escalations"],
            "agent": sub["agent"],
            "scope": sub["scope"],
            "next": format!("onus_judge with id {id} verifies it; a person sees it in onus ui under Lanes & judge"),
        });
        if let Some(n) = note {
            out["agentNote"] = json!(n);
        }
        Ok(out)
    }

    fn escalation_door(&self) -> EscalationDoor<'_> {
        EscalationDoor {
            state: &self.state,
            token: self.door.token(),
            public: self.door.public(),
            key: self.key.as_deref(),
            trusted: self.trusted.as_deref(),
            cloud: self.cloud.as_deref(),
            repository: self.cloud.as_ref().and_then(|c| c.repository()),
            pull: None,
        }
    }

    fn escalate(&self, input: Value) -> Result<Value> {
        let task = match (
            self.task(),
            input["task"].as_str().filter(|t| !t.is_empty()),
        ) {
            // With a token, the request is the token's task, whatever the agent says.
            (Some(t), _) => t,
            (None, Some(t)) => t.to_string(),
            (None, None) => bail!("name the task (no task token was given to onus mcp)"),
        };
        let kind = input["kind"].as_str().unwrap_or("permission");
        let kind: onus_doors::escalation::EscalationKind = serde_json::from_value(json!(kind))
            .map_err(|_| {
                anyhow!(
                    "`{kind}` is not a kind: permission, broken-test, contradictory-spec or impossible-task"
                )
            })?;
        let list = |k: &str| -> Vec<String> {
            input[k]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let req = crate::doors::new_request(
            &self.state.root,
            &task,
            kind,
            &list("scopes"),
            &list("evidence"),
            input["reason"].as_str().unwrap_or_default(),
        )?;
        cloud_doors::route(&self.escalation_door(), req)
    }

    /// The safe exit: a structured report that the task is blocked, with no rights
    /// asked for, routed as any escalation.
    fn blocked(&self, mut input: Value) -> Result<Value> {
        let kind = cloud_doors::blocked_kind(input["kind"].as_str().unwrap_or_default())?;
        input["kind"] = json!(kind);
        input["scopes"] = json!([]);
        let mut out = self.escalate(input)?;
        out["next"] = json!(cloud_doors::BLOCKED_NEXT);
        Ok(out)
    }

    fn escalation(&self, input: Value) -> Result<Value> {
        let id = input["id"].as_str().unwrap_or_default();
        let wait = input["wait"].as_u64().unwrap_or(0).min(50);
        let mut out = cloud_doors::follow(&self.escalation_door(), id, wait)?;
        let kept = out
            .as_object_mut()
            .and_then(|o| o.remove("token"))
            .and_then(|t| t.as_str().map(str::to_string));
        if out["state"] == "granted"
            && let Some(t) = kept
        {
            if self.door.task().is_some() {
                // The server uses it from now on: reads, environments, submissions.
                match self.door.adopt(&t) {
                    Ok(()) => {
                        // Also for the git gateway, which takes it as the password.
                        out["token"] = json!(t);
                        out["adopted"] = json!(true);
                        out["next"] = json!(
                            "This server now uses the granted token: your old rights plus exactly what you asked for."
                        );
                    }
                    Err(e) => {
                        // Not handed out either: it is not an extension of yours.
                        out["adopted"] = json!(false);
                        out["next"] = json!(format!("This server keeps your old token: {e:#}."));
                    }
                }
            } else {
                out["token"] = json!(t);
                out["next"] = json!(
                    "Use this token from now on: your old rights plus exactly what you asked for."
                );
            }
        }
        Ok(out)
    }

    /// The Onus Cloud client for the next call.
    fn cloud(&self) -> Result<cloud::Client> {
        self.cloud
            .as_ref()
            .context("not signed in to Onus Cloud: run onus login, or set ONUS_CLOUD_TOKEN")?
            .client()
    }

    /// A cloud action, with the repository this server is for when the agent names none.
    fn cloud_act(&self, name: &str, mut input: Value) -> Result<Value> {
        let repository = self.cloud.as_ref().and_then(|c| c.repository());
        let unnamed = |k: &str, input: &Value| match &input[k] {
            Value::String(s) => s.trim().is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => true,
        };
        match (name, repository) {
            ("cloud.map", Some(r)) if unnamed("repository", &input) => {
                input["repository"] = json!(r);
            }
            ("cloud.tokenMint", Some(r))
                if unnamed("repositories", &input)
                    && input["allRepositories"].as_bool() != Some(true) =>
            {
                input["repositories"] = json!([r]);
            }
            _ => {}
        }
        cloud::act(&self.cloud()?, &self.state.root, name, input)
    }

    fn outcomes(&self, input: Value) -> Result<Value> {
        let all = call(&self.state, "outcomes", json!({}))?;
        let agent = input["agent"].as_str().filter(|a| !a.is_empty());
        let records: Vec<Value> = all["records"]
            .as_array()
            .map(|r| {
                r.iter()
                    .filter(|o| agent.is_none_or(|a| o["agent"] == a))
                    .rev()
                    .take(50)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({
            "summary": match agent {
                Some(a) => all["summary"]["byAgent"][a].clone(),
                None => all["summary"].clone(),
            },
            "recent": records,
            "heldOutBacklog": all["backlog"],
        }))
    }

    fn evidence(&self) -> Result<Value> {
        let all = call(&self.state, "evidence.list", json!({}))?;
        let runs: Vec<Value> = all["runs"]
            .as_array()
            .map(|r| {
                r.iter()
                    .map(|run| {
                        let m = &run["manifest"];
                        json!({
                            "id": run["id"],
                            "commit": m["commit"],
                            "environment": m["environment"]["name"],
                            "task": m["environment"]["task"],
                            "command": m["command"],
                            "exitCode": m["exitCode"],
                            "tests": m["tests"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut out = json!({ "runs": runs });
        self.door
            .filter_list("onus_evidence", &mut out, &self.door.facts());
        if let Some(runs) = out["runs"].as_array_mut() {
            runs.truncate(20);
        }
        Ok(out)
    }

    /// A list, less what the task did not make.
    fn listed(&self, tool: &str, mut answer: Value) -> Result<Value> {
        self.door.filter_list(tool, &mut answer, &self.door.facts());
        Ok(answer)
    }
}

impl onus_cli::mcp::Actions for AgentActions {
    fn call(&self, name: &str, input: Value) -> Result<Value> {
        match name {
            "agent.submit" => self.submit(input),
            "agent.judge" => call(&self.state, "submissions.judge", input),
            "agent.envCreate" => {
                let mut input = input;
                input["token"] = json!(self.door.token());
                call(&self.state, "envs.create", input)
            }
            "envs.list" => self.listed("onus_envs", call(&self.state, name, input)?),
            "cloud.envs" => self.listed("onus_cloud_envs", self.cloud_act(name, input)?),
            "agent.evidence" => self.evidence(),
            "agent.escalate" => self.escalate(input),
            "agent.blocked" => self.blocked(input),
            "agent.escalation" => self.escalation(input),
            "agent.outcomes" => self.outcomes(input),
            "cloud.pulls" if self.task().is_some() => {
                // A task token is for this worktree's repository, and so is
                // the list: not every repository of the organisation.
                let mut input = input;
                if input["repository"]
                    .as_str()
                    .is_none_or(|r| r.trim().is_empty())
                {
                    // The worktree's remote, else the repository the operator named
                    // (--cloud-repo).
                    let Some(r) = onus_cli::cloud::repository_of(&self.state.root)
                        .or_else(|| self.cloud.as_ref().and_then(|c| c.repository()))
                    else {
                        bail!(
                            "your token is for this worktree's repository, and this worktree has no GitHub remote: pass repository (owner/name)"
                        );
                    };
                    input["repository"] = json!(r);
                }
                self.cloud_act(name, input)
            }
            n if n.starts_with("cloud.") => self.cloud_act(n, input),
            n if PASSTHROUGH.contains(&n) => call(&self.state, n, input),
            _ => bail!("agents cannot call `{name}`"),
        }
    }
}
