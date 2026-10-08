//! Phase 3 commands: task tokens, scope suggestions, the git gateway and the
//! audit log (ADR 0008).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use onus_doors::audit::{AuditLog, Record};
use onus_doors::gateway::{Gateway, now};
use onus_doors::plan::Plan;
use onus_doors::scope::Right;
use onus_doors::token::{self, RootKey};

#[derive(Debug, Subcommand)]
pub enum TokenCmd {
    /// Create a root key pair: `root.key` (keep it with the operator) and `root.pub`.
    Keygen {
        /// The folder to write the key files to.
        #[arg(long, default_value = ".", value_name = "DIR")]
        out: PathBuf,
    },
    /// Mint a token for a task plan.
    Mint {
        /// The task plan (YAML): task, writes, reads, hosts, secrets, refs, ttl.
        #[arg(long, value_name = "FILE")]
        plan: PathBuf,
        /// The root private key (`root.key`).
        #[arg(long, value_name = "FILE")]
        key: PathBuf,
        /// Record the mint in this audit log.
        #[arg(long, value_name = "FILE")]
        audit: Option<PathBuf>,
    },
    /// Narrow a token, for a sub-agent: every kind of right named here allows
    /// only what is named. It can never widen the token.
    Attenuate {
        #[command(flatten)]
        token: TokenArgs,
        /// A narrower right, such as write:path:services/notifications/src/sms/** (repeatable).
        #[arg(long = "only", value_name = "RIGHT", required = true)]
        only: Vec<String>,
    },
    /// Show what a token grants, as JSON.
    Inspect {
        #[command(flatten)]
        token: TokenArgs,
    },
    /// Exit 0 when the token allows a right now, 1 with the reason when not.
    Check {
        #[command(flatten)]
        token: TokenArgs,
        /// The right to check, such as write:path:services/billing/a.ts.
        right: String,
    },
}

#[derive(Debug, Args)]
pub struct TokenArgs {
    /// The token (default: $ONUS_TOKEN).
    #[arg(long, env = "ONUS_TOKEN", value_name = "TOKEN", hide_env_values = true)]
    token: String,
    /// The root public key (`root.pub`).
    #[arg(long, value_name = "FILE")]
    key_public: PathBuf,
}

impl TokenArgs {
    fn verify(&self) -> Result<token::Verified> {
        let public = read_key(&self.key_public)?;
        token::verify(&self.token, &token::public_key(&public)?)
    }
}

#[derive(Debug, Subcommand)]
pub enum ScopeCmd {
    /// Suggest reads for a plan's writes from the map: the components written,
    /// what they use and what uses them, leaving out sensitive ones.
    Suggest {
        #[arg(long, value_name = "FILE")]
        plan: PathBuf,
        /// The repository to map.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum GatewayCmd {
    /// Serve task mirrors over git smart HTTP. Agents clone
    /// http://<addr>/<task>.git with their token as the password; pushes that
    /// change paths outside the token's write scope are refused, and the rest
    /// are replayed onto the real repository and pushed to its remote.
    Serve {
        /// The real repository: a clone the gateway owns, with credentials to push.
        #[arg(long, value_name = "DIR")]
        repo: PathBuf,
        /// The remote accepted commits are pushed to.
        #[arg(long, default_value = "origin")]
        remote: String,
        /// The commit tasks start from.
        #[arg(long, default_value = "main")]
        base: String,
        /// The root public key tokens are verified with.
        #[arg(long, value_name = "FILE")]
        key_public: PathBuf,
        /// Where mirrors, task state and the audit log live.
        #[arg(long, value_name = "DIR")]
        state: PathBuf,
        /// The address to listen on.
        #[arg(long, default_value = "127.0.0.1:9419", value_name = "ADDR")]
        listen: std::net::SocketAddr,
    },
    /// Run by a mirror's git hooks; not for people.
    #[command(hide = true)]
    Hook {
        stage: String,
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        task: String,
    },
}

fn read_key(path: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the key {}", path.display()))?
        .trim()
        .to_string())
}

fn rights(list: &[String]) -> Result<Vec<Right>> {
    list.iter()
        .map(|s| s.parse::<Right>().map_err(anyhow::Error::msg))
        .collect()
}

fn token_json(v: &token::Verified) -> serde_json::Value {
    serde_json::json!({
        "task": v.task,
        "expires": v.expires,
        "rights": v.rights.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
        "attenuations": v.attenuations,
    })
}

