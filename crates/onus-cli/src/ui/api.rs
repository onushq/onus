//! The JSON API behind `onus ui`: one function per action, each calling the
//! same code as the matching command. Requests are `POST /api/<name>` with a
//! JSON body; answers are JSON.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component as PathPart, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, anyhow, bail};
use onus_core::{EdgeKind, Lane, ids};
use onus_index::Query;
use serde::Deserialize;
use serde_json::{Value, json};

use super::State;
use crate::{doors, envs, lanes};

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Error {
        Error::Failed(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Failed(e.into())
    }
}

type Answer = Result<Value, Error>;

fn input<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T, Error> {
    serde_json::from_value(v).map_err(|e| Error::Failed(anyhow!("bad request: {e}")))
}

pub fn call(state: &State, name: &str, body: Value) -> Answer {
    match name {
        "status" => status(state),
        "refs" => refs(state),
        "map.query" => map_query(state, body),
        "map.graph" => map_graph(state),
        "map.files" => map_files(state, body),
        "files.read" => files_read(state, body),
        "check" => check(state, body),
        "report" => report(state, body),
        "config" => config(state),
        "config.init" => Ok(json!({ "text": onus_map::init::init_config(&state.root) })),
        "guide" => guide(body),
        "lanes.policy" => lanes_policy(state),
        "lanes.submit" => lanes_submit(state, body),
        "lanes.classify" => lanes_classify(state, body),
        "lanes.judge" => lanes_judge(state, body),
        "submissions.list" => submissions_list(state),
        "submissions.show" => submissions_show(state, body),
        "submissions.save" => submissions_save(state, body),
        "submissions.judge" => submissions_judge(state, body),
        "outcomes" => outcomes(state),
        "outcomes.record" => outcomes_record(state, body),
        "outcomes.incident" => outcomes_incident(state, body),
        "outcomes.ingestReverts" => outcomes_ingest(state, body),
        "envs.engine" => envs_engine(state),
        "envs.list" => Ok(serde_json::to_value(onus_env::env::list()?)?),
        "envs.create" => envs_create(state, body),
        "envs.run" => envs_run(state, body),
        "envs.destroy" => envs_destroy(body),
        "evidence.list" => evidence_list(state),
        "evidence.show" => evidence_show(state, body),
        "evidence.artifact" => evidence_artifact(state, body),
        "runTest" => run_test(state, body),
        "keys.status" => keys_status(state),
        "keys.generate" => keys_generate(state),
        "token.inspect" => token_inspect(state, body),
        "token.check" => token_check(state, body),
        "token.attenuate" => token_attenuate(state, body),
        "token.mint" => token_mint(state, body),
        "scope.suggest" => scope_suggest(state, body),
        "escalations.list" => escalations_list(state),
        "escalations.create" => escalations_create(state, body),
        "escalations.decide" => escalations_decide(state, body),
        "escalations.grant" => escalations_grant(state, body),
        "escalations.deny" => escalations_deny(state, body),
        "audit" => audit(state),
        _ => Err(Error::NotFound(format!("no API `{name}`"))),
    }
}

fn git(root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("cannot run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn loaded_config(state: &State) -> anyhow::Result<Option<onus_map::LoadedConfig>> {
    Ok(onus_map::config::load_from_tree(&state.root)?)
}

// ---------------------------------------------------------------------------
// The repository and its map

fn status(state: &State) -> Answer {
    let root = &state.root;
    let branch = git(root, &["branch", "--show-current"]).unwrap_or_default();
    let head = git(root, &["log", "-1", "--format=%H%x1f%s%x1f%an%x1f%ct"]).unwrap_or_default();
    let mut parts = head.split('\u{1f}');
    let commit = json!({
        "sha": parts.next().unwrap_or(""),
        "subject": parts.next().unwrap_or(""),
        "author": parts.next().unwrap_or(""),
        "at": parts.next().and_then(|t| t.parse::<u64>().ok()).unwrap_or(0),
    });
    let dirty: Vec<String> = git(root, &["status", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .map(|l| l.get(3..).unwrap_or("").to_string())
        .collect();
    let session = state.server.session(root)?;
    let (index, meta) = session.index()?;
    let config = loaded_config(state).ok().flatten();
    let c = config.as_ref().map(|c| &c.config);
    let store = onus_env::store::Store::for_repo(root).ok();
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "root": root,
        "name": root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        "branch": branch,
        "head": commit,
        "dirty": dirty,
        "map": index.status(),
        "mapMeta": meta,
        "config": {
            "exists": config.is_some(),
            "components": c.map_or(0, |c| c.components.len()),
            "contracts": c.map_or(0, |c| c.contracts.len()),
            "rules": c.map_or(0, |c| c.rules.len()),
            "lanes": c.is_some_and(|c| c.lanes.is_some()),
            "environment": c.is_some_and(|c| c.environment.is_some()),
        },
        "paths": {
            "outcomes": state.outcomes,
            "audit": state.audit,
            "escalations": state.escalations,
            "keys": state.keys,
            "evidence": store.as_ref().map(|s| s.root().to_path_buf()),
        },
    }))
}

