//! `onus mcp`: the codebase map as MCP tools, over stdio.
//!
//! Every tool is read-only and answers from the map of the agent's own
//! worktree, as it is on disk now. Answers are JSON, sorted, bounded and
//! carry their evidence (file and line). The work happens in the
//! repository's map server (see [`crate::daemon`]), shared with every other
//! agent; this process only relays.
//!
//! Every call goes through the door ([`crate::door`]): checked against the
//! task token the server holds, then logged.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use onus_index::Query;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ServerCapabilities, ServerConfig,
    },
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::daemon::{Op, Request, Response};
use crate::door::{self, Denial, Door};

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
    /// Checks every call against the task token and logs it. With a token,
    /// answers leave out what the task may not read.
    door: Option<Arc<Door>>,
    /// Component folders, which turn map ids into repository paths.
    dirs: Arc<Mutex<Option<BTreeMap<String, String>>>>,
    actions: Option<Arc<dyn Actions>>,
}

/// What a tool call learned that its log entry needs: what the read filter
/// left out or refused, and an action's answer.
#[derive(Debug, Default)]
struct Note {
    hidden: usize,
    denial: Option<Denial>,
    answer: Option<Value>,
}

tokio::task_local! {
    static NOTE: Arc<Mutex<Note>>;
}

/// Adds to the note of the call being served, if any.
fn note(f: impl FnOnce(&mut Note)) {
    let _ = NOTE.try_with(|n| {
        if let Ok(mut n) = n.lock() {
            f(&mut n);
        }
    });
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
    /// Ids of escalations granted during the task (from onus_escalate). Each
    /// is checked: it must be granted, for your task.
    #[serde(default)]
    pub escalations: Vec<String>,
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
    /// An escalation id from `onus_escalate` or `onus_blocked`.
    pub id: String,
    /// Wait up to this many seconds (at most 50) while it is being checked or waiting
    /// for a person (default 0: answer now).
    #[serde(default)]
    pub wait: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BlockedParams {
    /// `broken-test` (a test you did not write fails and cannot be fixed in scope),
    /// `contradictory-spec` (the spec contradicts itself or the code) or
    /// `impossible-task` (it cannot be done as asked).
    pub kind: String,
    /// Evidence, strongest first: `failing-test:<file>`, `trace:<file>`,
    /// `map-path:<path>`, `draft-diff:<file>`, `rationale:<text>`.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// What blocks you, in a sentence or two.
    pub reason: String,
    /// The task (default: your token's task).
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudMapParams {
    /// The GitHub repository, `owner/name` (default: this worktree's).
    #[serde(default)]
    pub repository: Option<String>,
    /// A commit Onus Cloud mapped (default: the default branch's newest map).
    #[serde(default)]
    pub commit: Option<String>,
    /// A pull request's number: its head commit, as Onus Cloud analysed it.
    #[serde(default)]
    pub pull: Option<u64>,
    /// The question, as the map tools take it: `{"query": "dependents", "target":
    /// "UserPreferences", "depth": 2}`, `{"query": "impact", "target": "formatPhone",
    /// "change": "change-signature"}`, `{"query": "status"}`, `{"query": "overview"}`.
    pub question: Query,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudTokenParams {
    /// The scopes it opens: `env:read`, `env:run`, `evidence:write`, `pulls:read`,
    /// `map:read`, `escalation:write`. Never more than yours.
    pub scopes: Vec<String>,
    /// The repositories it is for, `owner/name` (default: this worktree's).
    #[serde(default)]
    pub repositories: Vec<String>,
    /// Every repository yours is for, instead.
    #[serde(default, rename = "allRepositories")]
    pub all_repositories: bool,
    /// How long it lasts, in seconds (60 to 86400; default 3600). Never past yours.
    #[serde(default, rename = "ttlSeconds")]
    pub ttl_seconds: Option<u64>,
    /// A name for it, such as the sub-agent's step.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OutcomesParams {
    /// An agent setup (`tool/model/config`) to show the record of.
    #[serde(default)]
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudEnvCreateParams {
    /// The GitHub repository, `owner/name` (default: this worktree's).
    #[serde(default)]
    pub repository: Option<String>,
    /// A branch, tag, commit or `pull/<n>` (default: this worktree's HEAD, which must be pushed).
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    /// A name for it (default: the repository's and a few letters).
    #[serde(default)]
    pub name: Option<String>,
    /// Hosts it may reach (`api.example.com`, `*.npmjs.org`); none means no network.
    #[serde(default)]
    pub hosts: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudEnvRunParams {
    /// The environment, from `onus_cloud_env_create` or `onus_cloud_envs`.
    pub name: String,
    /// A shell command run at the repository root, such as `npm test`.
    pub command: String,
    /// Stop it after this many seconds (default 1800).
    #[serde(default, rename = "timeoutSeconds")]
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudEnvsParams {
    /// Also the recently ended ones.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudPullsParams {
    /// Only this repository, `owner/name`.
    #[serde(default)]
    pub repository: Option<String>,
    /// `open`, `closed` or `merged`.
    #[serde(default)]
    pub state: Option<String>,
    /// `auto-merge`, `judge`, `human` or `blocked`.
    #[serde(default)]
    pub lane: Option<String>,
    /// Only those waiting for a person.
    #[serde(default)]
    pub attention: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudPullParams {
    /// The pull request's number.
    pub number: u64,
    /// Its repository, `owner/name` (default: this worktree's).
    #[serde(default)]
    pub repository: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloudEvidenceParams {
    /// The pull request's number.
    pub number: u64,
    /// Its repository, `owner/name` (default: this worktree's).
    #[serde(default)]
    pub repository: Option<String>,
    /// The run's id, from `onus_cloud_env_run`.
    pub run: String,
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
            door: None,
            dirs: Arc::new(Mutex::new(None)),
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
            Ok(v) => {
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                note(|n| n.answer = Some(v));
                CallToolResult::success(vec![ContentBlock::text(text)])
            }
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))]),
        })
    }

    /// Keeps the Onus Cloud tools only when this process can sign in (`onus login`, or
    /// ONUS_API_KEY): an agent that cannot use them is not shown them.
    pub fn with_cloud(mut self, signed_in: bool) -> OnusMcp {
        if !signed_in {
            let names: Vec<String> = self
                .tool_router
                .list_all()
                .into_iter()
                .map(|t| t.name.to_string())
                .filter(|n| n.starts_with("onus_cloud_"))
                .collect();
            for n in names {
                self.tool_router.remove_route(&n);
            }
        }
        self
    }

    /// Checks and logs every call at `door`, and limits answers to what its
    /// token may read.
    pub fn with_door(mut self, door: Option<Arc<Door>>) -> OnusMcp {
        self.door = door;
        self
    }

    /// Component folders, asked once from the map.
    async fn component_dirs(&self) -> BTreeMap<String, String> {
        if let Ok(guard) = self.dirs.lock()
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
        let dirs: BTreeMap<String, String> =
            tokio::task::spawn_blocking(move || backend.call(&req))
                .await
                .ok()
                .and_then(|r| r.ok())
                .and_then(|r| r.ok)
                .and_then(|v| serde_json::from_value(v["componentDirs"].clone()).ok())
                .unwrap_or_default();
        if let Ok(mut guard) = self.dirs.lock() {
            *guard = Some(dirs.clone());
        }
        dirs
    }

    /// An action whose answer is a map answer (`{result, componentDirs, ...}`):
    /// its `result` is limited to what the token may read, as the map tools' are.
    async fn act_limited(
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
        let mut v = match answer {
            Ok(v) => v,
            Err(e) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "{e:#}"
                ))]));
            }
        };
        let dirs: BTreeMap<String, String> =
            serde_json::from_value(v["componentDirs"].clone()).unwrap_or_default();
        let result = v["result"].take();
        Ok(match self.limit(result, Some(dirs)).await {
            Ok((result, hidden)) => {
                v["result"] = result;
                if let Some(o) = v.as_object_mut() {
                    o.remove("componentDirs");
                }
                if hidden > 0 {
                    v["hidden"] = json!(format!(
                        "{hidden} items outside your read scope were left out"
                    ));
                }
                note(|n| n.hidden = hidden);
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                note(|n| n.answer = Some(v));
                CallToolResult::success(vec![ContentBlock::text(text)])
            }
            Err(denial) => {
                let text = denial.text();
                note(|n| n.denial = Some(denial));
                CallToolResult::error(vec![ContentBlock::text(text)])
            }
        })
    }

    /// Removes what the token may not read; a denial when the whole answer
    /// is about an unreadable file. `dirs` maps component ids to their folders
    /// (default: this worktree's map).
    async fn limit(
        &self,
        mut ok: Value,
        dirs: Option<BTreeMap<String, String>>,
    ) -> Result<(Value, usize), Denial> {
        let Some(door) = self.door.clone().filter(|d| d.task().is_some()) else {
            return Ok((ok, 0));
        };
        let dirs = match dirs {
            Some(d) => d,
            None => self.component_dirs().await,
        };
        let readable = |p: &str| door.readable(p);
        if let Some(path) = onus_doors::filter::unreadable_subject(&ok, &readable) {
            return Err(door.read_denial(&path));
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
            }) => match self.limit(ok, None).await {
                Ok((ok, hidden)) => {
                    note(|n| n.hidden = hidden);
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
                Err(denial) => {
                    let text = denial.text();
                    note(|n| n.denial = Some(denial));
                    CallToolResult::error(vec![ContentBlock::text(text)])
                }
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
        description = "The test files that exercise a symbol, file or component, with their number of cases and skipped cases, and how each link is known: `ran` (coverage shows this test running it), `static-ran` (the test refers to it and the measured run executed it), `static` (it refers to it; no coverage), `static-not-run` (it refers to the target but the measured run never executed it). Strongest first. `coverage` gives the target's executable and ran lines when the coverage measured its file; tests can be empty while lines ran, when the target ran through other code."
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
        description = "Submit a committed change for review: Onus reports it, attaches your evidence (runs from onus_env_run), your task's scope and the escalations granted to you (ids from onus_escalate, checked), and classifies it into a lane (auto-merge, judge, human or blocked) with every reason. Returns a submission id for onus_judge. Submit when you are done, with an intent and evidence; a change without them is rejected by the judge."
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
                "escalations": p.escalations,
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
        description = "Run a command (such as the tests) in an environment. The run is recorded as evidence: its output, JUnit results, traces and coverage, under a run id you can pass to onus_submit. Returns the exit code, test counts and the end of the output."
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
        description = "Ask for more than your task's token allows, with evidence. Onus files the request with its blast radius and sensitive labels; low-risk requests with a reproduced failing test are granted by policy, others by a person. When the policy decides here, running the failing tests can take up to 15 minutes in all before this answers. Check the answer with onus_escalation."
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
        self.act(
            "agent.escalation",
            serde_json::json!({ "id": p.id, "wait": p.wait.min(50) }),
        )
        .await
    }

    #[tool(
        name = "onus_blocked",
        description = "You are blocked: a test you did not write fails and cannot be fixed in scope, the spec contradicts itself or the code, or the task cannot be done as asked. Report it with evidence and stop; do not work around it. A person answers through onus_escalation."
    )]
    async fn blocked(
        &self,
        Parameters(p): Parameters<BlockedParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "agent.blocked",
            serde_json::json!({ "kind": p.kind, "evidence": p.evidence, "reason": p.reason, "task": p.task }),
        )
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

    #[tool(
        name = "onus_cloud_status",
        description = "Onus Cloud: who this CLI is signed in as, the organisation it acts in, and what its plan allows of hosted environments (how many at once, minutes left)."
    )]
    async fn cloud_status(&self) -> Result<CallToolResult, ErrorData> {
        self.act("cloud.status", serde_json::json!({})).await
    }

    #[tool(
        name = "onus_cloud_env_create",
        description = "Create a hosted environment in Onus Cloud: a container built from a pushed commit (default: this worktree's HEAD) with the repository's declared image, setup and seed, on Onus Cloud's machines. Waits until it is ready (minutes, the first time). Run commands in it with onus_cloud_env_run. Use it to simulate a change in a clean environment when you cannot or should not run containers here."
    )]
    async fn cloud_env_create(
        &self,
        Parameters(p): Parameters<CloudEnvCreateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "cloud.envCreate",
            serde_json::json!({ "repository": p.repository, "ref": p.reference, "name": p.name, "hosts": p.hosts }),
        )
        .await
    }

    #[tool(
        name = "onus_cloud_env_run",
        description = "Run a command (such as the tests) in a hosted environment. Onus Cloud records the run: its output, JUnit results and traces, under a manifest the judge can check. Waits for it and returns the state (passed, failed, error), exit code, test counts and the end of the output, with the run id for onus_cloud_attach_evidence."
    )]
    async fn cloud_env_run(
        &self,
        Parameters(p): Parameters<CloudEnvRunParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "cloud.envRun",
            serde_json::json!({ "name": p.name, "command": p.command, "timeoutSeconds": p.timeout_seconds }),
        )
        .await
    }

    #[tool(
        name = "onus_cloud_envs",
        description = "Hosted environments of the organisation: name, repository, commit, status, and what the plan allows."
    )]
    async fn cloud_envs(
        &self,
        Parameters(p): Parameters<CloudEnvsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act("cloud.envs", serde_json::json!({ "all": p.all }))
            .await
    }

    #[tool(
        name = "onus_cloud_env_destroy",
        description = "Destroy a hosted environment when you are done with it (idle ones are destroyed after a while, but their minutes count until then)."
    )]
    async fn cloud_env_destroy(
        &self,
        Parameters(p): Parameters<EnvNameParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act("cloud.envDestroy", serde_json::json!({ "name": p.name }))
            .await
    }

    #[tool(
        name = "onus_cloud_pulls",
        description = "Pull requests Onus Cloud governs for the organisation, newest first: lane (auto-merge, judge, human, blocked), state, agent and what needs attention. Read-only."
    )]
    async fn cloud_pulls(
        &self,
        Parameters(p): Parameters<CloudPullsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "cloud.pulls",
            serde_json::json!({ "repository": p.repository, "state": p.state, "lane": p.lane, "attention": p.attention }),
        )
        .await
    }

    #[tool(
        name = "onus_cloud_pull",
        description = "One pull request in Onus Cloud: its lane and why, its analyses (with the judge's verdict) and the evidence attached to it. Read-only."
    )]
    async fn cloud_pull(
        &self,
        Parameters(p): Parameters<CloudPullParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "cloud.pull",
            serde_json::json!({ "repository": p.repository, "number": p.number }),
        )
        .await
    }

    #[tool(
        name = "onus_cloud_map",
        description = "Ask the map Onus Cloud keeps of a repository: at a commit it mapped, at a pull request's head, or (by default) at the default branch's newest commit. The same questions and answers as the map tools, answered here from the map the binary built in Onus Cloud; nothing is read from this worktree."
    )]
    async fn cloud_map(
        &self,
        Parameters(p): Parameters<CloudMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act_limited(
            "cloud.map",
            serde_json::json!({ "repository": p.repository, "commit": p.commit, "pull": p.pull, "question": p.question }),
        )
        .await
    }

    #[tool(
        name = "onus_cloud_token",
        description = "Mint a narrower Onus Cloud token for a sub-agent or a step of your task: only the scopes and repositories you name, never more than yours, ending within 24 hours. Pass it as ONUS_CLOUD_TOKEN."
    )]
    async fn cloud_token(
        &self,
        Parameters(p): Parameters<CloudTokenParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "cloud.tokenMint",
            serde_json::json!({ "scopes": p.scopes, "repositories": p.repositories, "allRepositories": p.all_repositories, "ttlSeconds": p.ttl_seconds, "label": p.label }),
        )
        .await
    }

    #[tool(
        name = "onus_cloud_attach_evidence",
        description = "Attach a run recorded in a hosted environment (onus_cloud_env_run) to a pull request as evidence. Onus Cloud analyses the pull request again when the run is for its head commit, and the judge checks the run against the recording. It approves nothing by itself."
    )]
    async fn cloud_attach_evidence(
        &self,
        Parameters(p): Parameters<CloudEvidenceParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.act(
            "cloud.attachEvidence",
            serde_json::json!({ "repository": p.repository, "number": p.number, "run": p.run }),
        )
        .await
    }
}

