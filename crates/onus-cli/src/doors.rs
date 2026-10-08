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
        /// The repository whose map resolves `writeComponents`, `readContracts` and
        /// `escalateBefore`.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
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
        TokenCmd::Mint {
            plan,
            key,
            audit,
            repo,
        } => {
            let plan = Plan::parse(
                &std::fs::read_to_string(&plan)
                    .with_context(|| format!("cannot read {}", plan.display()))?,
            )?;
            let root = RootKey::from_hex(&read_key(&key)?)?;
            let grant = if plan.needs_map() {
                let config = onus_map::config::load_from_tree(&repo)?;
                let contracts = config
                    .as_ref()
                    .map(|c| {
                        c.config
                            .contracts
                            .iter()
                            .map(|(name, contract)| (name.clone(), contract.symbol.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let map = onus_cli::build(&repo, config, None)?;
                plan.grant_with_map(now(), &map, &contracts)?
            } else {
                plan.grant(now())?
            };
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum KindArg {
    Permission,
    BrokenTest,
    ContradictorySpec,
    ImpossibleTask,
}

impl From<KindArg> for onus_doors::escalation::EscalationKind {
    fn from(k: KindArg) -> Self {
        use onus_doors::escalation::EscalationKind as K;
        match k {
            KindArg::Permission => K::Permission,
            KindArg::BrokenTest => K::BrokenTest,
            KindArg::ContradictorySpec => K::ContradictorySpec,
            KindArg::ImpossibleTask => K::ImpossibleTask,
        }
    }
}

#[derive(Debug, Args)]
pub struct EscalateArgs {
    /// The task asking.
    #[arg(long)]
    task: String,
    #[arg(long, value_enum, default_value = "permission")]
    kind: KindArg,
    /// A right asked for, such as write:path:services/billing/src/** (repeatable).
    #[arg(long = "scope", value_name = "RIGHT")]
    scopes: Vec<String>,
    /// Evidence, strongest first: failing-test:<file>, trace:<file>, map-path:<path>,
    /// draft-diff:<file>, rationale:<text> (repeatable).
    #[arg(long = "evidence", value_name = "KIND:REF")]
    evidence: Vec<String>,
    /// Why the task needs it.
    #[arg(long)]
    reason: String,
    /// The repository whose map gives the blast radius and labels.
    #[arg(long, default_value = ".", value_name = "DIR")]
    repo: PathBuf,
    /// Write the request here instead of printing it.
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum EscalationCmd {
    /// Decide a request by policy: grant it (printing the new token) when it is
    /// low risk with a reproduced failing test, else say why a person must decide
    /// (exit code 3).
    Decide {
        /// The request (JSON from `onus escalate`).
        request: PathBuf,
        #[command(flatten)]
        token: TokenArgs,
        /// The root private key that mints the grant.
        #[arg(long, value_name = "FILE")]
        key: PathBuf,
        /// Reproduce failing-test evidence at this ref of --repo.
        #[arg(long, value_name = "REF")]
        reproduce_at: Option<String>,
        /// The repository to reproduce in.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// The container image for the test runner.
        #[arg(long, default_value = "node:22")]
        image: String,
        /// A setup command run first, with network (such as `npm ci`).
        #[arg(long)]
        setup: Option<String>,
        /// The test command; `{test}` becomes the failing test's file.
        #[arg(long, default_value = "npx vitest run {test}")]
        test_command: String,
        /// The most dependent files an automatic grant may reach.
        #[arg(long, default_value_t = 20)]
        max_blast_radius: u32,
        #[arg(long, value_name = "FILE")]
        audit: Option<PathBuf>,
    },
    /// Grant a request as a person: prints the new token.
    Grant {
        request: PathBuf,
        #[command(flatten)]
        token: TokenArgs,
        #[arg(long, value_name = "FILE")]
        key: PathBuf,
        /// Who decided.
        #[arg(long)]
        by: String,
        #[arg(long, value_name = "FILE")]
        audit: Option<PathBuf>,
    },
    /// Deny a request as a person.
    Deny {
        request: PathBuf,
        #[arg(long)]
        by: String,
        #[arg(long)]
        reason: String,
        #[arg(long, value_name = "FILE")]
        audit: Option<PathBuf>,
    },
}

fn sensitive_labels(repo: &Path) -> Result<(Option<onus_map::LoadedConfig>, Vec<String>)> {
    let config = onus_map::config::load_from_tree(repo)?;
    let labels = match &config {
        Some(c) => c
            .config
            .labels
            .iter()
            .filter(|(_, l)| l.sensitivity >= onus_core::Sensitivity::Medium)
            .map(|(name, _)| name.clone())
            .collect(),
        None => vec!["auth".into(), "payments".into(), "pii".into()],
    };
    Ok((config, labels))
}

pub fn escalate(args: EscalateArgs) -> Result<i32> {
    let scopes = rights(&args.scopes)?;
    if scopes.is_empty() && args.kind == KindArg::Permission {
        bail!("a permission request names the rights it asks for (--scope)");
    }
    let evidence = args
        .evidence
        .iter()
        .map(|e| onus_doors::escalation::Evidence::parse(e))
        .collect::<Result<Vec<_>>>()?;
    let (config, labels) = sensitive_labels(&args.repo)?;
    let map = onus_cli::build(&args.repo, config, None)?;
    let req = onus_doors::escalation::Request::new(
        &args.task,
        args.kind.into(),
        scopes,
        evidence,
        &args.reason,
        Some(&map),
        &labels,
        now(),
    );
    let json = serde_json::to_string_pretty(&req)?;
    match args.out {
        Some(path) => {
            std::fs::write(&path, format!("{json}\n"))?;
            eprintln!(
                "onus: escalation {} written to {} (blast radius {}, evidence grade {})",
                req.id,
                path.display(),
                req.blast_radius,
                req.best_grade().map_or("none".into(), |g| g.to_string())
            );
        }
        None => println!("{json}"),
    }
    Ok(0)
}

fn load_request(path: &Path) -> Result<onus_doors::escalation::Request> {
    serde_json::from_str(
        &std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?,
    )
    .with_context(|| format!("{} is not an escalation request", path.display()))
}

fn record(audit: &Option<PathBuf>, r: Record) -> Result<()> {
    if let Some(log) = audit {
        AuditLog::new(log).append(r, now())?;
    }
    Ok(())
}

pub fn escalation(cmd: EscalationCmd) -> Result<i32> {
    use onus_doors::escalation::{Decision, Policy, decide, grant};
    match cmd {
        EscalationCmd::Decide {
            request,
            token,
            key,
            reproduce_at,
            repo,
            image,
            setup,
            test_command,
            max_blast_radius,
            audit,
        } => {
            let mut req = load_request(&request)?;
            let original = token.verify()?;
            if let Some(at) = &reproduce_at {
                for e in req.evidence.iter_mut().filter(|e| e.grade == 1) {
                    let command = test_command.replace("{test}", &e.reference);
                    let run =
                        onus_doors::runner::run(&repo, at, &image, setup.as_deref(), &command)?;
                    e.reproduced = Some(run.failed());
                    e.run = Some(run);
                }
            }
            let decision = decide(&req, &Policy { max_blast_radius });
            let (label, reasons, minted) = match &decision {
                Decision::Granted { reasons } => {
                    let root = RootKey::from_hex(&read_key(&key)?)?;
                    (
                        "granted",
                        reasons.clone(),
                        Some(grant(&req, &original, &root)?),
                    )
                }
                Decision::NeedsPerson { reasons } => ("needs-person", reasons.clone(), None),
            };
            record(
                &audit,
                Record {
                    actor: req.task.clone(),
                    action: "escalate".into(),
                    subject: req.id.clone(),
                    decision: label.into(),
                    reason: reasons.join("; "),
                    details: serde_json::to_value(&req)?,
                },
            )?;
            let mut out = serde_json::json!({
                "request": req.id,
                "decision": label,
                "reasons": reasons,
                "evidence": req.evidence,
            });
            if let Some(t) = minted {
                out["token"] = serde_json::json!(t);
            }
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(if label == "granted" { 0 } else { 3 })
        }
        EscalationCmd::Grant {
            request,
            token,
            key,
            by,
            audit,
        } => {
            let req = load_request(&request)?;
            let original = token.verify()?;
            let root = RootKey::from_hex(&read_key(&key)?)?;
            let minted = grant(&req, &original, &root)?;
            record(
                &audit,
                Record {
                    actor: by.clone(),
                    action: "grant".into(),
                    subject: req.id.clone(),
                    decision: "granted".into(),
                    reason: format!("granted by {by}"),
                    details: serde_json::to_value(&req)?,
                },
            )?;
            println!("{minted}");
            Ok(0)
        }
        EscalationCmd::Deny {
            request,
            by,
            reason,
            audit,
        } => {
            let req = load_request(&request)?;
            record(
                &audit,
                Record {
                    actor: by.clone(),
                    action: "deny".into(),
                    subject: req.id.clone(),
                    decision: "denied".into(),
                    reason: reason.clone(),
                    details: serde_json::to_value(&req)?,
                },
            )?;
            println!("denied {} by {by}: {reason}", req.id);
            Ok(0)
        }
    }
}

#[derive(Debug, Args)]
pub struct RunTestArgs {
    /// The repository.
    #[arg(long, default_value = ".", value_name = "DIR")]
    repo: PathBuf,
    /// The commit to run at.
    #[arg(long, default_value = "HEAD", value_name = "REF")]
    reference: String,
    /// The container image.
    #[arg(long, default_value = "node:22")]
    image: String,
    /// A setup command run first, with network (such as `npm ci`).
    #[arg(long)]
    setup: Option<String>,
    /// The test command (run with `sh -c`, without network).
    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

pub fn run_test(args: RunTestArgs) -> Result<i32> {
    let run = onus_doors::runner::run(
        &args.repo,
        &args.reference,
        &args.image,
        args.setup.as_deref(),
        &args.command.join(" "),
    )?;
    println!("{}", serde_json::to_string_pretty(&run)?);
    Ok(if run.failed() { 1 } else { 0 })
}