fn refs(state: &State) -> Answer {
    let root = &state.root;
    let line = |l: &str| -> Value {
        let p: Vec<&str> = l.split('\u{1f}').collect();
        json!({
            "name": p.first().copied().unwrap_or(""),
            "sha": p.get(1).copied().unwrap_or(""),
            "at": p.get(2).and_then(|t| t.parse::<u64>().ok()).unwrap_or(0),
            "subject": p.get(3).copied().unwrap_or(""),
        })
    };
    let refs: Vec<Value> = git(
        root,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--count=300",
            "--format=%(refname:short)%1f%(objectname:short)%1f%(committerdate:unix)%1f%(subject)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?
    .lines()
    .filter(|l| !l.starts_with("origin/HEAD") && !l.is_empty())
    .map(line)
    .collect();
    let commits: Vec<Value> = git(root, &["log", "-60", "--format=%h%x1f%h%x1f%ct%x1f%s"])
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(line)
        .collect();
    let default = git(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .or_else(|| {
        ["main", "master"]
            .iter()
            .find(|b| git(root, &["rev-parse", "--verify", "--quiet", b]).is_ok())
            .map(|b| b.to_string())
    });
    Ok(json!({ "refs": refs, "commits": commits, "default": default }))
}

fn map_query(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        query: Query,
    }
    let q: In = input(body)?;
    let session = state.server.session(&state.root)?;
    let (index, _) = session.index()?;
    index
        .answer(&q.query)
        .map_err(|e| Error::Failed(anyhow!(e)))
}

/// Components and how they depend on each other, for drawing.
fn map_graph(state: &State) -> Answer {
    let session = state.server.session(&state.root)?;
    let (index, _) = session.index()?;
    let map = index.map();
    let mut files: BTreeMap<&str, (u32, u32)> = BTreeMap::new();
    for f in &map.files {
        if let Some(c) = &f.component_id {
            let e = files.entry(c.as_str()).or_default();
            e.0 += 1;
            e.1 += f.lines;
        }
    }
    let mut public: BTreeMap<&str, u32> = BTreeMap::new();
    for s in &map.symbols {
        if s.visibility == onus_core::Visibility::Public
            && let Some(c) = ids::component_of(&s.id)
        {
            *public.entry(c).or_default() += 1;
        }
    }
    // Code dependencies between components, by kind.
    let mut deps: BTreeMap<(String, String), BTreeMap<&str, u32>> = BTreeMap::new();
    let mut publishes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut consumes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut externals: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in &map.edges {
        let Some(from) = ids::component_of(&e.from) else {
            continue;
        };
        match e.kind {
            EdgeKind::Publishes => {
                publishes
                    .entry(ids::name_of(&e.to).to_string())
                    .or_default()
                    .insert(from.to_string());
            }
            EdgeKind::Consumes => {
                consumes
                    .entry(ids::name_of(&e.to).to_string())
                    .or_default()
                    .insert(from.to_string());
            }
            EdgeKind::CallsExternal => {
                externals
                    .entry(from.to_string())
                    .or_default()
                    .insert(e.to.clone());
            }
            k if k.is_code_dependency() => {
                if let Some(to) = ids::component_of(&e.to).filter(|t| *t != from) {
                    *deps
                        .entry((from.to_string(), to.to_string()))
                        .or_default()
                        .entry(k.as_str())
                        .or_default() += 1;
                }
            }
            _ => {}
        }
    }
    let components: Vec<Value> = map
        .components
        .iter()
        .map(|c| {
            let (n, lines) = files.get(c.id.as_str()).copied().unwrap_or((0, 0));
            json!({
                "id": c.id,
                "kind": c.kind,
                "roots": c.roots,
                "owners": c.owners,
                "labels": c.labels,
                "packageName": c.package_name,
                "files": n,
                "lines": lines,
                "publicSymbols": public.get(c.id.as_str()).copied().unwrap_or(0),
                "externals": externals.get(&c.id).cloned().unwrap_or_default(),
            })
        })
        .collect();
    let edges: Vec<Value> = deps
        .into_iter()
        .map(|((from, to), kinds)| {
            json!({ "from": from, "to": to, "count": kinds.values().sum::<u32>(), "kinds": kinds })
        })
        .collect();
    let mut events = Vec::new();
    let names: BTreeSet<&String> = publishes.keys().chain(consumes.keys()).collect();
    for name in names {
        events.push(json!({
            "name": name,
            "publishers": publishes.get(name).cloned().unwrap_or_default(),
            "consumers": consumes.get(name).cloned().unwrap_or_default(),
        }));
    }
    let rules: Vec<Value> = map
        .rules
        .iter()
        .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
        .collect();
    Ok(json!({
        "components": components,
        "edges": edges,
        "events": events,
        "externals": map.externals,
        "rules": rules,
        "diagnostics": map.diagnostics,
    }))
}

fn map_files(state: &State, body: Value) -> Answer {
    #[derive(Deserialize, Default)]
    struct In {
        #[serde(default)]
        component: Option<String>,
    }
    let q: In = input(body)?;
    let session = state.server.session(&state.root)?;
    let (index, _) = session.index()?;
    let map = index.map();
    let files: Vec<Value> = map
        .files
        .iter()
        .filter(|f| q.component.is_none() || f.component_id == q.component)
        .map(|f| {
            json!({
                "path": f.path,
                "component": f.component_id,
                "language": f.language,
                "isTest": f.is_test,
                "lines": f.lines,
            })
        })
        .collect();
    Ok(json!({ "files": files }))
}

/// A path inside the worktree, refusing anything that leaves it.
fn inside(root: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, PathPart::Normal(_) | PathPart::CurDir))
    {
        bail!("`{rel}` is not a path inside the repository");
    }
    let full = root.join(p);
    let canonical = onus_cli::daemon::canonical(&full).with_context(|| format!("no file {rel}"))?;
    if !canonical.starts_with(root) {
        bail!("`{rel}` leads outside the repository");
    }
    Ok(canonical)
}

