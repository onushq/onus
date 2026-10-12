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

pub fn read_key(path: &Path) -> Result<String> {
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

pub fn token_json(v: &token::Verified) -> serde_json::Value {
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
            println!("{}", keygen(&out)?.display());
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
            println!("{}", mint_plan(&repo, &plan, &key, audit.as_deref())?);
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

/// Makes a root key pair in `out`: `root.key` (private) and `root.pub`.
/// Returns the public key's path.
pub fn keygen(out: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out)?;
    let key = RootKey::generate();
    let private = out.join("root.key");
    if private.exists() {
        bail!("{} exists; move it away first", private.display());
    }
    write_private(&private, &key.private_hex())?;
    std::fs::write(out.join("root.pub"), format!("{}\n", key.public_hex()))?;
    Ok(out.join("root.pub"))
}

/// Mints a task token from a plan, resolving components and contracts
/// through the map of `repo` when the plan names them.
pub fn mint_plan(repo: &Path, plan: &Plan, key: &Path, audit: Option<&Path>) -> Result<String> {
    let root = RootKey::from_hex(&read_key(key)?)?;
    let grant = if plan.needs_map() {
        let config = onus_map::config::load_from_tree(repo)?;
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
        let map = onus_cli::build(repo, config, None)?;
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
    Ok(minted)
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
            let s = suggest(&repo, &plan)?;
            println!("{}", serde_json::to_string_pretty(&s)?);
            Ok(0)
        }
    }
}

