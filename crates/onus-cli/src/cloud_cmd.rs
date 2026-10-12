//! `onus login`, `onus logout` and `onus cloud`: the hosted service from the terminal.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use onus_cli::cloud::{self, Client, CloudDoor, segment};
use serde_json::{Value, json};

use crate::cloud_doors::{self, EscalationDoor};

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// The Onus Cloud API (default: $ONUS_CLOUD_URL, else https://api.onushq.com).
    #[arg(long, env = "ONUS_CLOUD_URL", value_name = "URL")]
    url: Option<String>,
    /// The organisation (slug) commands act in; you can also pick it when you approve.
    #[arg(long, value_name = "SLUG")]
    org: Option<String>,
    /// Print the link instead of opening a browser.
    #[arg(long)]
    no_browser: bool,
}

#[derive(Debug, Subcommand)]
pub enum CloudCmd {
    /// Who you are signed in as, your organisations, and what your plan allows.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Choose the organisation commands act in (kept with your login).
    Use { organisation: String },
    /// Hosted environments: containers Onus Cloud builds from a pushed commit and runs
    /// commands in, recording each run as evidence.
    Env {
        #[command(subcommand)]
        cmd: CloudEnvCmd,
    },
    /// Pull requests Onus Cloud governs, newest first, with their lanes.
    Pulls {
        /// Only this repository (owner/name).
        #[arg(long, value_name = "OWNER/NAME")]
        repo: Option<String>,
        /// open, closed or merged.
        #[arg(long)]
        state: Option<String>,
        /// auto-merge, judge, human or blocked.
        #[arg(long)]
        lane: Option<String>,
        /// Only those waiting for a person.
        #[arg(long)]
        attention: bool,
        #[arg(long, default_value_t = 30)]
        limit: u64,
        #[arg(long)]
        json: bool,
    },
    /// One pull request: its lane and why, its analyses and its evidence.
    Pull {
        /// owner/name#12, or #12 for this folder's repository.
        pull: String,
        #[arg(long)]
        json: bool,
    },
    /// Attach a run recorded in a hosted environment to a pull request, as evidence the
    /// judge checks against the recording.
    Evidence {
        /// owner/name#12, or #12 for this folder's repository.
        pull: String,
        /// The run's id (from `onus cloud env run`).
        #[arg(long, value_name = "ID")]
        run: String,
    },
    /// Scoped tokens: one organisation, some repositories, some scopes, a fixed end. Give
    /// one to an agent or a script as ONUS_CLOUD_TOKEN; it opens only those doors.
    Token {
        #[command(subcommand)]
        cmd: TokenCloudCmd,
    },
    /// Ask for more than a task's token allows, in Onus Cloud: it checks the request on the
    /// default branch, then a person (or the policy) decides. Exit 0 when granted (the
    /// token on stdout, minted here with --key), 1 when denied, 3 while waiting.
    Escalate {
        #[command(flatten)]
        args: crate::doors::EscalateArgs,
        #[command(flatten)]
        door: DoorArgs,
        /// The pull request it is about.
        #[arg(long, value_name = "NUMBER")]
        pull: Option<u64>,
        /// Wait this many seconds for a decision.
        #[arg(long, default_value_t = 0, value_name = "SECONDS")]
        wait: u64,
        #[arg(long)]
        json: bool,
    },
    /// Where an escalation stands (`onus cloud escalation <id>`), the organisation's
    /// escalations, or withdraw one. Exit 0 when granted or answered, 1 when denied,
    /// withdrawn, expired or failed, 3 while it is checked or waiting.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Escalation {
        #[command(subcommand)]
        cmd: Option<CloudEscalationCmd>,
        /// The request's id (from onus cloud escalate, onus_escalate or onus_blocked).
        id: Option<String>,
        #[command(flatten)]
        show: ShowArgs,
    },
    /// Report that a task is blocked, with evidence: a test it did not write fails, the
    /// spec contradicts itself or the code, or the task cannot be done as asked. A person
    /// answers. Exit 0 when answered, 1 when denied, 3 while waiting.
    Blocked {
        #[arg(long, value_enum)]
        kind: BlockedKind,
        /// Evidence, strongest first: failing-test:<file>, trace:<file>, map-path:<path>,
        /// draft-diff:<file>, rationale:<text> (repeatable).
        #[arg(long = "evidence", value_name = "KIND:REF")]
        evidence: Vec<String>,
        /// What blocks it.
        #[arg(long)]
        reason: String,
        /// The task (default: the token's).
        #[arg(long)]
        task: Option<String>,
        /// The worktree whose map gives the blast radius, labels and owners.
        #[arg(long, default_value = ".", value_name = "DIR")]
        worktree: PathBuf,
        #[command(flatten)]
        door: DoorArgs,
        /// Wait this many seconds for an answer.
        #[arg(long, default_value_t = 0, value_name = "SECONDS")]
        wait: u64,
        #[arg(long)]
        json: bool,
    },
    /// Ask the map Onus Cloud keeps: the same questions and answers as `onus query`,
    /// answered here from the map the binary built in Onus Cloud.
    Map {
        #[command(subcommand)]
        question: Option<crate::QueryCmd>,
        /// The repository (default: this folder's GitHub remote, else $ONUS_CLOUD_REPO).
        #[arg(long, global = true, value_name = "OWNER/NAME")]
        repo: Option<String>,
        /// A commit Onus Cloud mapped (default: the default branch's newest map).
        #[arg(long, global = true, value_name = "SHA")]
        commit: Option<String>,
        /// A pull request's head, as Onus Cloud analysed it.
        #[arg(long, global = true, value_name = "NUMBER", conflicts_with = "commit")]
        pull: Option<u64>,
        /// Write the map to this file (for `onus query --map`).
        #[arg(long, global = true, value_name = "FILE")]
        download: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum TokenCloudCmd {
    /// Mint a scoped token: the token alone on stdout, what it opens on stderr.
    Create {
        /// A repository it is for (repeatable; default: this folder's GitHub remote).
        #[arg(long = "repo", value_name = "OWNER/NAME")]
        repos: Vec<String>,
        /// Every repository of the organisation (or of the token it is minted from).
        #[arg(long, conflicts_with = "repos")]
        all_repos: bool,
        /// What it opens (repeatable): env:read, env:run, evidence:write, pulls:read,
        /// map:read, escalation:write.
        #[arg(long = "scope", value_name = "SCOPE", required = true)]
        scopes: Vec<String>,
        /// How long it lasts: 90s, 30m, 2h, 1d (at most 24h).
        #[arg(long, default_value = "1h", value_name = "DURATION")]
        ttl: String,
        /// A name for it, such as the task.
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Your logins and scoped tokens: scopes, repositories, end, what each was minted from.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Revoke a token and every token minted from it.
    Revoke { id: String },
}

#[derive(Debug, Subcommand)]
pub enum CloudEscalationCmd {
    /// Where an escalation stands; mirrors a decision into the audit log, and mints a
    /// grant with --key.
    Show {
        id: String,
        #[command(flatten)]
        show: ShowArgs,
    },
    /// The organisation's escalations, newest first.
    List {
        /// Only those waiting for a person.
        #[arg(long)]
        waiting: bool,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw an escalation you filed.
    Withdraw { id: String },
}

/// What `onus cloud escalation <id>` takes.
#[derive(Debug, Args)]
pub struct ShowArgs {
    /// The worktree whose escalations folder keeps the request.
    #[arg(long, default_value = ".", value_name = "DIR")]
    worktree: PathBuf,
    #[command(flatten)]
    door: DoorArgs,
    /// Wait this many seconds for a decision.
    #[arg(long, default_value_t = 0, value_name = "SECONDS")]
    wait: u64,
    #[arg(long)]
    json: bool,
}

/// The task token and the root keys of the door that files and collects requests.
#[derive(Debug, Args)]
pub struct DoorArgs {
    /// The task's token (default: $ONUS_TOKEN). Only what it holds is sent: task,
    /// rights, end.
    #[arg(long, env = "ONUS_TOKEN", value_name = "TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// The root public key that verifies --token.
    #[arg(long, value_name = "FILE")]
    key_public: Option<PathBuf>,
    /// The root private key: a grant is minted here, from the request kept here.
    #[arg(long, value_name = "FILE")]
    key: Option<PathBuf>,
    /// The repository in Onus Cloud (default: the worktree's GitHub remote, else
    /// $ONUS_CLOUD_REPO).
    #[arg(long, env = "ONUS_CLOUD_REPO", value_name = "OWNER/NAME")]
    cloud_repo: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BlockedKind {
    BrokenTest,
    ContradictorySpec,
    ImpossibleTask,
}

impl From<BlockedKind> for onus_doors::escalation::EscalationKind {
    fn from(k: BlockedKind) -> Self {
        use onus_doors::escalation::EscalationKind as K;
        match k {
            BlockedKind::BrokenTest => K::BrokenTest,
            BlockedKind::ContradictorySpec => K::ContradictorySpec,
            BlockedKind::ImpossibleTask => K::ImpossibleTask,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum CloudEnvCmd {
    /// Create an environment from a pushed commit: the repository's declared image, setup and
    /// seed, built by Onus Cloud. Waits until it is ready.
    Create {
        /// The repository (default: this folder's GitHub remote).
        #[arg(long, value_name = "OWNER/NAME")]
        repo: Option<String>,
        /// A branch, tag, commit or pull/<n> (default: this folder's HEAD, which must be pushed).
        #[arg(long = "ref", value_name = "REF")]
        reference: Option<String>,
        /// Its name (default: the repository's and a few letters).
        #[arg(long)]
        name: Option<String>,
        /// A host it may reach through its egress proxy (repeatable); none: no network.
        #[arg(long = "host", value_name = "HOST")]
        hosts: Vec<String>,
        /// Return once it is queued.
        #[arg(long)]
        no_wait: bool,
        #[arg(long)]
        json: bool,
    },
    /// Run a command in an environment, recorded as evidence. Exits with 0 when it passed,
    /// 1 when it failed.
    Run {
        name: String,
        /// Stop it after this many seconds (default 1800).
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
        /// Return once it is queued.
        #[arg(long)]
        no_wait: bool,
        #[arg(long)]
        json: bool,
        /// The command, run with `sh -c` in the repository root.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Environments: the running ones, or with --all the recently ended too.
    List {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// An environment and its runs.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// A run's whole output.
    Log { name: String, run: String },
    /// Destroy an environment.
    Destroy { name: String },
}

fn print_json(v: &Value) -> Result<i32> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(0)
}

fn short(sha: &Value) -> String {
    sha.as_str()
        .map(|s| s[..12.min(s.len())].to_string())
        .unwrap_or_else(|| "-".into())
}

fn env_line(e: &Value) -> String {
    let error = e["error"]
        .as_str()
        .map(|m| format!("  {m}"))
        .unwrap_or_default();
    format!(
        "{}\t{}\t{}@{}\t{}{error}",
        e["name"].as_str().unwrap_or_default(),
        e["status"].as_str().unwrap_or_default(),
        e["repository"].as_str().unwrap_or_default(),
        e["commit"]
            .as_str()
            .map(|_| short(&e["commit"]))
            .unwrap_or_else(|| e["ref"].as_str().unwrap_or_default().to_string()),
        e["image"].as_str().unwrap_or("-"),
    )
}

fn run_summary(run: &Value) -> String {
    let tests = &run["tests"];
    let counts = if tests.is_object() {
        format!(
            ", {} tests, {} failed",
            tests["tests"],
            tests["failures"].as_u64().unwrap_or(0) + tests["errors"].as_u64().unwrap_or(0)
        )
    } else {
        String::new()
    };
    match run["state"].as_str().unwrap_or_default() {
        "error" => format!(
            "could not run: {}",
            run["error"].as_str().unwrap_or("unknown error")
        ),
        state => format!(
            "{state} (exit {}){counts}, manifest {}",
            run["exitCode"],
            short(&run["manifest"])
        ),
    }
}

pub fn login(args: LoginArgs) -> Result<i32> {
    let url = args.url.unwrap_or_else(|| cloud::DEFAULT_URL.into());
    let (creds, path) = cloud::login(
        &url,
        args.org.as_deref(),
        !args.no_browser,
        &mut std::io::stderr(),
    )?;
    let who = creds.user["name"]
        .as_str()
        .or(creds.user["email"].as_str())
        .unwrap_or("you");
    eprintln!("onus: signed in to {} as {who}", creds.url);
    match &creds.organisation {
        Some(o) => eprintln!("onus: commands act in {o} (change it with `onus cloud use <slug>`)"),
        None => {
            eprintln!("onus: pick an organisation with `onus cloud use <slug>` if you have several")
        }
    }
    eprintln!(
        "onus: the token is in {} (readable by you only)",
        path.display()
    );
    Ok(0)
}

pub fn logout() -> Result<i32> {
    if cloud::logout()? {
        eprintln!("onus: signed out of Onus Cloud");
    } else {
        eprintln!("onus: not signed in");
    }
    Ok(0)
}

pub fn run(org: Option<String>, cmd: CloudCmd) -> Result<i32> {
    let mut client = Client::require()?;
    if org.is_some() {
        client.org = org;
    }
    let here = PathBuf::from(".");
    match cmd {
        CloudCmd::Status { json } => {
            let s = cloud::act(&client, &here, "cloud.status", json!({}))?;
            if json {
                return print_json(&s);
            }
            let who = &s["who"];
            let name = who["name"]
                .as_str()
                .or(who["email"].as_str())
                .unwrap_or_default();
            match client.via {
                "api-key" => println!(
                    "Onus Cloud at {}, with the API key {name}",
                    s["url"].as_str().unwrap_or_default()
                ),
                "token" => println!(
                    "Onus Cloud at {}, {}",
                    s["url"].as_str().unwrap_or_default(),
                    token_line(&s["token"], s["organisation"].as_str())
                ),
                _ => println!(
                    "Signed in to {} as {name}{}",
                    s["url"].as_str().unwrap_or_default(),
                    who["email"]
                        .as_str()
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                ),
            }
            match s["organisation"].as_str() {
                // A token without env:read has no allowance to show.
                Some(_) if s["environments"].is_null() => {}
                Some(o) => {
                    let a = &s["environments"];
                    println!(
                        "Organisation: {o}, {} plan",
                        a["plan"].as_str().unwrap_or("?")
                    );
                    let limit = a["environments"]["limit"]
                        .as_u64()
                        .map_or("unlimited".to_string(), |l| l.to_string());
                    let minutes = match (
                        a["minutes"]["included"].as_u64(),
                        a["minutes"]["left"].as_u64(),
                    ) {
                        (_, None) => format!("{} used this period", a["minutes"]["used"]),
                        (Some(i), Some(l)) => format!("{l} of {i} left this period"),
                        (None, Some(l)) => format!("{l} left"),
                    };
                    println!(
                        "Environments: {} running of {limit}; minutes: {minutes}",
                        a["environments"]["active"]
                    );
                }
                None => println!("No organisation chosen: `onus cloud use <slug>`"),
            }
            let others: Vec<&str> = s["organisations"]
                .as_array()
                .map(|a| a.iter().filter_map(|o| o["slug"].as_str()).collect())
                .unwrap_or_default();
            if others.len() > 1 {
                println!("Your organisations: {}", others.join(", "));
            }
            Ok(0)
        }
        CloudCmd::Use { organisation } => {
            if client.via == "token" {
                bail!(
                    "a scoped token acts in one organisation ({})",
                    client.organisation().unwrap_or_else(|_| "its own".into())
                );
            }
            let Some(mut creds) = cloud::load_credentials()? else {
                bail!(
                    "`onus cloud use` keeps the choice with your login: run `onus login` first (in CI, set ONUS_CLOUD_ORG)"
                );
            };
            let me = client.whoami()?;
            let known = me["organisations"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|o| o["slug"] == organisation.as_str() || o["id"] == organisation.as_str())
            });
            if !known {
                bail!("you are not a member of {organisation}");
            }
            creds.organisation = Some(organisation.clone());
            cloud::save_credentials(&creds)?;
            eprintln!("onus: commands act in {organisation}");
            Ok(0)
        }
        CloudCmd::Env { cmd } => env(&client, &here, cmd),
        CloudCmd::Pulls {
            repo,
            state,
            lane,
            attention,
            limit,
            json,
        } => {
            let out = cloud::act(
                &client,
                &here,
                "cloud.pulls",
                json!({ "repository": repo, "state": state, "lane": lane, "attention": attention, "limit": limit }),
            )?;
            if json {
                return print_json(&out);
            }
            for p in out["pulls"].as_array().cloned().unwrap_or_default() {
                println!(
                    "{}#{}\t{}\t{}\t{}",
                    p["repositoryFullName"]
                        .as_str()
                        .or(p["fullName"].as_str())
                        .unwrap_or_default(),
                    p["number"],
                    p["lane"].as_str().unwrap_or("-"),
                    p["state"].as_str().unwrap_or_default(),
                    p["title"].as_str().unwrap_or_default()
                );
            }
            Ok(0)
        }
        CloudCmd::Pull { pull, json } => {
            let out = cloud::act(&client, &here, "cloud.pull", json!({ "pull": pull }))?;
            if json {
                return print_json(&out);
            }
            let p = &out["pull"];
            println!(
                "{}#{} {}\n{} · lane {} · {}",
                out["repository"]["fullName"].as_str().unwrap_or_default(),
                p["number"],
                p["title"].as_str().unwrap_or_default(),
                p["state"].as_str().unwrap_or_default(),
                p["lane"].as_str().unwrap_or("-"),
                p["author"].as_str().unwrap_or_default()
            );
            if let Some(latest) = out["analyses"].as_array().and_then(|a| a.first()) {
                println!(
                    "latest analysis: {} at {}{}",
                    latest["status"].as_str().unwrap_or_default(),
                    short(&latest["headSha"]),
                    latest["verdict"]
                        .as_str()
                        .map(|v| format!(", judge: {v}"))
                        .unwrap_or_default()
                );
            }
            for e in out["evidence"].as_array().cloned().unwrap_or_default() {
                let recorded = if e["environmentRunId"].is_string() {
                    " (recorded by Onus Cloud)"
                } else {
                    ""
                };
                println!(
                    "evidence: `{}` exit {} at {}{recorded}",
                    e["command"].as_str().unwrap_or_default(),
                    e["exitCode"],
                    short(&e["headSha"])
                );
            }
            Ok(0)
        }
        CloudCmd::Evidence { pull, run } => {
            let out = cloud::act(
                &client,
                &here,
                "cloud.attachEvidence",
                attach_input(&pull, &run),
            )?;
            eprintln!(
                "onus: attached run {run} to {pull}{}",
                if out["reanalysing"] == true {
                    "; analysing it again"
                } else {
                    ""
                }
            );
            Ok(0)
        }
        CloudCmd::Token { cmd } => token(&client, &here, cmd),
        CloudCmd::Escalate {
            args,
            door,
            pull,
            wait,
            json,
        } => {
            let root = onus_cli::daemon::worktree_root(&args.repo);
            let state = local_state(&root);
            let cloud = plain(client, &door, &root);
            let edoor = escalation_door(&state, &door, &cloud, pull)?;
            let task = match task_of(&edoor)? {
                Some(t) if t != args.task => {
                    bail!("--token is for task `{t}`, not `{}`", args.task)
                }
                _ => args.task.clone(),
            };
            let req = crate::doors::new_request(
                &root,
                &task,
                args.kind.into(),
                &args.scopes,
                &args.evidence,
                &args.reason,
            )?;
            if let Some(out) = &args.out {
                std::fs::write(out, format!("{}\n", serde_json::to_string_pretty(&req)?))?;
            }
            let id = req.id.clone();
            let filed = cloud_doors::route(&edoor, req)?;
            let out = if wait > 0 {
                wait_for(&edoor, &id, wait)?
            } else {
                filed
            };
            report(&out, json)
        }
        CloudCmd::Escalation { cmd, id, show } => match (cmd, id) {
            (Some(CloudEscalationCmd::Show { id, show }), _) => escalation_show(client, &id, &show),
            (None, Some(id)) => escalation_show(client, &id, &show),
            (Some(CloudEscalationCmd::List { waiting, json }), _) => {
                let out = cloud::escalations(&client, waiting)?;
                if json {
                    return print_json(&out);
                }
                for e in out["escalations"].as_array().cloned().unwrap_or_default() {
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        e["clientRequestId"]
                            .as_str()
                            .or(e["requestId"].as_str())
                            .or(e["id"].as_str())
                            .unwrap_or_default(),
                        e["state"].as_str().unwrap_or_default(),
                        e["kind"].as_str().unwrap_or_default(),
                        e["repository"].as_str().unwrap_or_default(),
                        e["task"].as_str().unwrap_or("-"),
                        e["owners"]
                            .as_array()
                            .map(|o| o
                                .iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join(" "))
                            .unwrap_or_default()
                    );
                }
                Ok(0)
            }
            (Some(CloudEscalationCmd::Withdraw { id }), _) => {
                // A request filed from here is known by its local id.
                let local = onus_cli::daemon::worktree_root(Path::new("."))
                    .join(".onus/escalations")
                    .join(format!("{id}.cloud.json"));
                let remote = std::fs::read_to_string(&local)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .and_then(|v| v["escalationId"].as_str().map(str::to_string))
                    .unwrap_or_else(|| id.clone());
                let out = cloud::withdraw_escalation(&client, &remote)?;
                eprintln!(
                    "onus: escalation {id} is {}",
                    out["escalation"]["state"].as_str().unwrap_or("withdrawn")
                );
                Ok(0)
            }
            (None, None) => bail!("name an escalation: onus cloud escalation <id>, or list"),
        },
        CloudCmd::Blocked {
            kind,
            evidence,
            reason,
            task,
            worktree,
            door,
            wait,
            json,
        } => {
            let root = onus_cli::daemon::worktree_root(&worktree);
            let state = local_state(&root);
            let cloud = plain(client, &door, &root);
            let edoor = escalation_door(&state, &door, &cloud, None)?;
            let task = match (task_of(&edoor)?, task) {
                (Some(t), _) => t,
                (None, Some(t)) => t,
                (None, None) => bail!("name the task (--task), or pass its --token"),
            };
            let req =
                crate::doors::new_request(&root, &task, kind.into(), &[], &evidence, &reason)?;
            let id = req.id.clone();
            let mut out = cloud_doors::route(&edoor, req)?;
            out["next"] = json!(cloud_doors::BLOCKED_NEXT);
            if wait > 0 {
                out = wait_for(&edoor, &id, wait)?;
            }
            report(&out, json)
        }
        CloudCmd::Map {
            question,
            repo,
            commit,
            pull,
            download,
        } => {
            let repository = cloud::repository_for(&here, repo.as_deref()).context(
                "this folder has no GitHub remote: pass --repo owner/name (or set ONUS_CLOUD_REPO)",
            )?;
            if let Some(file) = &download {
                let at = cloud::map_commit(&client, &here, &repository, commit.as_deref(), pull)?;
                let map = cloud::hosted_map(&client, &repository, at.as_deref())?;
                std::fs::write(file, serde_json::to_vec(&map)?)
                    .with_context(|| format!("cannot write {}", file.display()))?;
                eprintln!(
                    "onus: the map of {repository} at {} is in {} (ask it with onus query --map {})",
                    map.commit,
                    file.display(),
                    file.display()
                );
                if question.is_none() {
                    return Ok(0);
                }
            }
            let question = question
                .context("ask a question (status, dependents, impact, …), or pass --download FILE")?
                .query()?;
            let mut out = cloud::act(
                &client,
                &here,
                "cloud.map",
                json!({ "repository": repository, "commit": commit, "pull": pull, "question": question }),
            )?;
            if let Some(o) = out.as_object_mut() {
                o.remove("componentDirs");
            }
            print_json(&out)
        }
    }
}

/// `owner/name` of a scoped token's repositories, or every repository.
fn token_line(t: &Value, org: Option<&str>) -> String {
    let list = |v: &Value| -> String {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let repos = match list(&t["repositories"]) {
        r if r.is_empty() => "every repository".to_string(),
        r => r,
    };
    let ends = t["expiresAt"]
        .as_str()
        .and_then(cloud::parse_utc)
        .map(|e| {
            let now = onus_doors::gateway::now();
            if e <= now {
                "ended".to_string()
            } else {
                format!("ends in {}", duration(e - now))
            }
        })
        .unwrap_or_default();
    format!(
        "with a scoped token{} in {}: {} on {repos}; {ends}",
        t["label"]
            .as_str()
            .map(|l| format!(" ({l})"))
            .unwrap_or_default(),
        t["organisation"]["slug"].as_str().or(org).unwrap_or("?"),
        list(&t["scopes"]),
    )
}

/// `1h52m`, `30m`, `45s`.
fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

fn token(client: &Client, here: &Path, cmd: TokenCloudCmd) -> Result<i32> {
    match cmd {
        TokenCloudCmd::Create {
            repos,
            all_repos,
            scopes,
            ttl,
            label,
            json,
        } => {
            let ttl_seconds = cloud::parse_ttl(&ttl)?;
            let repositories = if all_repos {
                None
            } else if repos.is_empty() {
                Some(vec![cloud::repository_of(here).context(
                    "this folder has no GitHub remote: pass --repo owner/name, or --all-repos",
                )?])
            } else {
                Some(repos)
            };
            let out = cloud::mint_token(
                client,
                &cloud::Narrow {
                    repositories,
                    scopes,
                    ttl_seconds,
                    label,
                },
            )?;
            if json {
                return print_json(&out);
            }
            println!(
                "{}",
                out["token"].as_str().context("no token in the answer")?
            );
            let list = |v: &Value| -> String {
                v.as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            };
            let repos = match list(&out["repositories"]) {
                r if r.is_empty() => "every repository".to_string(),
                r => r,
            };
            eprintln!(
                "onus: token for {} ({repos}): {}; ends {} ({}). Revoke: onus cloud token revoke {}",
                out["organisation"]["slug"].as_str().unwrap_or("?"),
                list(&out["scopes"]),
                out["expiresAt"].as_str().unwrap_or("?"),
                duration(ttl_seconds),
                out["tokenId"].as_str().unwrap_or("?"),
            );
            eprintln!("onus: give it to the agent as ONUS_CLOUD_TOKEN; it cannot be widened");
            Ok(0)
        }
        TokenCloudCmd::List { json } => {
            let out = client.get("/v1/me/cli-tokens")?;
            if json {
                return print_json(&out);
            }
            for t in out["tokens"].as_array().cloned().unwrap_or_default() {
                let scoped = t["kind"] == "scoped";
                println!(
                    "{}\t{}\t{}\t{}\t{}\tends {}{}{}",
                    t["id"].as_str().unwrap_or_default(),
                    if scoped { "scoped" } else { "login" },
                    t["organisation"]["slug"].as_str().unwrap_or("-"),
                    if scoped {
                        t["scopes"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .unwrap_or_default()
                    } else {
                        "-".into()
                    },
                    match t["repositories"].as_array() {
                        Some(a) => a
                            .iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(","),
                        None if scoped => "all".into(),
                        None => "-".into(),
                    },
                    t["expiresAt"].as_str().unwrap_or("?"),
                    t["parentId"]
                        .as_str()
                        .map(|p| format!("\tfrom {p}"))
                        .unwrap_or_default(),
                    t["label"]
                        .as_str()
                        .or(t["client"].as_str())
                        .map(|l| format!("\t{l}"))
                        .unwrap_or_default(),
                );
            }
            Ok(0)
        }
        TokenCloudCmd::Revoke { id } => {
            let out = client.delete(&format!("/v1/me/cli-tokens/{}", segment(&id)))?;
            match out["revoked"].as_u64() {
                Some(n) if n > 1 => eprintln!(
                    "onus: revoked {id} and {} token{} minted from it",
                    n - 1,
                    if n == 2 { "" } else { "s" }
                ),
                _ => eprintln!("onus: revoked {id}"),
            }
            Ok(0)
        }
    }
}

/// The records of the worktree `root` (`.onus/`), as `onus mcp` and `onus ui` keep them.
fn local_state(root: &Path) -> crate::ui::State {
    let root = onus_cli::daemon::canonical(root).unwrap_or_else(|_| root.to_path_buf());
    crate::ui::State::new(root, String::new(), 0, crate::ui::Places::default())
}

/// The client as it is, for the worktree's repository: the CLI acts for its own person.
fn plain(client: Client, door: &DoorArgs, root: &Path) -> CloudDoor {
    let repository = cloud::repository_for(root, door.cloud_repo.as_deref());
    CloudDoor::plain(client, repository)
}

fn escalation_door<'a>(
    state: &'a crate::ui::State,
    door: &'a DoorArgs,
    cloud: &'a CloudDoor,
    pull: Option<u64>,
) -> Result<EscalationDoor<'a>> {
    let public = match &door.key_public {
        Some(k) => Some(onus_doors::token::public_key(&crate::doors::read_key(k)?)?),
        None => None,
    };
    Ok(EscalationDoor {
        state,
        token: door.token.clone().filter(|t| !t.trim().is_empty()),
        public,
        key: door.key.as_deref(),
        cloud: Some(cloud),
        repository: cloud.repository(),
        pull,
    })
}

/// The task of the door's token, verified.
fn task_of(door: &EscalationDoor) -> Result<Option<String>> {
    match (&door.token, door.public) {
        (Some(t), Some(k)) => Ok(Some(onus_doors::token::verify(t, &k)?.task)),
        (Some(_), None) => bail!("--token needs the root public key (--key-public)"),
        _ => Ok(None),
    }
}

/// Follows a request until it is decided or `wait` seconds have passed.
fn wait_for(door: &EscalationDoor, id: &str, wait: u64) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        let started = Instant::now();
        let left = deadline.saturating_duration_since(started).as_secs();
        let out = cloud_doors::follow(door, id, left.min(50))?;
        if cloud_doors::is_final(out["state"].as_str().unwrap_or_default()) || left == 0 {
            return Ok(out);
        }
        // Not filed in Onus Cloud, or it answered at once: do not spin.
        if started.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(
                Duration::from_secs(2).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}

fn escalation_show(client: Client, id: &str, show: &ShowArgs) -> Result<i32> {
    let root = onus_cli::daemon::worktree_root(&show.worktree);
    let state = local_state(&root);
    let cloud = plain(client, &show.door, &root);
    let edoor = escalation_door(&state, &show.door, &cloud, None)?;
    let out = if show.wait > 0 {
        wait_for(&edoor, id, show.wait)?
    } else {
        cloud_doors::follow(&edoor, id, 0)?
    };
    report(&out, show.json)
}

/// Prints where a request stands, the granted token alone on stdout; the exit code
/// says what became of it.
fn report(out: &Value, json: bool) -> Result<i32> {
    let state = out["state"].as_str().unwrap_or_default();
    if json {
        print_json(out)?;
    } else {
        let kind = out["kind"]
            .as_str()
            .or(out["request"]["kind"].as_str())
            .unwrap_or("escalation");
        let said = out["answer"]
            .as_str()
            .or(out["reason"].as_str())
            .or(out["next"].as_str())
            .unwrap_or_default();
        eprintln!(
            "onus: escalation {} ({kind}): {state}{}",
            out["id"].as_str().unwrap_or_default(),
            if said.is_empty() {
                String::new()
            } else {
                format!(" — {said}")
            }
        );
        if let Some(url) = out["url"].as_str() {
            eprintln!("onus: {url}");
        }
        if let Some(t) = out["token"].as_str() {
            println!("{t}");
        }
    }
    Ok(match state {
        "granted" | "answered" => 0,
        "denied" | "withdrawn" | "expired" | "failed" => 1,
        _ => 3,
    })
}

/// The pull request as the agent action takes it: repository and number.
fn attach_input(pull: &str, run: &str) -> Value {
    match pull.rsplit_once('#') {
        Some((r, n)) if !r.is_empty() => {
            json!({ "repository": r, "number": n.parse::<u64>().ok(), "run": run })
        }
        Some((_, n)) => json!({ "number": n.parse::<u64>().ok(), "run": run }),
        None => json!({ "number": pull.parse::<u64>().ok(), "run": run }),
    }
}

fn env(client: &Client, here: &Path, cmd: CloudEnvCmd) -> Result<i32> {
    match cmd {
        CloudEnvCmd::Create {
            repo,
            reference,
            name,
            hosts,
            no_wait,
            json,
        } => {
            let repository = match repo {
                Some(r) => r,
                None => cloud::repository_of(here)
                    .context("this folder has no GitHub remote: pass --repo owner/name")?,
            };
            let reference = match reference {
                Some(r) => r,
                None => {
                    let head = cloud::commit_of(here, "HEAD")
                        .context("this folder has no commit: pass --ref")?;
                    if !cloud::is_pushed(here, &head) {
                        bail!(
                            "HEAD ({}) is not pushed, and Onus Cloud builds from GitHub: push it, or pass --ref",
                            &head[..12]
                        );
                    }
                    head
                }
            };
            let mut body = json!({ "repository": repository, "ref": reference, "hosts": hosts });
            if let Some(n) = name {
                body["name"] = json!(n);
            }
            let wait = (!no_wait).then(|| Duration::from_secs(30 * 60));
            let env = client.create_environment(&body, wait, &mut |e| {
                if !json {
                    eprintln!(
                        "onus: environment {} is {}",
                        e["name"].as_str().unwrap_or_default(),
                        e["status"].as_str().unwrap_or_default()
                    );
                }
            })?;
            if json {
                print_json(&env)?;
            } else {
                println!("{}", env_line(&env));
            }
            if matches!(env["status"].as_str(), Some("failed")) {
                if !json {
                    eprintln!(
                        "onus: {}",
                        env["error"].as_str().unwrap_or("it could not be built")
                    );
                }
                return Ok(1);
            }
            if !json && env["status"] == "ready" {
                eprintln!(
                    "onus: run commands with `onus cloud env run {} -- <command>`",
                    env["name"].as_str().unwrap_or_default()
                );
            }
            Ok(0)
        }
        CloudEnvCmd::Run {
            name,
            timeout,
            no_wait,
            json,
            command,
        } => {
            let run = client.run_in_environment(&name, &command.join(" "), timeout, !no_wait)?;
            if json {
                print_json(&run)?;
            } else {
                if let Some(tail) = run["outputTail"].as_str().filter(|t| !t.is_empty()) {
                    println!("{tail}");
                }
                eprintln!(
                    "onus: run {}: {}",
                    run["id"].as_str().unwrap_or_default(),
                    run_summary(&run)
                );
                if matches!(run["state"].as_str(), Some("passed" | "failed")) {
                    eprintln!(
                        "onus: attach it to a pull request with `onus cloud evidence <owner/name#n> --run {}`",
                        run["id"].as_str().unwrap_or_default()
                    );
                }
            }
            match run["state"].as_str() {
                Some("passed" | "queued" | "running") => Ok(0),
                Some("failed") => Ok(1),
                _ => bail!(
                    "{}",
                    run["error"].as_str().unwrap_or("the run did not finish")
                ),
            }
        }
        CloudEnvCmd::List { all, json } => {
            let out = cloud::act(client, here, "cloud.envs", json!({ "all": all }))?;
            if json {
                return print_json(&out);
            }
            for e in out["environments"].as_array().cloned().unwrap_or_default() {
                println!("{}", env_line(&e));
            }
            Ok(0)
        }
        CloudEnvCmd::Show { name, json } => {
            let out =
                client.get(&client.org_path(&format!("/environments/{}", segment(&name)))?)?;
            if json {
                return print_json(&out);
            }
            println!("{}", env_line(&out["environment"]));
            for r in out["runs"].as_array().cloned().unwrap_or_default() {
                println!(
                    "{}\t{}\t{}",
                    r["id"].as_str().unwrap_or_default(),
                    run_summary(&r),
                    r["command"].as_str().unwrap_or_default()
                );
            }
            Ok(0)
        }
        CloudEnvCmd::Log { name, run } => {
            let log = client.get(&client.org_path(&format!(
                "/environments/{}/runs/{}/log",
                segment(&name),
                segment(&run)
            ))?)?;
            match log {
                Value::String(text) => print!("{text}"),
                other => println!("{}", serde_json::to_string_pretty(&other)?),
            }
            Ok(0)
        }
        CloudEnvCmd::Destroy { name } => {
            let out = cloud::act(client, here, "cloud.envDestroy", json!({ "name": name }))?;
            eprintln!(
                "onus: environment {name} is {}",
                out["environment"]["status"]
                    .as_str()
                    .unwrap_or("being destroyed")
            );
            Ok(0)
        }
    }
}