fn files_read(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        path: String,
    }
    let q: In = input(body)?;
    let path = inside(&state.root, &q.path)?;
    let bytes = std::fs::read(&path).with_context(|| format!("cannot read {}", q.path))?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(Error::Failed(anyhow!("{} is larger than 2 MB", q.path)));
    }
    if bytes.contains(&0) {
        return Err(Error::Failed(anyhow!("{} is not a text file", q.path)));
    }
    Ok(json!({ "path": q.path, "text": String::from_utf8_lossy(&bytes) }))
}

// ---------------------------------------------------------------------------
// Reports

fn markdown(report: &onus_core::SemanticReport, rules: bool) -> String {
    onus_report::to_markdown(report, rules)
}

fn check(state: &State, body: Value) -> Answer {
    #[derive(Deserialize, Default)]
    struct In {
        #[serde(default)]
        base: Option<String>,
    }
    let q: In = input(body)?;
    let base = q.base.unwrap_or_else(|| "HEAD".into());
    let report = state.server.worktree_report(&state.root, &base)?;
    let rules = !report.rule_violations.is_empty()
        || loaded_config(state)?.is_some_and(|c| !c.config.rules.is_empty());
    Ok(json!({
        "report": report,
        "markdown": markdown(&report, rules),
        "checklist": onus_cli::checklist(&report),
    }))
}

fn intent_of(text: Option<&str>) -> anyhow::Result<Option<onus_diff::Intent>> {
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return Ok(None);
    };
    // Markdown (a pull request body) holds an `onus-intent` block; anything
    // else is the YAML itself.
    if text.contains("```onus-intent") {
        Ok(onus_diff::intent::parse_markdown(text)?)
    } else {
        Ok(onus_diff::intent::parse(text)?)
    }
}

fn report(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        base: String,
        head: String,
        #[serde(default)]
        intent: Option<String>,
    }
    let q: In = input(body)?;
    let pair = std::sync::Arc::new(onus_cli::materialize_pair(
        &state.root,
        &q.base,
        &q.head,
        true,
    )?);
    let completer = {
        let pair = pair.clone();
        onus_cli::BaseCompleter(std::sync::Arc::new(move || pair.complete_base()))
    };
    let cache_dir =
        onus_cli::daemon::git_common_dir(&state.root).map(|d| d.join("onus").join("maps"));
    let opts = onus_cli::DiffOptions {
        intent: intent_of(q.intent.as_deref())?,
        base_label: onus_cli::ref_label(&q.base, &pair.base.sha),
        head_label: onus_cli::ref_label(&q.head, &pair.head.sha),
        base_commit: Some(pair.base.sha.clone()),
        head_commit: Some(pair.head.sha.clone()),
        cache_dir,
        changed_paths: Some(pair.changed.clone()),
        complete_base: Some(completer),
        ..Default::default()
    };
    let outcome = onus_cli::diff_dirs(pair.base.dir.path(), pair.head.dir.path(), &opts)?;
    let config = lanes::lanes_config(&state.root, None)?;
    let classification = lanes::classify_with(&outcome.report, None, &config, None)?;
    Ok(json!({
        "report": outcome.report,
        "markdown": outcome.render(onus_cli::Format::Md),
        "checklist": onus_cli::checklist(&outcome.report),
        "classification": classification,
        "notes": outcome.notes,
        "baseSha": pair.base.sha,
        "headSha": pair.head.sha,
    }))
}

