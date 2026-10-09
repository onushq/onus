//! `onus mcp`: the codebase map as MCP tools, over stdio.
//!
//! Every tool is read-only and answers from the map of the agent's own
//! worktree, as it is on disk now. Answers are JSON, sorted, bounded and
//! carry their evidence (file and line). The work happens in the
//! repository's map server (see [`crate::daemon`]), shared with every other
//! agent; this process only relays.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use onus_index::Query;
use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::daemon::{Op, Request, Response};

/// What agents can do besides asking the map: submit a change, run
/// environments, read evidence, ask for more access. The binary implements
/// it over the same functions as the commands and `onus ui`; agents get
/// only these, never what would let them approve themselves (recording
/// outcomes, granting escalations, minting tokens).
pub trait Actions: Send + Sync {
    fn call(&self, name: &str, input: serde_json::Value) -> Result<serde_json::Value>;
}

/// Where requests go: the repository's server, or this process.
pub trait Backend: Send + Sync {
    fn call(&self, req: &Request) -> Result<Response>;
}

impl Backend for crate::daemon::Server {
    fn call(&self, req: &Request) -> Result<Response> {
        Ok(self.handle(req))
    }
}

#[cfg(unix)]
impl Backend for crate::daemon::Client {
    fn call(&self, req: &Request) -> Result<Response> {
        self.request(req)
    }
}

#[derive(Clone)]
pub struct OnusMcp {
    root: PathBuf,
    backend: Arc<dyn Backend>,
    tool_router: ToolRouter<Self>,
    /// With a task token: answers leave out what the task may not read.
    read_scope: Option<Arc<ReadScope>>,
    actions: Option<Arc<dyn Actions>>,
}

/// A task token's read scope and the component folders that turn map ids
/// into repository paths.
pub struct ReadScope {
    pub token: onus_doors::token::Verified,
    dirs: std::sync::Mutex<Option<std::collections::BTreeMap<String, String>>>,
}

impl ReadScope {
    pub fn new(token: onus_doors::token::Verified) -> ReadScope {
        ReadScope {
            token,
            dirs: std::sync::Mutex::new(None),
        }
    }

    fn readable(&self, path: &str) -> bool {
        let now = onus_doors::gateway::now();
        self.token
            .authorize(onus_doors::scope::Kind::Read, path, now)
            .is_ok()
    }
}

