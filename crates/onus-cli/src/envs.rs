//! `onus env` and `onus evidence`: environments that run a change and the
//! store of what they recorded (ADR 0010).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use onus_doors::runner::{TestRun, tail};
use onus_doors::scope::Kind;
use onus_env::env::{self, Grants};
use onus_env::store::{ArtifactKind, Store};

#[derive(Debug, Subcommand)]
pub enum EnvCmd {
    /// Create an environment from a commit: the warm image (setup run once
    /// per lockfile), the commit's files, then the seed command.
    Create {
        /// The repository; its working tree's onus.yaml declares the environment.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// The commit to build from.
        #[arg(long = "ref", default_value = "HEAD", value_name = "REF")]
        reference: String,
        /// The environment's name (default: the token's task, else the short commit).
        #[arg(long)]
        name: Option<String>,
        /// Use this onus.yaml instead of the working tree's.
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// The task's token: its `net:host:` rights open the egress proxy to
        /// those hosts, its `secret:` rights pass `ONUS_SECRET_<NAME>` in.
        /// Without one, the environment has no network and no secrets.
        #[arg(long, env = "ONUS_TOKEN", value_name = "TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// The root public key (`root.pub`), to verify the token.
        #[arg(long, value_name = "FILE")]
        key_public: Option<PathBuf>,
    },
    /// Run a command in an environment and record the run in the evidence
    /// store. Prints a test run for `onus submit --evidence`; exit 1 when the
    /// command failed.
    Run {
        name: String,
        /// The repository whose evidence store records the run.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// Use this evidence store instead of the repository's.
        #[arg(long, value_name = "DIR")]
        store: Option<PathBuf>,
        /// The command, run with `sh -c` in the repository root.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// List environments.
    List,
    /// Remove an environment, its egress proxy and its network.
    Destroy { name: String },
}

#[derive(Debug, Subcommand)]
pub enum EvidenceCmd {
    /// List recorded runs, oldest first.
    List {
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        #[arg(long, value_name = "DIR")]
        store: Option<PathBuf>,
    },
    /// Print a run's manifest, or one of its artifacts.
    Show {
        /// The run id, or at least its first 8 characters.
        run: String,
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        #[arg(long, value_name = "DIR")]
        store: Option<PathBuf>,
        /// Print this artifact instead: its path (`-` for the output log).
        #[arg(long, value_name = "PATH")]
        artifact: Option<String>,
    },
}

fn store(repo: &Path, store: Option<PathBuf>) -> Result<Store> {
    match store {
        Some(dir) => Ok(Store::open(dir)),
        None => Store::for_repo(repo),
    }
}

/// What a task token lets an environment have: its hosts and secrets.
pub fn grants(token: Option<&str>, key: Option<&Path>) -> Result<Grants> {
    let Some(token) = token else {
        return Ok(Grants::default());
    };
    let key = key.context("--token needs --key-public to verify it")?;
    let public =
        std::fs::read_to_string(key).with_context(|| format!("cannot read {}", key.display()))?;
    let v = onus_doors::token::verify(token, &onus_doors::token::public_key(&public)?)?;
    let now = onus_doors::gateway::now();
    if v.expires != 0 && v.expires <= now {
        bail!("the token has expired");
    }
    let mut g = Grants::default();
    for r in &v.rights {
        // Only what the token, with its attenuations, still allows now.
        if !matches!(r.kind, Kind::Net | Kind::Secret)
            || v.authorize(r.kind, &r.pattern, now).is_err()
        {
            continue;
        }
        match r.kind {
            Kind::Net => g.hosts.push(r.pattern.clone()),
            _ => g.secrets.push(r.pattern.clone()),
        }
    }
    Ok(g)
}

pub fn env_cmd(cmd: EnvCmd) -> Result<i32> {
    match cmd {
        EnvCmd::Create {
            repo,
            reference,
            name,
            config,
            token,
            key_public,
        } => {
            let cfg = match config {
                Some(p) => Some(onus_map::config::load(&p)?),
                None => onus_map::config::load_from_tree(&repo)?,
            };
            let spec = onus_env::spec::resolve(
                &repo,
                cfg.as_ref().and_then(|c| c.config.environment.as_ref()),
            )?;
            let grants = grants(token.as_deref(), key_public.as_deref())?;
            let name = match name {
                Some(n) => n,
                None => match (&token, &key_public) {
                    (Some(t), Some(k)) => {
                        let public = std::fs::read_to_string(k)?;
                        onus_doors::token::verify(t, &onus_doors::token::public_key(&public)?)?
                            .task
                            .to_ascii_lowercase()
                    }
                    _ => short_commit(&repo, &reference)?,
                },
            };
            let e = env::create(&repo, &reference, &name, &spec, &grants)?;
            println!("{}", serde_json::to_string_pretty(&e)?);
            Ok(0)
        }
        EnvCmd::Run {
            name,
            repo,
            store: dir,
            command,
        } => {
            let store = store(&repo, dir)?;
            let command = command.join(" ");
            let (id, m) = env::run(&name, &command, &store)?;
            let run = test_run_of(&store, id, &m)?;
            println!("{}", serde_json::to_string_pretty(&run)?);
            Ok(if run.failed() { 1 } else { 0 })
        }
        EnvCmd::List => {
            for l in env::list()? {
                let net = if l.env.hosts.is_empty() {
                    "no network".to_string()
                } else {
                    l.env.hosts.join(",")
                };
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    l.env.name,
                    &l.env.commit[..l.env.commit.len().min(12)],
                    l.state,
                    l.env.warm_image.as_deref().unwrap_or(&l.env.image),
                    net
                );
            }
            Ok(0)
        }
        EnvCmd::Destroy { name } => {
            if !env::destroy(&name)? {
                bail!("there is no environment {name}");
            }
            eprintln!("onus: environment {name} removed");
            Ok(0)
        }
    }
}