fn config(state: &State) -> Answer {
    let path = state.root.join(onus_map::config::CONFIG_FILE);
    let text = std::fs::read_to_string(&path).ok();
    let (parsed, error) = match loaded_config(state) {
        Ok(Some(c)) => (Some(serde_json::to_value(&c.config)?), None),
        Ok(None) => (None, None),
        Err(e) => (None, Some(format!("{e:#}"))),
    };
    Ok(json!({
        "path": path,
        "exists": text.is_some(),
        "text": text,
        "parsed": parsed,
        "error": error,
    }))
}

fn guide(body: Value) -> Answer {
    #[derive(Deserialize, Default)]
    struct In {
        #[serde(default)]
        name: Option<String>,
    }
    let q: In = input(body)?;
    let topics = onus_cli::guide::TOPICS;
    match q.name {
        None => Ok(json!({
            "topics": topics.iter().map(|t| json!({ "name": t.name, "summary": t.summary })).collect::<Vec<_>>()
        })),
        Some(name) => {
            let t = topics
                .iter()
                .find(|t| t.name == name)
                .ok_or_else(|| Error::NotFound(format!("no guide topic `{name}`")))?;
            Ok(json!({ "name": t.name, "summary": t.summary, "text": t.text }))
        }
    }
}

// ---------------------------------------------------------------------------
// Lanes and outcomes