impl std::fmt::Debug for ReadScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadScope")
            .field("task", &self.token.task)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindParams {
    /// Words from a symbol name, such as `format phone` or `UserPreferences`.
    pub text: String,
    /// At most this many symbols (default 50).
    #[serde(default)]
    pub limit: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdParams {
    /// A symbol id (`component:path#Name`), as returned by other tools.
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WalkParams {
    /// A symbol id, a bare symbol name, a file path, a module id or a
    /// component id.
    pub target: String,
    /// How many steps to follow (default 1, at most 8).
    #[serde(default = "one")]
    pub depth: u32,
    /// At most this many results (default 50).
    #[serde(default)]
    pub limit: usize,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TargetParams {
    /// A symbol id, a bare symbol name, a file path, a module id or a
    /// component id.
    pub target: String,
    /// At most this many results (default 50).
    #[serde(default)]
    pub limit: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ImpactParams {
    /// A symbol id, a bare symbol name, a file path, a module id or a
    /// component id.
    pub target: String,
    /// The change to reason about: `remove`, `rename`, `change-signature`,
    /// `add-required-member` or `change-behavior`.
    pub change: onus_index::Change,
    /// At most this many sites (default 50).
    #[serde(default)]
    pub limit: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InvariantsParams {
    /// A symbol, file or component; empty for every declared invariant.
    #[serde(default)]
    pub target: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ComponentParams {
    /// A component id, as listed by `onus_status` or returned by other tools.
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileParams {
    /// A path relative to the repository root.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CheckParams {
    /// The commit to compare the worktree with (default `HEAD`).
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubmitParams {
    /// The commit the change starts from, such as `main`.
    pub base: String,
    /// The commit with the change (default `HEAD`). Commit your work first.
    #[serde(default)]
    pub head: Option<String>,
    /// What the change is meant to do, as an `onus-intent` YAML block or
    /// Markdown containing one: `summary`, `touches` (components),
    /// `contracts`, `externals`. Rows outside it go to a person.
    #[serde(default)]
    pub intent: Option<String>,
    /// Runs from `onus_env_run` (their ids) that prove the change works.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Your harness, such as `claude-code` or `codex`.
    #[serde(default)]
    pub agent_tool: Option<String>,
    /// Your model.
    #[serde(default)]
    pub agent_model: Option<String>,
    /// A name for your configuration (prompts, tools).
    #[serde(default)]
    pub agent_config: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SubmissionParams {
    /// A submission id from `onus_submit`.
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EnvCreateParams {
    /// The commit to build the environment from (default `HEAD`).
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    /// A name for it (default: your task, else the short commit).
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EnvRunParams {
    /// The environment, from `onus_env_create` or `onus_envs`.
    pub name: String,
    /// A shell command run at the repository root, such as `npm test`.
    pub command: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EnvNameParams {
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EvidenceParams {
    /// A run id (or its first 8 characters); empty to list the runs.
    #[serde(default)]
    pub id: Option<String>,
    /// An artifact of the run to read: its path, or `-` for the output log.
    #[serde(default)]
    pub artifact: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunTestParams {
    /// The commit to test (default `HEAD`).
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    /// The container image, such as `node:22`.
    pub image: String,
    /// A setup command run first, with network, such as `npm ci`.
    #[serde(default)]
    pub setup: Option<String>,
    /// The test command, run without network.
    pub command: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EscalateParams {
    /// `permission` (you need a right your token lacks), `broken-test`,
    /// `contradictory-spec` or `impossible-task`.
    #[serde(default = "permission")]
    pub kind: String,
    /// The rights you ask for, such as `write:path:services/orders/src/**`.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Evidence, strongest first: `failing-test:<file>`, `trace:<file>`,
    /// `map-path:<path>`, `draft-diff:<file>`, `rationale:<text>`. A
    /// failing test that reproduces can be granted without a person.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Why the task needs it.
    pub reason: String,
    /// The task (default: your token's task).
    #[serde(default)]
    pub task: Option<String>,
}

fn permission() -> String {
    "permission".into()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EscalationParams {
    /// An escalation id from `onus_escalate`.
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OutcomesParams {
    /// An agent setup (`tool/model/config`) to show the record of.
    #[serde(default)]
    pub agent: Option<String>,
}

impl std::fmt::Debug for OnusMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnusMcp")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

#[tool_router]
impl OnusMcp {
    pub fn new(root: PathBuf, backend: Arc<dyn Backend>) -> OnusMcp {
        OnusMcp {
            root,
            backend,
            tool_router: Self::tool_router(),
            read_scope: None,
            actions: None,
        }
    }

    /// Adds the tools that act: submissions, environments, escalations.
    pub fn with_actions(mut self, actions: Option<Arc<dyn Actions>>) -> OnusMcp {
        self.actions = actions;
        self
    }

    async fn act(
        &self,
        name: &'static str,
        input: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(actions) = self.actions.clone() else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "this Onus server only answers questions (started with --no-actions)",
            )]));
        };
        let answer = tokio::task::spawn_blocking(move || actions.call(name, input))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(match answer {
            Ok(v) => CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&v).unwrap_or_default(),
            )]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))]),
        })
    }

    /// Limits every answer to what `scope`'s token may read.
    pub fn with_read_scope(mut self, scope: Option<Arc<ReadScope>>) -> OnusMcp {
        self.read_scope = scope;
        self
    }

    /// Component folders, asked once from the map.
    async fn component_dirs(
        &self,
        scope: &ReadScope,
    ) -> std::collections::BTreeMap<String, String> {
        if let Ok(guard) = scope.dirs.lock()
            && let Some(d) = guard.as_ref()
        {
            return d.clone();
        }
        let req = Request {
            root: self.root.clone(),
            op: Op::Query {
                query: Query::Status,
            },
        };
        let backend = self.backend.clone();
        let dirs: std::collections::BTreeMap<String, String> =
            tokio::task::spawn_blocking(move || backend.call(&req))
                .await
                .ok()
                .and_then(|r| r.ok())
                .and_then(|r| r.ok)
                .and_then(|v| serde_json::from_value(v["componentDirs"].clone()).ok())
                .unwrap_or_default();
        if let Ok(mut guard) = scope.dirs.lock() {
            *guard = Some(dirs.clone());
        }
        dirs
    }

    /// Removes what the token may not read; `Err` when the whole answer is
    /// about an unreadable file.
    async fn limit(&self, mut ok: serde_json::Value) -> Result<(serde_json::Value, usize), String> {
        let Some(scope) = self.read_scope.clone() else {
            return Ok((ok, 0));
        };
        let dirs = self.component_dirs(&scope).await;
        let readable = |p: &str| scope.readable(p);
        if onus_doors::filter::about_unreadable(&ok, &readable) {
            return Err(format!(
                "outside the read scope of task `{}`",
                scope.token.task
            ));
        }
        let id_path = |s: &str| -> Option<String> {
            let (comp, rest) = s.split_once(':')?;
            let path = rest.split('#').next()?;
            let dir = dirs.get(comp)?;
            Some(if dir.is_empty() {
                path.to_string()
            } else {
                format!("{dir}/{path}")
            })
        };
        let removed = onus_doors::filter::filter_by_read(&mut ok, &readable, &id_path);
        Ok((ok, removed))
    }

    async fn run(&self, op: Op) -> Result<CallToolResult, ErrorData> {
        let req = Request {
            root: self.root.clone(),
            op,
        };
        let backend = self.backend.clone();
        let response = tokio::task::spawn_blocking(move || backend.call(&req))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(match response {
            Ok(Response {
                ok: Some(ok), map, ..
            }) => match self.limit(ok).await {
                Ok((ok, hidden)) => {
                    let mut body = serde_json::json!({ "result": ok, "map": map });
                    if hidden > 0 {
                        body["hidden"] = serde_json::json!(format!(
                            "{hidden} items outside your read scope were left out"
                        ));
                    }
                    CallToolResult::success(vec![ContentBlock::text(
                        serde_json::to_string_pretty(&body).unwrap_or_default(),
                    )])
                }
                Err(why) => CallToolResult::error(vec![ContentBlock::text(why)]),
            },
            Ok(Response { error, .. }) => CallToolResult::error(vec![ContentBlock::text(
                error.unwrap_or_else(|| "no answer".into()),
            )]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))]),
        })
    }

    async fn query(&self, query: Query) -> Result<CallToolResult, ErrorData> {
        self.run(Op::Query { query }).await
    }

    #[tool(
        name = "onus_status",
        description = "What the codebase map of this worktree holds: components, files, symbols, edges and tests. Start here."
    )]
    async fn status(&self) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Status).await
    }

    #[tool(
        name = "onus_find",
        description = "Find symbols (functions, classes, types, constants) by name words, best match first. Use before writing a helper to see whether one exists."
    )]
    async fn find(
        &self,
        Parameters(p): Parameters<FindParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Find {
            text: p.text,
            limit: p.limit,
        })
        .await
    }

    #[tool(
        name = "onus_symbol",
        description = "One symbol: kind, location, contract shape (parameters, return type, members), declared invariants, and how many places use it."
    )]
    async fn symbol(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Symbol { id: p.id }).await
    }

    #[tool(
        name = "onus_dependents",
        description = "What depends on a symbol, file or component (imports, calls, type references), with file and line for each. Use before changing something to see what could break."
    )]
    async fn dependents(
        &self,
        Parameters(p): Parameters<WalkParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Dependents {
            target: p.target,
            depth: p.depth,
            limit: p.limit,
        })
        .await
    }

    #[tool(
        name = "onus_dependencies",
        description = "What a symbol, file or component depends on: code, external services, events, database tables and config keys, with file and line for each."
    )]
    async fn dependencies(
        &self,
        Parameters(p): Parameters<WalkParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Dependencies {
            target: p.target,
            depth: p.depth,
            limit: p.limit,
        })
        .await
    }

    #[tool(
        name = "onus_tests_for",
        description = "The test files that exercise a symbol, file or component, with their number of cases and skipped cases."
    )]
    async fn tests_for(
        &self,
        Parameters(p): Parameters<TargetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::TestsFor {
            target: p.target,
            limit: p.limit,
        })
        .await
    }

    #[tool(
        name = "onus_owners",
        description = "Who owns a symbol, file or component (onus.yaml and CODEOWNERS), and its sensitivity labels such as payments or pii."
    )]
    async fn owners(
        &self,
        Parameters(p): Parameters<TargetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Owners { target: p.target }).await
    }

    #[tool(
        name = "onus_impact",
        description = "Before changing a symbol, file or component: what breaks if you remove it, rename it, change its signature, add a required member to it, or change only its behavior. Returns one site per file (file, line, how it is used), test files first among equals, and the tests that exercise it. Use the sites as the list of places to update and the tests as what to run."
    )]
    async fn impact(
        &self,
        Parameters(p): Parameters<ImpactParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Impact {
            target: p.target,
            change: p.change,
            limit: p.limit,
        })
        .await
    }

    #[tool(
        name = "onus_invariants",
        description = "The invariants onus.yaml declares for a symbol, file or component (all of them when the target is empty): rules a change must keep, such as \"amounts are in cents\"."
    )]
    async fn invariants(
        &self,
        Parameters(p): Parameters<InvariantsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Invariants { target: p.target }).await
    }

    #[tool(
        name = "onus_component",
        description = "A component: kind, owners, labels, public symbols, the components it uses and that use it, external services and events."
    )]
    async fn component(
        &self,
        Parameters(p): Parameters<ComponentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::Component { id: p.id }).await
    }

    #[tool(
        name = "onus_file",
        description = "A file: its component, the symbols it declares, what it imports and which files import it."
    )]
    async fn file(
        &self,
        Parameters(p): Parameters<FileParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.query(Query::File { path: p.path }).await
    }

    #[tool(
        name = "onus_check",
        description = "Check your work before finishing: the changes in meaning between a commit (default HEAD) and this worktree as it is now. Returns a checklist of things to verify (third-party APIs this repository has never used, with the pinned version; users of a changed interface you have not updated; secrets; broken boundary rules) and the full report (contract changes, new external services, new dependencies between components, weakened tests)."
    )]
    async fn check(
        &self,
        Parameters(p): Parameters<CheckParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(Op::Check { base: p.base }).await
    }

    #[tool(
        name = "onus_lanes",
        description = "The lane policy of this repository: which changes merge on their own, which go to the judge, which need a person, and the floors no policy can lower. Read it to know what your change will need."
    )]
    async fn lanes(&self) -> Result<CallToolResult, ErrorData> {
        self.act("lanes.policy", serde_json::json!({})).await
    }

    #[tool(
        name = "onus_submit",
        description = "Submit a committed change for review: Onus reports it, attaches your evidence (runs from onus_env_run) and your task's scope, and classifies it into a lane (auto-merge, judge, human or blocked) with every reason. Returns a submission id for onus_judge. Submit when you are done, with an intent and evidence; a change without them is rejected by the judge."
    )]
    async fn submit(
        &self,
        Parameters(p): Parameters<SubmitParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "agent.submit",
            serde_json::json!({
                "base": p.base,
                "head": p.head.unwrap_or_else(|| "HEAD".into()),
                "intent": p.intent,
                "evidence": p.evidence,
                "agent": { "tool": p.agent_tool.unwrap_or_default(), "model": p.agent_model.unwrap_or_default(), "config": p.agent_config.unwrap_or_default(), "team": "" },
            }),
        )
        .await
    }

    #[tool(
        name = "onus_judge",
        description = "Run the verifying judge on a submission: it re-runs your evidence in fresh containers, checks intent, contracts and tests, and runs held-out checks you cannot see. Returns approve, reject (with reasons to act on) or escalate. Takes minutes; needs Docker or Podman."
    )]
    async fn judge(
        &self,
        Parameters(p): Parameters<SubmissionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act("agent.judge", serde_json::json!({ "id": p.id }))
            .await
    }

    #[tool(
        name = "onus_env_create",
        description = "Create an environment: a container built from a commit with the repository's declared image, setup and seed data. It has no network unless your task's token names hosts, and only the secrets your token names. Run commands in it with onus_env_run."
    )]
    async fn env_create(
        &self,
        Parameters(p): Parameters<EnvCreateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "agent.envCreate",
            serde_json::json!({ "ref": p.reference.unwrap_or_else(|| "HEAD".into()), "name": p.name }),
        )
        .await
    }

    #[tool(
        name = "onus_env_run",
        description = "Run a command (such as the tests) in an environment. The run is recorded as evidence: its output, JUnit results and traces, under a run id you can pass to onus_submit. Returns the exit code, test counts and the end of the output."
    )]
    async fn env_run(
        &self,
        Parameters(p): Parameters<EnvRunParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "envs.run",
            serde_json::json!({ "name": p.name, "command": p.command }),
        )
        .await
    }

    #[tool(
        name = "onus_envs",
        description = "The environments that exist: name, commit, state, network and secrets."
    )]
    async fn envs(&self) -> Result<CallToolResult, ErrorData> {
        self.act("envs.list", serde_json::json!({})).await
    }

    #[tool(
        name = "onus_env_destroy",
        description = "Remove an environment when you are done with it."
    )]
    async fn env_destroy(
        &self,
        Parameters(p): Parameters<EnvNameParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act("envs.destroy", serde_json::json!({ "name": p.name }))
            .await
    }

    #[tool(
        name = "onus_evidence",
        description = "Recorded runs: without an id, the latest runs; with an id, the run's manifest (commit, command, exit code, tests, artifacts); with an artifact, its contents (`-` for the output log)."
    )]
    async fn evidence(
        &self,
        Parameters(p): Parameters<EvidenceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match (p.id.filter(|i| !i.is_empty()), p.artifact) {
            (None, _) => self.act("agent.evidence", serde_json::json!({})).await,
            (Some(id), None) => {
                self.act("evidence.show", serde_json::json!({ "id": id }))
                    .await
            }
            (Some(id), Some(path)) => {
                self.act(
                    "evidence.artifact",
                    serde_json::json!({ "id": id, "path": path }),
                )
                .await
            }
        }
    }

    #[tool(
        name = "onus_run_test",
        description = "Run a test command once at a commit in a throwaway container with no network, to reproduce a failure. Nothing is recorded; use onus_env_run for evidence."
    )]
    async fn run_test(
        &self,
        Parameters(p): Parameters<RunTestParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "runTest",
            serde_json::json!({ "ref": p.reference.unwrap_or_else(|| "HEAD".into()), "image": p.image, "setup": p.setup, "command": p.command }),
        )
        .await
    }

    #[tool(
        name = "onus_escalate",
        description = "Ask for more than your task's token allows, with evidence. Onus files the request with its blast radius and sensitive labels; low-risk requests with a reproduced failing test are granted by policy, others by a person. Check the answer with onus_escalation."
    )]
    async fn escalate(
        &self,
        Parameters(p): Parameters<EscalateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "agent.escalate",
            serde_json::json!({ "kind": p.kind, "scopes": p.scopes, "evidence": p.evidence, "reason": p.reason, "task": p.task }),
        )
        .await
    }

    #[tool(
        name = "onus_escalation",
        description = "The state of an escalation request: open, granted (with the new token, your old rights plus exactly what you asked for) or denied (with the reason)."
    )]
    async fn escalation(
        &self,
        Parameters(p): Parameters<EscalationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act("agent.escalation", serde_json::json!({ "id": p.id }))
            .await
    }

    #[tool(
        name = "onus_outcomes",
        description = "Track records: what happened to changes after they merged (reverted, incidents), per agent setup, and where held-out tests are wanted. Read-only. An agent setup needs a record before it may auto-merge."
    )]
    async fn outcomes(
        &self,
        Parameters(p): Parameters<OutcomesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act("agent.outcomes", serde_json::json!({ "agent": p.agent }))
            .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OnusMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("onus", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Onus answers questions about this codebase from a map computed from the code \
                 (never generated): where things are, what depends on what, which tests cover \
                 what, who owns it. The map follows the files of this worktree as they change. \
                 Ids look like `component:path#Name`; tools also accept a bare name or a file \
                 path. Run onus_check before finishing a change and include its findings in your \
                 summary as evidence; it reports facts, it does not approve the change. To hand a \
                 change in: commit it, run its tests with onus_env_create and onus_env_run, then \
                 onus_submit with an intent and those runs as evidence, and onus_judge to see the \
                 verdict. If your token does not allow what the task needs, onus_escalate with \
                 evidence instead of working around it.",
            )
    }
}