/// Reads the map suggests for a plan's writes, leaving out sensitive
/// components.
pub fn suggest(repo: &Path, plan: &Plan) -> Result<onus_doors::plan::Suggestion> {
    let (config, sensitive) = sensitive_labels(repo)?;
    let map = onus_cli::build(repo, config, None)?;
    Ok(onus_doors::plan::suggest_reads(
        &map,
        &plan.writes,
        &sensitive,
    ))
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
    pub(crate) task: String,
    #[arg(long, value_enum, default_value = "permission")]
    pub(crate) kind: KindArg,
    /// A right asked for, such as write:path:services/billing/src/** (repeatable).
    #[arg(long = "scope", value_name = "RIGHT")]
    pub(crate) scopes: Vec<String>,
    /// Evidence, strongest first: failing-test:<file>, trace:<file>, map-path:<path>,
    /// draft-diff:<file>, rationale:<text> (repeatable).
    #[arg(long = "evidence", value_name = "KIND:REF")]
    pub(crate) evidence: Vec<String>,
    /// Why the task needs it.
    #[arg(long)]
    pub(crate) reason: String,
    /// The repository whose map gives the blast radius and labels.
    #[arg(long, default_value = ".", value_name = "DIR")]
    pub(crate) repo: PathBuf,
    /// Write the request here instead of printing it.
    #[arg(long, value_name = "FILE")]
    pub(crate) out: Option<PathBuf>,
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
    /// Apply the policy to a request without minting anything: reproduce failing
    /// tests at --reproduce-at (with onus.yaml's tests.command and declared
    /// environment of --repo, a trusted checkout), then say whether policy would
    /// grant it. Exit 0: it would; 3: a person decides. Nothing is audited.
    Assess {
        /// The request (JSON from `onus escalate`).
        request: PathBuf,
        /// Reproduce failing-test evidence at this commit of --repo.
        #[arg(long, value_name = "REF")]
        reproduce_at: Option<String>,
        /// The trusted checkout whose onus.yaml gives the test command and environment.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// The container image. Default: onus.yaml's environment image (or the
        /// devcontainer's), else node:22.
        #[arg(long)]
        image: Option<String>,
        /// A setup command run first, with network. Default: onus.yaml's environment setup.
        #[arg(long)]
        setup: Option<String>,
        /// The test command; `{test}` becomes the failing test's file. Default:
        /// onus.yaml's tests.command; without one, failing tests are not reproduced.
        #[arg(long)]
        test_command: Option<String>,
        /// The most dependent files an automatic grant may reach.
        #[arg(long, default_value_t = 20)]
        max_blast_radius: u32,
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

pub fn sensitive_labels(repo: &Path) -> Result<(Option<onus_map::LoadedConfig>, Vec<String>)> {
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

/// A new escalation request, with its blast radius and sensitive labels
/// from the map of `repo`.
pub fn new_request(
    repo: &Path,
    task: &str,
    kind: onus_doors::escalation::EscalationKind,
    scopes: &[String],
    evidence: &[String],
    reason: &str,
) -> Result<onus_doors::escalation::Request> {
    let scopes = rights(scopes)?;
    if scopes.is_empty() && kind == onus_doors::escalation::EscalationKind::Permission {
        bail!("a permission request names the rights it asks for (--scope)");
    }
    let evidence = evidence
        .iter()
        .map(|e| onus_doors::escalation::Evidence::parse(e))
        .collect::<Result<Vec<_>>>()?;
    let (config, labels) = sensitive_labels(repo)?;
    let map = onus_cli::build(repo, config, None)?;
    Ok(onus_doors::escalation::Request::new(
        task,
        kind,
        scopes,
        evidence,
        reason,
        Some(&map),
        &labels,
        now(),
    ))
}

pub fn escalate(args: EscalateArgs) -> Result<i32> {
    let req = new_request(
        &args.repo,
        &args.task,
        args.kind.into(),
        &args.scopes,
        &args.evidence,
        &args.reason,
    )?;
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

/// Checks an escalation id: letters, digits, `-`, `_` and `.`, so it names
/// a file in the escalations folder and nowhere else.
pub fn check_request_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        bail!("`{id}` is not a request id");
    }
    Ok(())
}

/// Verifies the escalation ids a submission lists: each request exists, is
/// unchanged since it was filed, is for `task` (when the submission has a
/// token) and was granted with a root-signed token that holds exactly what
/// it asked for. Requests are in `dir`; decisions on those not granted are
/// read from the audit log beside it. Returns the ids, each once.
pub fn verify_escalations(
    dir: &Path,
    ids: &[String],
    public_key: &Path,
    task: Option<&str>,
) -> Result<Vec<String>> {
    let audit = dir.parent().map(|d| d.join("audit.jsonl"));
    verify_escalations_with(dir, audit.as_deref(), ids, public_key, task)
}

/// [`verify_escalations`] with the audit log that holds the decisions.
pub fn verify_escalations_with(
    dir: &Path,
    audit: Option<&Path>,
    ids: &[String],
    public_key: &Path,
    task: Option<&str>,
) -> Result<Vec<String>> {
    let root = token::public_key(&read_key(public_key)?)?;
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if out.contains(id) {
            continue;
        }
        let refuse = |what: String| -> anyhow::Error {
            anyhow::anyhow!(
                "{what}; check it with onus_escalation (or onus ui), and submit without it until it is granted"
            )
        };
        if check_request_id(id).is_err() {
            return Err(refuse(format!("`{id}` is not an escalation id")));
        }
        let path = dir.join(format!("{id}.json"));
        if !path.exists() {
            return Err(refuse(format!(
                "there is no escalation `{id}` in {}",
                dir.display()
            )));
        }
        let req = load_request(&path).map_err(|e| refuse(format!("escalation `{id}`: {e:#}")))?;
        if let Some(t) = task
            && req.task != t
        {
            return Err(refuse(format!(
                "escalation `{id}` belongs to task `{}`, not `{t}`",
                req.task
            )));
        }
        let Ok(granted) = std::fs::read_to_string(path.with_extension("token")) else {
            let decision = audit
                .and_then(|log| AuditLog::new(log).verify().ok())
                .and_then(|entries| {
                    entries.into_iter().rev().find(|e| {
                        e.subject == req.id
                            && matches!(e.action.as_str(), "escalate" | "grant" | "deny" | "answer")
                    })
                });
            let state = match decision {
                Some(e) if e.action == "deny" => format!("was denied: {}", e.reason),
                Some(e) if e.decision == "needs-person" => "is waiting for a person".into(),
                Some(e) if e.decision == "sent-to-onus-cloud" => {
                    "is waiting for a person in Onus Cloud".into()
                }
                Some(e) if e.action == "answer" => {
                    format!("was answered, not granted: {}", e.reason)
                }
                Some(e) if e.decision == "granted" => {
                    "was granted, but its token is not kept beside the request".into()
                }
                _ => "is still open".into(),
            };
            return Err(refuse(format!("escalation `{id}` {state}")));
        };
        onus_doors::escalation::verify_granted(&req, granted.trim(), &root)
            .map_err(|why| refuse(format!("escalation `{id}` cannot be used: {why}")))?;
        out.push(id.clone());
    }
    Ok(out)
}

pub fn load_request(path: &Path) -> Result<onus_doors::escalation::Request> {
    serde_json::from_str(
        &std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?,
    )
    .with_context(|| format!("{} is not an escalation request", path.display()))
}

fn record(audit: Option<&Path>, r: Record) -> Result<()> {
    if let Some(log) = audit {
        AuditLog::new(log).append(r, now())?;
    }
    Ok(())
}

/// Keeps a granted token beside its request (`<request>.token`), readable
/// only by its owner, so the agent that asked can collect it
/// (`onus_escalation`).
pub fn keep_token(request: &Path, token: &str) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    writeln!(opts.open(request.with_extension("token"))?, "{token}")?;
    Ok(())
}