/// Text content of a tool result.
fn text_of(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The call refused because the audit log could not be written.
fn unlogged(e: anyhow::Error) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(format!("{e:#}; nothing was done"))]).into()
}

/// An action that ran, but whose outcome the audit log could not record.
fn ran_unlogged(e: anyhow::Error) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(format!(
        "{e:#}; the call itself ran (check what it did), but its answer is not given because it is not logged"
    ))])
    .into()
}

/// At most `n` characters of `s`.
fn clip(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

impl OnusMcp {
    /// Writes one decision on a call to the door's log.
    #[allow(clippy::too_many_arguments)]
    async fn log(
        &self,
        door: &Arc<Door>,
        tool: &str,
        args: &Value,
        subject: &str,
        decision: &str,
        reason: &str,
        extra: Value,
    ) -> Result<()> {
        let door = door.clone();
        let (tool, args, subject, decision, reason) = (
            tool.to_string(),
            args.clone(),
            subject.to_string(),
            decision.to_string(),
            reason.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            door.record(&tool, &args, &subject, &decision, &reason, extra)
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
    }

    /// Runs the tool, with a note for what its log entry needs.
    async fn routed(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> (Result<CallToolResponse, ErrorData>, Note) {
        let cell = Arc::new(Mutex::new(Note::default()));
        let response = NOTE
            .scope(
                cell.clone(),
                self.tool_router
                    .call(ToolCallContext::new(self, request, context)),
            )
            .await;
        let note = cell.lock().map(|mut n| std::mem::take(&mut *n));
        (response, note.unwrap_or_default())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OnusMcp {
    /// Every call: checked at the door, then run, then logged. A call the
    /// log cannot record is refused.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(door) = self.door.clone() else {
            return self.routed(request, context).await.0;
        };
        let tool = request.name.to_string();
        let args = Value::Object(request.arguments.clone().unwrap_or_default());
        let subject = door.subject(&tool, &args);
        let checked = {
            let (door, tool, args) = (door.clone(), tool.clone(), args.clone());
            tokio::task::spawn_blocking(move || door.authorize(&tool, &args, &door.facts()))
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        };
        if let Err(denial) = checked {
            let missing: Vec<String> = denial.missing.iter().map(|r| r.to_string()).collect();
            let logged = self
                .log(
                    &door,
                    &tool,
                    &args,
                    &subject,
                    "denied",
                    &denial.reason,
                    json!({ "missing": missing }),
                )
                .await;
            return Ok(match logged {
                Ok(()) => CallToolResult::error(vec![ContentBlock::text(denial.text())]).into(),
                Err(e) => unlogged(e),
            });
        }
        let allowed = match door.task() {
            Some(t) => format!("the token of task `{t}` allows it"),
            None => "no task token: a local server".to_string(),
        };

        if door::is_query(&tool) {
            let (response, note) = self.routed(request, context).await;
            let (decision, reason, extra) = match note.denial {
                Some(d) => {
                    let missing: Vec<String> = d.missing.iter().map(|r| r.to_string()).collect();
                    ("denied", d.reason, json!({ "missing": missing }))
                }
                None if note.hidden > 0 => ("allowed", allowed, json!({ "hidden": note.hidden })),
                None => ("allowed", allowed, json!({})),
            };
            if let Err(e) = self
                .log(&door, &tool, &args, &subject, decision, &reason, extra)
                .await
            {
                return Ok(unlogged(e));
            }
            return response;
        }

        // An action: allowed, then done or failed.
        if let Err(e) = self
            .log(
                &door,
                &tool,
                &args,
                &subject,
                "allowed",
                &allowed,
                json!({}),
            )
            .await
        {
            return Ok(unlogged(e));
        }
        let (response, note) = self.routed(request, context).await;
        let (decision, reason, extra) = match &response {
            Ok(CallToolResponse::Complete(r)) if r.is_error == Some(true) => {
                let why = clip(&text_of(r), 500);
                ("failed", why.clone(), json!({ "error": why }))
            }
            Ok(CallToolResponse::Complete(r)) => {
                let answer = note
                    .answer
                    .or_else(|| serde_json::from_str(&text_of(r)).ok())
                    .unwrap_or(Value::Null);
                (
                    "done",
                    "answered".to_string(),
                    json!({ "result": door::result_fields(&tool, &args, &subject, &answer) }),
                )
            }
            Ok(_) => ("done", "answered".to_string(), json!({ "result": {} })),
            Err(e) => {
                let why = clip(&e.message, 500);
                ("failed", why.clone(), json!({ "error": why }))
            }
        };
        if let Err(e) = self
            .log(&door, &tool, &args, &subject, decision, &reason, extra)
            .await
        {
            return Ok(ran_unlogged(e));
        }
        response
    }

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
                 evidence instead of working around it. If you are blocked (broken test, \
                 contradictory spec, impossible task), call onus_blocked with evidence and stop. \
                 Every call is checked against your task's token and logged; a denial says which \
                 right is missing and how to ask for it. When the onus_cloud_ tools are listed, this \
                 CLI is signed in to Onus Cloud: onus_cloud_env_create and onus_cloud_env_run build \
                 and run a pushed commit on Onus Cloud's machines and record the runs, which \
                 onus_cloud_attach_evidence attaches to the pull request for the judge. \
                 onus_cloud_map asks the map Onus Cloud keeps for a commit or pull request. A denial \
                 from Onus Cloud says which scope is missing; ask for it instead of working around it.",
            )
    }
}

/// What a server offers besides the map: the door every call goes through (with the task
/// token, if any), the tools that act, and whether the Onus Cloud tools are listed.
#[derive(Clone, Default)]
pub struct Tools {
    pub door: Option<Arc<Door>>,
    pub actions: Option<Arc<dyn Actions>>,
    pub cloud: bool,
}

impl std::fmt::Debug for Tools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tools")
            .field("door", &self.door)
            .field("actions", &self.actions.is_some())
            .field("cloud", &self.cloud)
            .finish()
    }
}

impl OnusMcp {
    fn with_tools(self, tools: &Tools) -> OnusMcp {
        self.with_door(tools.door.clone())
            .with_cloud(tools.cloud)
            .with_actions(tools.actions.clone())
    }
}

/// Serves MCP over streamable HTTP on `addr` until the process stops.
/// Requests must name an allowed host (`Host` header): loopback names by
/// default, which keeps other machines and DNS rebinding out. `ready` is
/// called with the bound address (useful with port 0).
pub fn serve_http(
    root: PathBuf,
    backend: Arc<dyn Backend>,
    tools: Tools,
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
            move || Ok(OnusMcp::new(root.clone(), backend.clone()).with_tools(&tools)),
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
pub fn serve_stdio(root: PathBuf, backend: Arc<dyn Backend>, tools: Tools) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let service = OnusMcp::new(root, backend)
            .with_tools(&tools)
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok(())
    })
}