fn lanes_policy(state: &State) -> Answer {
    let config = loaded_config(state)?;
    let lanes = config.as_ref().and_then(|c| c.config.lanes.clone());
    Ok(json!({
        "configured": lanes.is_some(),
        "policy": lanes.unwrap_or_default(),
        "judge": onus_lanes::judge::judge_id(&config.and_then(|c| c.config.lanes).unwrap_or_default()),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitIn {
    base: String,
    head: String,
    #[serde(default)]
    intent: Option<String>,
    /// Evidence runs by manifest id.
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    escalations: Vec<String>,
    /// `<row id>=<who>`.
    #[serde(default)]
    approvals: Vec<String>,
    #[serde(default)]
    agent: onus_lanes::submission::AgentSetup,
}

fn lanes_submit(state: &State, body: Value) -> Answer {
    let q: SubmitIn = input(body)?;
    let store = onus_env::store::Store::for_repo(&state.root)?;
    let evidence = q
        .evidence
        .iter()
        .map(|id| {
            let (full, m) = store.manifest(id)?;
            envs::test_run_of(&store, full, &m)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let scope = match q.token.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => Some(lanes::scope_used(t, &state.public_key)?),
        None => None,
    };
    let approvals = q
        .approvals
        .iter()
        .map(|a| lanes::parse_approval(a))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut agent = q.agent;
    if agent.tool.is_empty() {
        agent.tool = "unknown".into();
    }
    let intent_markdown = q
        .intent
        .as_deref()
        .is_some_and(|t| t.contains("```onus-intent"));
    let sub = lanes::make_submission(
        &state.root,
        &q.base,
        &q.head,
        lanes::SubmissionParts {
            intent: q.intent.filter(|t| !t.trim().is_empty()),
            intent_markdown,
            evidence,
            scope,
            escalations: q.escalations,
            approvals,
            agent,
        },
    )?;
    Ok(serde_json::to_value(sub)?)
}

fn submission_in(body: Value) -> Result<onus_lanes::submission::Submission, Error> {
    #[derive(Deserialize)]
    struct In {
        submission: onus_lanes::submission::Submission,
    }
    Ok(input::<In>(body)?.submission)
}

fn lanes_classify(state: &State, body: Value) -> Answer {
    let sub = submission_in(body)?;
    let config = lanes::lanes_config(&state.root, None)?;
    let c = lanes::classify_with(&sub.report, Some(&sub), &config, Some(&state.outcomes))?;
    Ok(serde_json::to_value(c)?)
}

fn lanes_judge(state: &State, body: Value) -> Answer {
    let sub = submission_in(body)?;
    let config = lanes::lanes_config(&state.root, None)?;
    let c = lanes::classify_with(&sub.report, Some(&sub), &config, Some(&state.outcomes))?;
    let j = lanes::judge_submission(&state.root, &sub, &c, &config);
    Ok(json!({ "classification": c, "judgment": j }))
}

// Submissions handed in by agents, kept as <id>.json with the judge's
// verdict beside them as <id>.judgment.json.

fn submission_path(state: &State, id: &str, suffix: &str) -> anyhow::Result<PathBuf> {
    if id.len() < 6 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("`{id}` is not a submission id");
    }
    Ok(state.submissions.join(format!("{id}{suffix}")))
}

fn submissions_save(state: &State, body: Value) -> Answer {
    let sub = submission_in(body)?;
    let json = serde_json::to_vec_pretty(&sub)?;
    let id: String = {
        use sha2::Digest;
        sha2::Sha256::digest(&json)[..6]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    std::fs::create_dir_all(&state.submissions).map_err(anyhow::Error::from)?;
    std::fs::write(submission_path(state, &id, ".json")?, json).map_err(anyhow::Error::from)?;
    Ok(json!({ "id": id }))
}

fn load_submission(state: &State, id: &str) -> Result<onus_lanes::submission::Submission, Error> {
    let path = submission_path(state, id, ".json")?;
    let text = std::fs::read_to_string(&path)
        .map_err(|_| Error::NotFound(format!("no submission {id}")))?;
    Ok(serde_json::from_str(&text)?)
}

fn load_judgment(state: &State, id: &str) -> Option<Value> {
    std::fs::read_to_string(submission_path(state, id, ".judgment.json").ok()?)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn submissions_list(state: &State) -> Answer {
    let mut out = Vec::new();
    if let Ok(dir) = std::fs::read_dir(&state.submissions) {
        for e in dir.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_suffix(".json").filter(|n| !n.contains('.')) else {
                continue;
            };
            let Ok(sub) = load_submission(state, id) else {
                continue;
            };
            let at = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let judgment = load_judgment(state, id);
            out.push(json!({
                "id": id,
                "at": at,
                "base": sub.base,
                "head": sub.head,
                "agent": sub.agent,
                "intent": sub.intent,
                "rows": sub.report.changes.len(),
                "needsPerson": sub.report.summary.needs_attention,
                "evidence": sub.evidence.len(),
                "task": sub.scope.as_ref().map(|s| s.task.clone()),
                "verdict": judgment.as_ref().map(|j| j["judgment"]["verdict"].clone()),
                "lane": judgment.as_ref().map(|j| j["judgment"]["lane"].clone()),
            }));
        }
    }
    out.sort_by(|a, b| b["at"].as_u64().cmp(&a["at"].as_u64()));
    Ok(json!({ "dir": state.submissions, "submissions": out }))
}

#[derive(Deserialize)]
struct IdIn {
    id: String,
}

fn submissions_show(state: &State, body: Value) -> Answer {
    let q: IdIn = input(body)?;
    let sub = load_submission(state, &q.id)?;
    Ok(json!({ "id": q.id, "submission": sub, "judgment": load_judgment(state, &q.id) }))
}

fn submissions_judge(state: &State, body: Value) -> Answer {
    let q: IdIn = input(body)?;
    let sub = load_submission(state, &q.id)?;
    let out = lanes_judge(state, json!({ "submission": sub }))?;
    std::fs::write(
        submission_path(state, &q.id, ".judgment.json")?,
        serde_json::to_vec_pretty(&out)?,
    )
    .map_err(anyhow::Error::from)?;
    Ok(out)
}

fn load_outcomes(state: &State) -> anyhow::Result<Vec<onus_lanes::outcomes::Outcome>> {
    if state.outcomes.exists() {
        onus_lanes::outcomes::load(&state.outcomes)
    } else {
        Ok(Vec::new())
    }
}

fn outcomes(state: &State) -> Answer {
    let all = load_outcomes(state)?;
    let backlog: Vec<Value> = onus_lanes::outcomes::backlog(&all)
        .into_iter()
        .map(|(what, n)| json!({ "target": what, "incidents": n }))
        .collect();
    Ok(json!({
        "file": state.outcomes,
        "records": all,
        "summary": onus_lanes::outcomes::summarize(&all),
        "backlog": backlog,
        "results": lanes::RESULTS,
    }))
}

fn outcomes_record(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        change: String,
        agent: String,
        lane: Lane,
        result: String,
        #[serde(default)]
        verdict: Option<String>,
        #[serde(default)]
        judge: Option<String>,
        #[serde(default)]
        audited: bool,
        #[serde(default)]
        missed: bool,
        #[serde(default)]
        commit: Option<String>,
    }
    let q: In = input(body)?;
    let blank = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    lanes::record_outcome(
        &state.outcomes,
        &onus_lanes::outcomes::Outcome {
            at: onus_doors::gateway::now(),
            change: q.change,
            agent: q.agent,
            judge: blank(q.judge),
            lane: q.lane,
            verdict: blank(q.verdict),
            result: q.result,
            audited: q.audited,
            missed: q.missed,
            commit: blank(q.commit),
            involved: vec![],
            note: None,
        },
    )?;
    outcomes(state)
}

fn outcomes_incident(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        change: String,
        #[serde(default)]
        involved: Vec<String>,
        note: String,
    }
    let q: In = input(body)?;
    lanes::record_incident(&state.outcomes, &q.change, q.involved, q.note)?;
    outcomes(state)
}

fn outcomes_ingest(state: &State, body: Value) -> Answer {
    #[derive(Deserialize, Default)]
    struct In {
        #[serde(default)]
        since: Option<String>,
    }
    let q: In = input(body)?;
    if let Some(dir) = state.outcomes.parent() {
        std::fs::create_dir_all(dir).map_err(anyhow::Error::from)?;
    }
    let (found, added) = lanes::ingest_reverts(
        &state.outcomes,
        &state.root,
        q.since.as_deref().filter(|s| !s.trim().is_empty()),
    )?;
    let mut out = outcomes(state)?;
    out["ingested"] = json!({ "found": found, "recorded": added });
    Ok(out)
}

// ---------------------------------------------------------------------------
// Environments and evidence

fn envs_engine(state: &State) -> Answer {
    let engine = onus_doors::runner::engine();
    let config = loaded_config(state).ok().flatten();
    let spec = onus_env::spec::resolve(
        &state.root,
        config.as_ref().and_then(|c| c.config.environment.as_ref()),
    );
    Ok(json!({
        "engine": engine.as_ref().ok(),
        "engineError": engine.err().map(|e| format!("{e:#}")),
        "spec": spec.as_ref().ok(),
        "specError": spec.err().map(|e| format!("{e:#}")),
    }))
}

fn envs_create(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        #[serde(rename = "ref")]
        reference: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        token: Option<String>,
    }
    let q: In = input(body)?;
    let token = q.token.filter(|t| !t.trim().is_empty());
    let config = loaded_config(state)?;
    let spec = onus_env::spec::resolve(
        &state.root,
        config.as_ref().and_then(|c| c.config.environment.as_ref()),
    )?;
    let grants = envs::grants(
        token.as_deref(),
        token.as_ref().map(|_| state.public_key.as_path()),
    )?;
    let name = match q.name.filter(|n| !n.trim().is_empty()) {
        Some(n) => n,
        None => envs::short_commit(&state.root, &q.reference)?,
    };
    let e = onus_env::env::create(&state.root, &q.reference, &name, &spec, &grants)?;
    Ok(serde_json::to_value(e)?)
}

fn envs_run(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        name: String,
        command: String,
    }
    let q: In = input(body)?;
    let store = onus_env::store::Store::for_repo(&state.root)?;
    let (id, m) = onus_env::env::run(&q.name, &q.command, &store)?;
    let run = envs::test_run_of(&store, id.clone(), &m)?;
    Ok(json!({ "id": id, "manifest": m, "run": run }))
}

fn envs_destroy(body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        name: String,
    }
    let q: In = input(body)?;
    Ok(json!({ "removed": onus_env::env::destroy(&q.name)? }))
}