/// A recorded run as a test run for a submission: it names its manifest.
pub fn test_run_of(store: &Store, id: String, m: &onus_env::store::Manifest) -> Result<TestRun> {
    let log = m
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Log)
        .map(|a| store.get(&a.sha256))
        .transpose()?
        .unwrap_or_default();
    Ok(TestRun {
        commit: m.commit.clone(),
        image: m.environment.image.clone(),
        // The judge re-runs the command in a fresh container: with the
        // environment's setup and its seed, so it sees the same data.
        setup: {
            let steps: Vec<&str> = [&m.environment.setup, &m.environment.seed]
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect();
            (!steps.is_empty()).then(|| steps.join(" && "))
        },
        command: m.command.clone(),
        exit_code: m.exit_code,
        output_tail: tail(&String::from_utf8_lossy(&log), 40),
        manifest: Some(id),
    })
}

pub fn short_commit(repo: &Path, reference: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "rev-parse",
            "--short=12",
            &format!("{reference}^{{commit}}"),
        ])
        .output()?;
    if !out.status.success() {
        bail!("`{reference}` is not a commit");
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

pub fn evidence_cmd(cmd: EvidenceCmd) -> Result<i32> {
    match cmd {
        EvidenceCmd::List { repo, store: dir } => {
            for (id, m) in store(&repo, dir)?.list()? {
                let tests = m.tests.map_or(String::new(), |t| {
                    format!("\t{} tests, {} failed", t.tests, t.failures + t.errors)
                });
                println!(
                    "{}\t{}\t{}\texit {}\t{}{tests}",
                    &id[..12],
                    &m.commit[..m.commit.len().min(12)],
                    m.environment.name,
                    m.exit_code,
                    m.command
                );
            }
            Ok(0)
        }
        EvidenceCmd::Show {
            run,
            repo,
            store: dir,
            artifact,
        } => {
            let store = store(&repo, dir)?;
            let (id, m) = store.manifest(&run)?;
            match artifact {
                None => {
                    let out = serde_json::json!({ "id": id, "manifest": m });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                }
                Some(path) => {
                    let a = m
                        .artifacts
                        .iter()
                        .find(|a| a.path == path)
                        .with_context(|| format!("run {} has no artifact {path}", &id[..12]))?;
                    std::io::stdout().write_all(&store.get(&a.sha256)?)?;
                }
            }
            Ok(0)
        }
    }
}