/// Serves MCP over streamable HTTP on `addr` until the process stops.
/// Requests must name an allowed host (`Host` header): loopback names by
/// default, which keeps other machines and DNS rebinding out. `ready` is
/// called with the bound address (useful with port 0).
pub fn serve_http(
    root: PathBuf,
    backend: Arc<dyn Backend>,
    read_scope: Option<Arc<ReadScope>>,
    actions: Option<Arc<dyn Actions>>,
    addr: std::net::SocketAddr,
    allowed_hosts: Vec<String>,
    ready: impl FnOnce(std::net::SocketAddr),
) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let mut config = StreamableHttpServerConfig::default();
        if !allowed_hosts.is_empty() {
            let mut hosts = config.allowed_hosts.clone();
            hosts.extend(allowed_hosts);
            config = config.with_allowed_hosts(hosts);
        }
        let service = StreamableHttpService::new(
            move || {
                Ok(OnusMcp::new(root.clone(), backend.clone())
                    .with_read_scope(read_scope.clone())
                    .with_actions(actions.clone()))
            },
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let listener = tokio::net::TcpListener::bind(addr).await?;
        ready(listener.local_addr()?);
        loop {
            let (stream, _) = listener.accept().await?;
            let service = hyper_util::service::TowerToHyperService::new(service.clone());
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                // A client that drops its connection ends only that connection.
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
    })
}

/// Serves MCP on stdin and stdout until the client disconnects.
pub fn serve_stdio(
    root: PathBuf,
    backend: Arc<dyn Backend>,
    read_scope: Option<Arc<ReadScope>>,
    actions: Option<Arc<dyn Actions>>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let service = OnusMcp::new(root, backend)
            .with_read_scope(read_scope)
            .with_actions(actions)
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok(())
    })
}