/// How `decide` reproduces failing-test evidence.
#[derive(Debug, Clone)]
pub struct Reproduce {
    pub repo: PathBuf,
    pub at: String,
    pub image: String,
    pub setup: Option<String>,
    /// `{test}` becomes the failing test's file.
    pub test_command: String,
}

/// Applies the policy to a request without minting anything: reproduces its
/// failing tests first (when `reproduce` is given), then decides. Whether a
/// test reproduced is only ever what this run saw, never what the request
/// says. A failing test is run only when its reference is a plain path to a
/// test file that exists at the commit; otherwise it counts as not
/// reproduced, and a person decides with the reasons saying why.
pub fn assess_request(
    mut req: onus_doors::escalation::Request,
    reproduce: Option<&Reproduce>,
    max_blast_radius: u32,
) -> Result<(
    onus_doors::escalation::Request,
    onus_doors::escalation::Decision,
)> {
    use onus_doors::escalation::{Decision, Policy, decide};
    if req.content_id() != req.id {
        bail!(
            "request {} was changed after it was filed; file it again with onus escalate",
            req.id
        );
    }
    for e in req.evidence.iter_mut().filter(|e| e.grade == 1) {
        e.reproduced = None;
        e.run = None;
    }
    let mut refused = Vec::new();
    if let Some(r) = reproduce
        && req.evidence.iter().any(|e| e.grade == 1)
    {
        if r.at.starts_with('-') {
            bail!("`{}` is not a commit", r.at);
        }
        let config = onus_map::config::load_from_tree(&r.repo)?;
        let tests = onus_map::build::test_globs(config.as_ref().map(|c| &c.config));
        for e in req.evidence.iter_mut().filter(|e| e.grade == 1) {
            if let Some(why) = refuse_test(&r.repo, &r.at, &e.reference, |p| tests.is_match(p)) {
                e.reproduced = Some(false);
                refused.push(why);
                continue;
            }
            let command = r.test_command.replace("{test}", &e.reference);
            let run =
                onus_doors::runner::run(&r.repo, &r.at, &r.image, r.setup.as_deref(), &command)?;
            e.reproduced = Some(run.failed());
            e.run = Some(run);
        }
    }
    let decision = decide(&req, &Policy { max_blast_radius });
    if refused.is_empty() {
        return Ok((req, decision));
    }
    // Evidence that could not be run sends the request to a person, even
    // when another failing test reproduced.
    let mut reasons = match decision {
        Decision::NeedsPerson { reasons } => reasons,
        Decision::Granted { .. } => vec![],
    };
    reasons.extend(refused);
    Ok((req, Decision::NeedsPerson { reasons }))
}

/// Why failing-test evidence `reference` is not run at `at` of `repo`: it
/// is not a plain repository path, is not a file there, or is not a test
/// file. `None` when it may be run.
fn refuse_test(
    repo: &Path,
    at: &str,
    reference: &str,
    is_test: impl Fn(&str) -> bool,
) -> Option<String> {
    // It goes into the test command as is, so it must not read as an option.
    let plain = !reference.is_empty()
        && !reference.starts_with(['/', '-'])
        && !reference.split('/').any(|part| part == "..")
        && reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/@+-".contains(c));
    if !plain {
        return Some(format!("`{reference}` is not a path"));
    }
    // A file, not a directory that happens to match the test globs.
    let exists = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-t", &format!("{at}:{reference}")])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && o.stdout.trim_ascii() == b"blob");
    if !exists {
        return Some(format!("`{reference}` does not exist at `{at}`"));
    }
    if !is_test(reference) {
        return Some(format!("`{reference}` is not a test file"));
    }
    None
}