fn evidence_list(state: &State) -> Answer {
    let store = onus_env::store::Store::for_repo(&state.root)?;
    let runs: Vec<Value> = store
        .list()?
        .into_iter()
        .rev()
        .map(|(id, m)| json!({ "id": id, "manifest": m }))
        .collect();
    Ok(json!({ "store": store.root(), "runs": runs }))
}

fn evidence_show(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        id: String,
    }
    let q: In = input(body)?;
    let store = onus_env::store::Store::for_repo(&state.root)?;
    let (id, m) = store.manifest(&q.id)?;
    Ok(json!({ "id": id, "manifest": m }))
}

fn evidence_artifact(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        id: String,
        path: String,
    }
    let q: In = input(body)?;
    let store = onus_env::store::Store::for_repo(&state.root)?;
    let (id, m) = store.manifest(&q.id)?;
    let a = m
        .artifacts
        .iter()
        .find(|a| a.path == q.path)
        .ok_or_else(|| Error::NotFound(format!("run {} has no artifact {}", &id[..12], q.path)))?;
    let bytes = store.get(&a.sha256)?;
    Ok(json!({ "artifact": a, "text": String::from_utf8_lossy(&bytes) }))
}

fn run_test(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        #[serde(rename = "ref")]
        reference: String,
        image: String,
        #[serde(default)]
        setup: Option<String>,
        command: String,
    }
    let q: In = input(body)?;
    let run = onus_doors::runner::run(
        &state.root,
        &q.reference,
        &q.image,
        q.setup.as_deref().filter(|s| !s.trim().is_empty()),
        &q.command,
    )?;
    Ok(serde_json::to_value(run)?)
}

