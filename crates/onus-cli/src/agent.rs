//! What `onus mcp` lets an agent do besides asking the map, over the same
//! API as `onus ui`: hand in a change, run environments, read evidence and
//! outcomes, ask for more access. Nothing here lets an agent approve
//! itself: it cannot record outcomes, grant escalations or mint tokens,
//! and its task token (from `--token`) is attached by the server, not
//! chosen by the agent. Every call has already been checked at the door
//! ([`onus_cli::door`]) when it gets here.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use onus_cli::door::Door;
use serde_json::{Value, json};

use crate::ui::{State, api};

pub struct AgentActions {
    pub state: State,
    /// The door the server checks calls at; it holds the task token (from
    /// `onus mcp --token`, or a granted escalation's since).
    pub door: Arc<Door>,
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

    fn escalate(&self, mut input: Value) -> Result<Value> {
        let task = match (
            self.task(),
            input["task"].as_str().filter(|t| !t.is_empty()),
        ) {
            // With a token, the request is the token's task, whatever the agent says.
            (Some(t), _) => t,
            (None, Some(t)) => t.to_string(),
            (None, None) => bail!("name the task (no task token was given to onus mcp)"),
        };
        input["task"] = json!(task);
        let out = call(&self.state, "escalations.create", input)?;
        let r = &out["request"];
        Ok(json!({
            "id": r["id"],
            "task": r["task"],
            "blastRadius": r["blastRadius"],
            "sensitive": r["sensitive"],
            "evidence": r["evidence"],
            "state": "open",
            "next": "A person, or the policy for low-risk requests with a reproduced failing test, decides. Check with onus_escalation.",
        }))
    }

    fn escalation(&self, input: Value) -> Result<Value> {
        let id = input["id"].as_str().unwrap_or_default();
        let path = api::request_path(&self.state, id)?;
        let req = crate::doors::load_request(&path).map_err(|_| anyhow!("no escalation {id}"))?;
        if let Some(task) = self.task()
            && task != req.task
        {
            bail!("escalation {id} belongs to another task");
        }
        let decision = api::decisions(&self.state).remove(id);
        // The kept token proves a grant on its own: it is root-signed and
        // names this request, wherever the grant was logged.
        let kept = api::kept_token(&self.state, id);
        let proven = match (&kept, self.door.public()) {
            (Some(t), Some(key)) => onus_doors::escalation::verify_granted(&req, t, &key).is_ok(),
            _ => false,
        };
        let state = match decision.as_ref().and_then(|d| d["decision"].as_str()) {
            _ if proven => "granted",
            Some("granted") => "granted",
            Some("denied") => "denied",
            Some("needs-person") => "waiting for a person",
            _ => "open",
        };
        let mut out = json!({ "id": id, "state": state, "request": req, "decision": decision });
        if state == "granted"
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
            "cloud.envs" => self.listed(
                "onus_cloud_envs",
                onus_cli::cloud::act(
                    &onus_cli::cloud::Client::require()?,
                    &self.state.root,
                    name,
                    input,
                )?,
            ),
            "agent.evidence" => self.evidence(),
            "agent.escalate" => self.escalate(input),
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
                    let Some(r) = onus_cli::cloud::repository_of(&self.state.root) else {
                        bail!(
                            "your token is for this worktree's repository, and this worktree has no GitHub remote: pass repository (owner/name)"
                        );
                    };
                    input["repository"] = json!(r);
                }
                onus_cli::cloud::act(
                    &onus_cli::cloud::Client::require()?,
                    &self.state.root,
                    name,
                    input,
                )
            }
            n if n.starts_with("cloud.") => onus_cli::cloud::act(
                &onus_cli::cloud::Client::require()?,
                &self.state.root,
                n,
                input,
            ),
            n if PASSTHROUGH.contains(&n) => call(&self.state, n, input),
            _ => bail!("agents cannot call `{name}`"),
        }
    }
}
