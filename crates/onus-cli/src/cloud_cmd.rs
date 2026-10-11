//! `onus login`, `onus logout` and `onus cloud`: the hosted service from the terminal.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use onus_cli::cloud::{self, Client, segment};
use serde_json::{Value, json};

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
    }
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