// ---------------------------------------------------------------------------
// Keys, tokens and scopes

fn keys_status(state: &State) -> Answer {
    let public = state.keys.join("root.pub");
    Ok(json!({
        "dir": state.keys,
        "private": state.keys.join("root.key").exists(),
        "public": public.exists(),
        "publicKey": std::fs::read_to_string(&public).ok().map(|k| k.trim().to_string()),
    }))
}

fn keys_generate(state: &State) -> Answer {
    doors::keygen(&state.keys)?;
    keys_status(state)
}

fn verified(state: &State, token: &str) -> anyhow::Result<onus_doors::token::Verified> {
    let public = doors::read_key(&state.public_key)?;
    onus_doors::token::verify(token, &onus_doors::token::public_key(&public)?)
}

#[derive(Deserialize)]
struct TokenIn {
    token: String,
}

fn token_inspect(state: &State, body: Value) -> Answer {
    let q: TokenIn = input(body)?;
    let v = verified(state, &q.token)?;
    Ok(doors::token_json(&v))
}

fn token_check(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        token: String,
        right: String,
    }
    let q: In = input(body)?;
    let v = verified(state, &q.token)?;
    let r: onus_doors::scope::Right = q.right.parse().map_err(|e: String| anyhow!(e))?;
    Ok(
        match v.authorize(r.kind, &r.pattern, onus_doors::gateway::now()) {
            Ok(()) => json!({ "allowed": true, "right": r.to_string() }),
            Err(e) => {
                json!({ "allowed": false, "right": r.to_string(), "reason": format!("{e:#}") })
            }
        },
    )
}

fn token_attenuate(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        token: String,
        only: Vec<String>,
    }
    let q: In = input(body)?;
    let v = verified(state, &q.token)?;
    let rights = q
        .only
        .iter()
        .map(|s| s.parse().map_err(|e: String| anyhow!(e)))
        .collect::<anyhow::Result<Vec<onus_doors::scope::Right>>>()?;
    Ok(json!({ "token": v.attenuate(&rights)? }))
}

#[derive(Deserialize)]
struct PlanIn {
    plan: String,
}

fn token_mint(state: &State, body: Value) -> Answer {
    let q: PlanIn = input(body)?;
    let plan = onus_doors::plan::Plan::parse(&q.plan)?;
    let audit_dir = state.audit.parent().map(Path::to_path_buf);
    if let Some(d) = audit_dir {
        std::fs::create_dir_all(d).map_err(anyhow::Error::from)?;
    }
    let token = doors::mint_plan(
        &state.root,
        &plan,
        &state.keys.join("root.key"),
        Some(&state.audit),
    )?;
    let v = verified(state, &token)?;
    Ok(json!({ "token": token, "inspect": doors::token_json(&v) }))
}

fn scope_suggest(state: &State, body: Value) -> Answer {
    let q: PlanIn = input(body)?;
    let plan = onus_doors::plan::Plan::parse(&q.plan)?;
    Ok(serde_json::to_value(doors::suggest(&state.root, &plan)?)?)
}

// ---------------------------------------------------------------------------
// Escalations and the audit log

pub fn request_path(state: &State, id: &str) -> anyhow::Result<PathBuf> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        bail!("`{id}` is not a request id");
    }
    Ok(state.escalations.join(format!("{id}.json")))
}

/// Decisions on record in the audit log, by request id.
pub fn decisions(state: &State) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Ok(entries) = onus_doors::audit::AuditLog::new(&state.audit).verify() else {
        return out;
    };
    for e in entries {
        if matches!(e.action.as_str(), "escalate" | "grant" | "deny") {
            out.insert(
                e.subject.clone(),
                json!({ "action": e.action, "decision": e.decision, "by": e.actor, "reason": e.reason, "at": e.at }),
            );
        }
    }
    out
}

fn escalations_list(state: &State) -> Answer {
    let decided = decisions(state);
    let mut requests = Vec::new();
    if let Ok(dir) = std::fs::read_dir(&state.escalations) {
        for e in dir.filter_map(Result::ok) {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Ok(req) = doors::load_request(&path) {
                let decision = decided.get(&req.id).cloned();
                requests.push(json!({ "request": req, "decision": decision, "file": path }));
            }
        }
    }
    requests.sort_by(|a, b| {
        b["request"]["at"]
            .as_u64()
            .cmp(&a["request"]["at"].as_u64())
    });
    Ok(json!({ "dir": state.escalations, "requests": requests }))
}