/// Decides a request by policy, reproducing failing tests first; a grant
/// carries the new token.
pub fn decide_request(
    req: onus_doors::escalation::Request,
    original: &token::Verified,
    key: &Path,
    reproduce: Option<&Reproduce>,
    max_blast_radius: u32,
    audit: Option<&Path>,
) -> Result<serde_json::Value> {
    use onus_doors::escalation::{Decision, grant};
    let (req, decision) = assess_request(req, reproduce, max_blast_radius)?;
    let (label, reasons, minted) = match &decision {
        Decision::Granted { reasons } => {
            let root = RootKey::from_hex(&read_key(key)?)?;
            (
                "granted",
                reasons.clone(),
                Some(grant(&req, original, &root)?),
            )
        }
        Decision::NeedsPerson { reasons } => ("needs-person", reasons.clone(), None),
    };
    record(
        audit,
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
    Ok(out)
}

/// What `onus escalation assess` prints: the request (with its reproduced
/// evidence), whether policy would grant it, and why. The test command,
/// image and setup default to onus.yaml of `repo`, a trusted checkout, so the
/// agent that filed the request cannot choose what reproduces it.
pub fn assess(
    req: onus_doors::escalation::Request,
    repo: &Path,
    reproduce_at: Option<String>,
    image: Option<String>,
    setup: Option<String>,
    test_command: Option<String>,
    max_blast_radius: u32,
) -> Result<serde_json::Value> {
    use onus_doors::escalation::Decision;
    let has_test = req.evidence.iter().any(|e| e.grade == 1);
    let (reproduce, why_not_run) =
        trusted_reproduce(repo, reproduce_at, image, setup, test_command, has_test)?;
    let (req, decision) = assess_request(req, reproduce.as_ref(), max_blast_radius)?;
    let (label, mut reasons) = match decision {
        Decision::Granted { reasons } => ("granted", reasons),
        Decision::NeedsPerson { reasons } => ("needs-person", reasons),
    };
    reasons.extend(why_not_run);
    Ok(serde_json::json!({
        "request": req,
        "decision": label,
        "reasons": reasons,
        "evidence": req.evidence,
    }))
}

/// How failing tests are reproduced at `at` of `repo`, a trusted checkout: with the
/// test command, image and setup given, else onus.yaml's `tests.command` and declared
/// environment. `None` with the reason when they are not reproduced.
pub fn trusted_reproduce(
    repo: &Path,
    at: Option<String>,
    image: Option<String>,
    setup: Option<String>,
    test_command: Option<String>,
    has_test: bool,
) -> Result<(Option<Reproduce>, Option<String>)> {
    let config = onus_map::config::load_from_tree(repo)?.map(|c| c.config);
    let test_command = test_command
        .or_else(|| config.as_ref().and_then(|c| c.tests.command.clone()))
        .filter(|c| !c.trim().is_empty());
    let mut why_not_run = None;
    let reproduce = match (&at, &test_command) {
        (Some(at), Some(command)) if has_test => {
            let environment = config.as_ref().and_then(|c| c.environment.as_ref());
            let declared = match onus_env::spec::resolve(repo, environment) {
                Ok(spec) => Some(spec),
                // Nothing declared: the default image.
                Err(_) if environment.is_none() && !has_devcontainer(repo) => None,
                // Declared but unusable: running elsewhere would prove nothing.
                Err(e) if image.is_none() => {
                    why_not_run = Some(format!(
                        "the environment cannot be resolved ({e:#}): failing tests are not reproduced"
                    ));
                    None
                }
                Err(_) => None,
            };
            why_not_run.is_none().then(|| Reproduce {
                repo: repo.to_path_buf(),
                at: at.clone(),
                image: image
                    .or_else(|| declared.as_ref().map(|d| d.image.clone()))
                    .unwrap_or_else(|| "node:22".into()),
                setup: setup.or_else(|| declared.and_then(|d| d.setup)),
                test_command: command.clone(),
            })
        }
        _ => None,
    };
    if has_test && test_command.is_none() {
        why_not_run = Some(
            "no test command is configured (onus.yaml tests.command): failing tests are not reproduced"
                .into(),
        );
    }
    Ok((reproduce, why_not_run))
}

/// Whether `repo` has a devcontainer, which declares an environment as
/// onus.yaml's `environment:` does.
fn has_devcontainer(repo: &Path) -> bool {
    [".devcontainer/devcontainer.json", ".devcontainer.json"]
        .iter()
        .any(|p| repo.join(p).is_file())
}

/// Grants a request as a person: the new token.
pub fn grant_request(
    req: &onus_doors::escalation::Request,
    original: &token::Verified,
    key: &Path,
    by: &str,
    audit: Option<&Path>,
) -> Result<String> {
    let root = RootKey::from_hex(&read_key(key)?)?;
    let minted = onus_doors::escalation::grant(req, original, &root)?;
    record(
        audit,
        Record {
            actor: by.to_string(),
            action: "grant".into(),
            subject: req.id.clone(),
            decision: "granted".into(),
            reason: format!("granted by {by}"),
            details: serde_json::to_value(req)?,
        },
    )?;
    Ok(minted)
}

/// Denies a request as a person.
pub fn deny_request(
    req: &onus_doors::escalation::Request,
    by: &str,
    reason: &str,
    audit: Option<&Path>,
) -> Result<()> {
    record(
        audit,
        Record {
            actor: by.to_string(),
            action: "deny".into(),
            subject: req.id.clone(),
            decision: "denied".into(),
            reason: reason.to_string(),
            details: serde_json::to_value(req)?,
        },
    )
}

pub fn escalation(cmd: EscalationCmd) -> Result<i32> {
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
            let req = load_request(&request)?;
            let original = token.verify()?;
            let reproduce = reproduce_at.map(|at| Reproduce {
                repo,
                at,
                image,
                setup,
                test_command,
            });
            let out = decide_request(
                req,
                &original,
                &key,
                reproduce.as_ref(),
                max_blast_radius,
                audit.as_deref(),
            )?;
            if let Some(t) = out["token"].as_str() {
                keep_token(&request, t)?;
            }
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(if out["decision"] == "granted" { 0 } else { 3 })
        }
        EscalationCmd::Assess {
            request,
            reproduce_at,
            repo,
            image,
            setup,
            test_command,
            max_blast_radius,
        } => {
            let req = load_request(&request)?;
            let out = assess(
                req,
                &repo,
                reproduce_at,
                image,
                setup,
                test_command,
                max_blast_radius,
            )?;
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(if out["decision"] == "granted" { 0 } else { 3 })
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
            let minted = grant_request(&req, &original, &key, &by, audit.as_deref())?;
            keep_token(&request, &minted)?;
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
            deny_request(&req, &by, &reason, audit.as_deref())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use onus_doors::escalation::{EscalationKind, Evidence, Request};
    use onus_doors::token::Grant;

    struct Setup {
        _dir: tempfile::TempDir,
        escalations: PathBuf,
        audit: PathBuf,
        public: PathBuf,
        key: PathBuf,
        token: token::Verified,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("keys");
        let public = keygen(&keys).unwrap();
        let key = keys.join("root.key");
        let root = RootKey::from_hex(&read_key(&key).unwrap()).unwrap();
        let raw = token::mint(
            &root,
            &Grant {
                task: "sms".into(),
                rights: rights(&["write:path:services/notifications/**".into()]).unwrap(),
                expires: now() + 3600,
                agent: None,
            },
        )
        .unwrap();
        let token = token::verify(&raw, &root.public()).unwrap();
        let escalations = dir.path().join(".onus/escalations");
        std::fs::create_dir_all(&escalations).unwrap();
        Setup {
            audit: dir.path().join(".onus/audit.jsonl"),
            escalations,
            public,
            key,
            token,
            _dir: dir,
        }
    }

    fn file(s: &Setup, task: &str, scope: &str) -> Request {
        let req = Request::new(
            task,
            EscalationKind::Permission,
            vec![scope.parse().unwrap()],
            vec![Evidence::parse("rationale:needed").unwrap()],
            "needed",
            None,
            &[],
            now(),
        );
        std::fs::write(
            s.escalations.join(format!("{}.json", req.id)),
            serde_json::to_string_pretty(&req).unwrap(),
        )
        .unwrap();
        req
    }

    fn verify(s: &Setup, id: &str) -> Result<Vec<String>> {
        verify_escalations(&s.escalations, &[id.to_string()], &s.public, Some("sms"))
    }

    fn needs_person(d: &onus_doors::escalation::Decision) -> Vec<String> {
        match d {
            onus_doors::escalation::Decision::NeedsPerson { reasons } => reasons.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn assessing_a_rationale_sends_it_to_a_person() {
        let req = Request::new(
            "sms",
            EscalationKind::Permission,
            vec!["write:path:services/orders/**".parse().unwrap()],
            vec![Evidence::parse("rationale:needed").unwrap()],
            "needed",
            None,
            &[],
            now(),
        );
        let (_, decision) = assess_request(req, None, 20).unwrap();
        let reasons = needs_person(&decision);
        assert!(reasons.iter().any(|r| r.contains("grade 5")), "{reasons:?}");
    }

    #[test]
    fn failing_tests_that_are_not_test_files_at_the_commit_are_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/a.ts"), "export const a = 1;\n").unwrap();
        std::fs::write(repo.join("src/a.test.ts"), "throw new Error();\n").unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(repo)
                    .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                    .args(["-c", "commit.gpgsign=false"])
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        std::fs::create_dir_all(repo.join("src/b.test.d")).unwrap();
        std::fs::write(repo.join("src/b.test.d/x.ts"), "export {};\n").unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "src/a.ts", "src/b.test.d"]);
        git(&["commit", "-q", "-m", "a"]);
        // Never run: a container engine is not needed to refuse these.
        let reproduce = Reproduce {
            repo: repo.to_path_buf(),
            at: "HEAD".into(),
            image: "node:22".into(),
            setup: None,
            test_command: "npx vitest run {test}".into(),
        };
        let assess = |reference: &str| {
            let req = Request::new(
                "sms",
                EscalationKind::Permission,
                vec!["write:path:src/**".parse().unwrap()],
                vec![Evidence::parse(&format!("failing-test:{reference}")).unwrap()],
                "needed",
                None,
                &[],
                now(),
            );
            let (req, decision) = assess_request(req, Some(&reproduce), 20).unwrap();
            assert_eq!(req.evidence[0].reproduced, Some(false));
            assert!(req.evidence[0].run.is_none());
            needs_person(&decision)
        };
        let reasons = assess("a.test.ts; exit 1");
        assert!(
            reasons.contains(&"`a.test.ts; exit 1` is not a path".to_string()),
            "{reasons:?}"
        );
        for path in [
            "../a.test.ts",
            "/etc/passwd",
            "src/../src/a.test.ts",
            "--reporter=x.test.ts",
        ] {
            let reasons = assess(path);
            assert!(
                reasons.contains(&format!("`{path}` is not a path")),
                "{reasons:?}"
            );
        }
        // In the worktree, but not committed.
        let reasons = assess("src/a.test.ts");
        assert!(
            reasons.contains(&"`src/a.test.ts` does not exist at `HEAD`".to_string()),
            "{reasons:?}"
        );
        // A directory is not a test file, whatever its name.
        let reasons = assess("src/b.test.d");
        assert!(
            reasons.contains(&"`src/b.test.d` does not exist at `HEAD`".to_string()),
            "{reasons:?}"
        );
        let reasons = assess("src/a.ts");
        assert!(
            reasons.contains(&"`src/a.ts` is not a test file".to_string()),
            "{reasons:?}"
        );
        // A commit that reads as an option is refused.
        let reproduce = Reproduce {
            at: "--output=x".into(),
            ..reproduce
        };
        let req = Request::new(
            "sms",
            EscalationKind::Permission,
            vec!["write:path:src/**".parse().unwrap()],
            vec![Evidence::parse("failing-test:src/a.test.ts").unwrap()],
            "needed",
            None,
            &[],
            now(),
        );
        let err = assess_request(req, Some(&reproduce), 20).unwrap_err();
        assert!(err.to_string().contains("is not a commit"), "{err}");
    }

    #[test]
    fn what_a_request_says_about_reproduction_is_not_believed() {
        let mut evidence = Evidence::parse("failing-test:src/a.test.ts").unwrap();
        evidence.reproduced = Some(true);
        let req = Request::new(
            "sms",
            EscalationKind::Permission,
            vec!["write:path:src/**".parse().unwrap()],
            vec![evidence],
            "needed",
            None,
            &[],
            now(),
        );
        let (assessed, decision) = assess_request(req.clone(), None, 20).unwrap();
        assert_eq!(assessed.evidence[0].reproduced, None);
        let reasons = needs_person(&decision);
        assert!(
            reasons.iter().any(|r| r.contains("was not reproduced")),
            "{reasons:?}"
        );

        // Changed after it was filed: refused.
        let mut changed = req;
        changed.scopes.push("write:path:lib/**".parse().unwrap());
        let err = assess_request(changed, None, 20).unwrap_err();
        assert!(
            err.to_string().contains("was changed after it was filed"),
            "{err}"
        );
    }

    #[test]
    fn only_granted_escalations_of_the_task_verify() {
        let s = setup();
        let audit = Some(s.audit.as_path());

        // Granted by a person: it verifies, once however often it is listed.
        let granted = file(&s, "sms", "write:path:services/orders/src/**");
        let t = grant_request(&granted, &s.token, &s.key, "@lead", audit).unwrap();
        keep_token(&s.escalations.join(format!("{}.json", granted.id)), &t).unwrap();
        let ids = vec![granted.id.clone(), granted.id.clone()];
        assert_eq!(
            verify_escalations(&s.escalations, &ids, &s.public, Some("sms")).unwrap(),
            std::slice::from_ref(&granted.id)
        );
        // Without a token, the task is not compared.
        assert!(verify_escalations(&s.escalations, &ids, &s.public, None).is_ok());

        // Waiting for a person (the policy said so), and still open.
        let pending = file(&s, "sms", "write:path:services/billing/**");
        decide_request(pending.clone(), &s.token, &s.key, None, 20, audit).unwrap();
        let err = verify(&s, &pending.id).unwrap_err().to_string();
        assert!(err.contains("is waiting for a person"), "{err}");
        assert!(
            err.contains("submit without it until it is granted"),
            "{err}"
        );
        let open = file(&s, "sms", "write:path:services/users/**");
        assert!(
            verify(&s, &open.id)
                .unwrap_err()
                .to_string()
                .contains("is still open")
        );

        // Denied, with the reason.
        let denied = file(&s, "sms", "write:path:services/auth/**");
        deny_request(&denied, "@lead", "auth is out of scope", audit).unwrap();
        let err = verify(&s, &denied.id).unwrap_err().to_string();
        assert!(err.contains("was denied: auth is out of scope"), "{err}");

        // Another task's request.
        let other = file(&s, "other", "write:path:services/orders/src/**");
        let err = verify(&s, &other.id).unwrap_err().to_string();
        assert!(err.contains("belongs to task `other`, not `sms`"), "{err}");

        // A request the task filed itself, with its own token copied beside
        // it: root-signed, but nobody granted it.
        let forged = file(&s, "sms", "write:path:services/notifications/src/**");
        let root = RootKey::from_hex(&read_key(&s.key).unwrap()).unwrap();
        let own = token::mint(
            &root,
            &Grant {
                task: "sms".into(),
                rights: s.token.rights.clone(),
                expires: s.token.expires,
                agent: None,
            },
        )
        .unwrap();
        keep_token(&s.escalations.join(format!("{}.json", forged.id)), &own).unwrap();
        let err = verify(&s, &forged.id).unwrap_err().to_string();
        assert!(err.contains("was not minted to grant it"), "{err}");

        // A request edited after the grant.
        let path = s.escalations.join(format!("{}.json", granted.id));
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("services/orders/src/**", "**")).unwrap();
        let err = verify(&s, &granted.id).unwrap_err().to_string();
        assert!(err.contains("was changed after it was filed"), "{err}");

        // Ids that name nothing, or outside the folder.
        assert!(
            verify(&s, "deadbeef0000")
                .unwrap_err()
                .to_string()
                .contains("no escalation")
        );
        assert!(
            verify(&s, "../x")
                .unwrap_err()
                .to_string()
                .contains("not an escalation id")
        );
    }
}