pub fn token(cmd: TokenCmd) -> Result<i32> {
    match cmd {
        TokenCmd::Keygen { out } => {
            std::fs::create_dir_all(&out)?;
            let key = RootKey::generate();
            let private = out.join("root.key");
            if private.exists() {
                bail!("{} exists; move it away first", private.display());
            }
            write_private(&private, &key.private_hex())?;
            std::fs::write(out.join("root.pub"), format!("{}\n", key.public_hex()))?;
            println!("{}", out.join("root.pub").display());
            Ok(0)
        }
        TokenCmd::Mint { plan, key, audit } => {
            let plan = Plan::parse(
                &std::fs::read_to_string(&plan)
                    .with_context(|| format!("cannot read {}", plan.display()))?,
            )?;
            let root = RootKey::from_hex(&read_key(&key)?)?;
            let grant = plan.grant(now())?;
            let minted = token::mint(&root, &grant)?;
            if let Some(log) = audit {
                AuditLog::new(log).append(
                    Record {
                        actor: "operator".into(),
                        action: "mint".into(),
                        subject: grant.task.clone(),
                        decision: "granted".into(),
                        reason: "task plan".into(),
                        details: serde_json::json!({
                            "rights": grant.rights.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                            "expires": grant.expires,
                        }),
                    },
                    now(),
                )?;
            }
            println!("{minted}");
            Ok(0)
        }
        TokenCmd::Attenuate { token, only } => {
            let v = token.verify()?;
            println!("{}", v.attenuate(&rights(&only)?)?);
            Ok(0)
        }
        TokenCmd::Inspect { token } => {
            let v = token.verify()?;
            println!("{}", serde_json::to_string_pretty(&token_json(&v))?);
            Ok(0)
        }
        TokenCmd::Check { token, right } => {
            let v = token.verify()?;
            let r: Right = right.parse().map_err(anyhow::Error::msg)?;
            match v.authorize(r.kind, &r.pattern, now()) {
                Ok(()) => {
                    println!("allowed: {r}");
                    Ok(0)
                }
                Err(e) => {
                    eprintln!("onus: {e}");
                    Ok(1)
                }
            }
        }
    }
}

/// Writes a private key readable only by its owner.
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    writeln!(f, "{text}")?;
    Ok(())
}

pub fn scope(cmd: ScopeCmd) -> Result<i32> {
    match cmd {
        ScopeCmd::Suggest { plan, repo } => {
            let plan = Plan::parse(&std::fs::read_to_string(&plan)?)?;
            let config = onus_map::config::load_from_tree(&repo)?;
            let sensitive: Vec<String> = match &config {
                Some(c) => c
                    .config
                    .labels
                    .iter()
                    .filter(|(_, l)| l.sensitivity >= onus_core::Sensitivity::Medium)
                    .map(|(name, _)| name.clone())
                    .collect(),
                None => vec!["auth".into(), "payments".into(), "pii".into()],
            };
            let map = onus_cli::build(&repo, config, None)?;
            let s = onus_doors::plan::suggest_reads(&map, &plan.writes, &sensitive);
            println!("{}", serde_json::to_string_pretty(&s)?);
            Ok(0)
        }
    }
}

pub fn gateway(cmd: GatewayCmd) -> Result<i32> {
    match cmd {
        GatewayCmd::Serve {
            repo,
            remote,
            base,
            key_public,
            state,
            listen,
        } => {
            let repo = std::fs::canonicalize(&repo)
                .with_context(|| format!("no repository at {}", repo.display()))?;
            std::fs::create_dir_all(&state)?;
            let state = std::fs::canonicalize(&state)?;
            let gateway = Gateway {
                repo,
                remote,
                base,
                state,
                public_key: read_key(&key_public)?,
            };
            let onus = std::env::current_exe()?;
            onus_doors::gateway::server::serve(
                onus_doors::gateway::server::Server { gateway, onus },
                listen,
                |addr| eprintln!("onus: gateway at http://{addr}/<task>.git"),
            )?;
            Ok(0)
        }
        GatewayCmd::Hook { stage, state, task } => {
            let gw = Gateway::load(&state)?;
            let stdin = std::io::stdin().lock();
            match stage.as_str() {
                "pre-receive" => match onus_doors::gateway::hook::pre_receive(&gw, &task, stdin) {
                    Ok(()) => Ok(0),
                    Err(e) => {
                        eprintln!("{e:#}");
                        Ok(1)
                    }
                },
                "post-receive" => {
                    for note in onus_doors::gateway::hook::post_receive(&gw, &task, stdin)? {
                        eprintln!("{note}");
                    }
                    Ok(0)
                }
                other => bail!("unknown hook `{other}`"),
            }
        }
    }
}

pub fn audit_verify(log: &Path) -> Result<i32> {
    let entries = AuditLog::new(log).verify()?;
    println!("{}: {} entries, chain intact", log.display(), entries.len());
    Ok(0)
}