fn escalations_create(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        task: String,
        kind: onus_doors::escalation::EscalationKind,
        #[serde(default)]
        scopes: Vec<String>,
        #[serde(default)]
        evidence: Vec<String>,
        reason: String,
    }
    let q: In = input(body)?;
    let req = doors::new_request(
        &state.root,
        &q.task,
        q.kind,
        &q.scopes,
        &q.evidence,
        &q.reason,
    )?;
    std::fs::create_dir_all(&state.escalations).map_err(anyhow::Error::from)?;
    let path = request_path(state, &req.id)?;
    std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&req)?))
        .map_err(anyhow::Error::from)?;
    Ok(json!({ "request": req, "file": path }))
}

fn load(state: &State, id: &str) -> anyhow::Result<onus_doors::escalation::Request> {
    doors::load_request(&request_path(state, id)?)
}

fn ensure_audit_dir(state: &State) -> anyhow::Result<()> {
    if let Some(d) = state.audit.parent() {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}

fn keep_token(state: &State, id: &str, token: &str) -> anyhow::Result<()> {
    doors::keep_token(&request_path(state, id)?, token)
}

/// A granted token kept for request `id`, if any.
pub fn kept_token(state: &State, id: &str) -> Option<String> {
    std::fs::read_to_string(request_path(state, id).ok()?.with_extension("token"))
        .ok()
        .map(|t| t.trim().to_string())
}

fn escalations_decide(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct In {
        id: String,
        token: String,
        #[serde(default)]
        reproduce_at: Option<String>,
        #[serde(default)]
        image: Option<String>,
        #[serde(default)]
        setup: Option<String>,
        #[serde(default)]
        test_command: Option<String>,
        #[serde(default)]
        max_blast_radius: Option<u32>,
    }
    let q: In = input(body)?;
    let req = load(state, &q.id)?;
    let original = verified(state, &q.token)?;
    let reproduce = q
        .reproduce_at
        .filter(|r| !r.trim().is_empty())
        .map(|at| doors::Reproduce {
            repo: state.root.clone(),
            at,
            image: q
                .image
                .filter(|i| !i.is_empty())
                .unwrap_or_else(|| "node:22".into()),
            setup: q.setup.filter(|s| !s.trim().is_empty()),
            test_command: q
                .test_command
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "npx vitest run {test}".into()),
        });
    ensure_audit_dir(state)?;
    let id = req.id.clone();
    let out = doors::decide_request(
        req,
        &original,
        &state.keys.join("root.key"),
        reproduce.as_ref(),
        q.max_blast_radius.unwrap_or(20),
        Some(&state.audit),
    )?;
    if let Some(t) = out["token"].as_str() {
        keep_token(state, &id, t)?;
    }
    Ok(out)
}

fn escalations_grant(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        id: String,
        token: String,
        by: String,
    }
    let q: In = input(body)?;
    let req = load(state, &q.id)?;
    let original = verified(state, &q.token)?;
    ensure_audit_dir(state)?;
    let token = doors::grant_request(
        &req,
        &original,
        &state.keys.join("root.key"),
        &q.by,
        Some(&state.audit),
    )?;
    keep_token(state, &req.id, &token)?;
    Ok(json!({ "token": token }))
}

fn escalations_deny(state: &State, body: Value) -> Answer {
    #[derive(Deserialize)]
    struct In {
        id: String,
        by: String,
        reason: String,
    }
    let q: In = input(body)?;
    let req = load(state, &q.id)?;
    ensure_audit_dir(state)?;
    doors::deny_request(&req, &q.by, &q.reason, Some(&state.audit))?;
    Ok(json!({ "denied": req.id }))
}

fn audit(state: &State) -> Answer {
    if !state.audit.exists() {
        return Ok(json!({ "file": state.audit, "exists": false, "entries": [], "intact": true }));
    }
    match onus_doors::audit::AuditLog::new(&state.audit).verify() {
        Ok(entries) => {
            Ok(json!({ "file": state.audit, "exists": true, "entries": entries, "intact": true }))
        }
        Err(e) => {
            // Show what is there, and where the chain breaks.
            let raw: Vec<Value> = std::fs::read_to_string(&state.audit)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            Ok(
                json!({ "file": state.audit, "exists": true, "entries": raw, "intact": false, "error": format!("{e:#}") }),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = onus_cli::daemon::canonical(dir.path()).unwrap();
        std::fs::write(root.join("a.ts"), "x").unwrap();
        assert!(inside(&root, "a.ts").is_ok());
        assert!(inside(&root, "../etc/passwd").is_err());
        assert!(inside(&root, "/etc/passwd").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", root.join("out")).unwrap();
            assert!(inside(&root, "out/passwd").is_err());
        }
    }
}
