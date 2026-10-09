//! What `onus mcp` lets an agent do besides asking the map, over the same
//! API as `onus ui`: hand in a change, run environments, read evidence and
//! outcomes, ask for more access. Nothing here lets an agent approve
//! itself: it cannot record outcomes, grant escalations or mint tokens,
//! and its task token (from `--token`) is attached by the server, not
//! chosen by the agent.

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::ui::{State, api};

pub struct AgentActions {
    pub state: State,
    /// The agent's task token, as given to `onus mcp --token`.
    pub token: Option<String>,
}

/// What an agent may call on the API as it is.
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
        let token = self.token.as_deref()?;
        let public = std::fs::read_to_string(&self.state.public_key).ok()?;
        let key = onus_doors::token::public_key(&public).ok()?;
        onus_doors::token::verify(token, &key).ok().map(|v| v.task)
    }

    fn submit(&self, mut input: Value) -> Result<Value> {
        if let Some(t) = &self.token {
            input["token"] = json!(t);
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
        Ok(json!({
            "id": id,
            "lane": c["lane"],
            "why": c["applied"],
            "summary": sub["report"]["summary"],
            "rows": rows,
            "evidence": sub["evidence"].as_array().map_or(0, Vec::len),
            "scope": sub["scope"],
            "next": format!("onus_judge with id {id} verifies it; a person sees it in onus ui under Lanes & judge"),
        }))
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
        let state = match decision.as_ref().and_then(|d| d["decision"].as_str()) {
            Some("granted") => "granted",
            Some("denied") => "denied",
            Some("needs-person") => "waiting for a person",
            _ => "open",
        };
        let mut out = json!({ "id": id, "state": state, "request": req, "decision": decision });
        if state == "granted"
            && let Some(t) = api::kept_token(&self.state, id)
        {
            out["token"] = json!(t);
            out["next"] = json!(
                "Use this token from now on: your old rights plus exactly what you asked for."
            );
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
                    .take(20)
                    .map(|run| {
                        let m = &run["manifest"];
                        json!({
                            "id": run["id"],
                            "commit": m["commit"],
                            "environment": m["environment"]["name"],
                            "command": m["command"],
                            "exitCode": m["exitCode"],
                            "tests": m["tests"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({ "runs": runs }))
    }
}

impl onus_cli::mcp::Actions for AgentActions {
    fn call(&self, name: &str, input: Value) -> Result<Value> {
        match name {
            "agent.submit" => self.submit(input),
            "agent.judge" => call(&self.state, "submissions.judge", input),
            "agent.envCreate" => {
                let mut input = input;
                input["token"] = json!(self.token);
                call(&self.state, "envs.create", input)
            }
            "agent.evidence" => self.evidence(),
            "agent.escalate" => self.escalate(input),
            "agent.escalation" => self.escalation(input),
            "agent.outcomes" => self.outcomes(input),
            n if PASSTHROUGH.contains(&n) => call(&self.state, n, input),
            _ => bail!("agents cannot call `{name}`"),
        }
    }
}
