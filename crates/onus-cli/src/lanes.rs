//! Phase 4 commands: submissions, lanes, the judge and outcome records
//! (ADR 0009).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use onus_core::{Lane, LanesConfig, LearnedConfig, SemanticReport};
use onus_lanes::classify::{
    AreaHistories, Classification, ClassifyContext, Learned, LearnedRisk, area, classify,
};
use onus_lanes::judge::{Verdict, judge_with};
use onus_lanes::learned::{self, Model};
use onus_lanes::outcomes::{self, HistoryScope, Outcome};
use onus_lanes::submission::{AgentSetup, Approval, ScopeUsed, Submission};
use onus_lanes::{rollout, signals};

#[derive(Debug, Args)]
pub struct AgentArgs {
    /// The agent's harness (claude-code, codex, cursor, person, …).
    #[arg(long, default_value = "unknown")]
    agent_tool: String,
    /// The agent's model.
    #[arg(long, default_value = "")]
    agent_model: String,
    /// A name for the agent's configuration.
    #[arg(long, default_value = "")]
    agent_config: String,
    /// The team the agent works for.
    #[arg(long, default_value = "")]
    agent_team: String,
}

#[derive(Debug, Args)]
pub struct SubmitArgs {
    #[arg(long, value_name = "REF")]
    base: String,
    #[arg(long, value_name = "REF")]
    head: String,
    #[arg(long, default_value = ".", value_name = "DIR")]
    repo: PathBuf,
    /// The intent: a YAML file, or Markdown with an `onus-intent` block.
    #[arg(long, value_name = "FILE")]
    intent: Option<PathBuf>,
    /// A test run from `onus run-test` (JSON, repeatable).
    #[arg(long = "evidence", value_name = "FILE")]
    evidence: Vec<PathBuf>,
    /// The task token the change was made under.
    #[arg(long, env = "ONUS_TOKEN", value_name = "TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// The root public key that verifies --token.
    #[arg(long, value_name = "FILE")]
    key_public: Option<PathBuf>,
    /// An escalation granted during the task (repeatable). Each is verified: it must be
    /// granted, for the token's task. Needs --key-public.
    #[arg(long = "escalation", value_name = "ID")]
    escalations: Vec<String>,
    /// Where escalation requests are kept (default: .onus/escalations in --repo).
    #[arg(long, value_name = "DIR")]
    escalations_dir: Option<PathBuf>,
    /// A person's approval of a report row, as `<row id>=<who>` (repeatable).
    #[arg(long = "approve", value_name = "ROW=WHO")]
    approvals: Vec<String>,
    #[command(flatten)]
    agent: AgentArgs,
    /// Write the submission here instead of printing it.
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct LaneArgs {
    /// A submission (from `onus submit`).
    #[arg(long, value_name = "FILE", conflicts_with = "report")]
    submission: Option<PathBuf>,
    /// A report (`onus report --format json`), for changes without a submission.
    #[arg(long, value_name = "FILE")]
    report: Option<PathBuf>,
    /// The repository whose onus.yaml holds the lane policy.
    #[arg(long, default_value = ".", value_name = "DIR")]
    repo: PathBuf,
    /// Use this onus.yaml instead.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Outcome records, for the agent setup's track record and the history
    /// of the components the change touches.
    #[arg(long, value_name = "FILE")]
    outcomes: Option<PathBuf>,
    /// The change, as owner/repo#123: history counts only that repository's
    /// changes (for records shared across repositories) and leaves the change
    /// itself out.
    #[arg(long, value_name = "OWNER/REPO#N")]
    change: Option<String>,
    /// Use this model file instead of lanes.learned.model at the base commit.
    #[arg(long, value_name = "FILE")]
    model: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum LanesCmd {
    /// Fit the learned lane model to outcome records; it may only ever raise lanes.
    #[command(after_long_help = "\
Examples:
  onus ci records pull --dir rec
  onus lanes train --outcomes rec/outcomes.jsonl
  onus lanes train --outcomes rec/outcomes.jsonl --dry-run
  onus lanes train --outcomes outcomes.jsonl --config onus.yaml --out lanes-model.json")]
    Train {
        /// Outcome records (JSONL), e.g. from `onus ci records pull`.
        #[arg(long, value_name = "FILE")]
        outcomes: PathBuf,
        /// Where to write the model (default: lanes.learned.model in --repo, else
        /// onus.lanes-model.json).
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// The repository whose onus.yaml holds lanes.learned. Not read when both
        /// --config and --out are given.
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// Use this onus.yaml instead: its lanes.learned thresholds are where the
        /// held-out fifth is measured.
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// A merged change counts as clean only after this many days without a problem.
        #[arg(long, default_value_t = 14)]
        settle_days: u32,
        /// The fewest labelled changes to train on.
        #[arg(long, default_value_t = 50)]
        min_examples: u32,
        /// The fewest problems among them.
        #[arg(long, default_value_t = 10)]
        min_positives: u32,
        /// Measure and print, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum OutcomesCmd {
    /// Record what happened to a change.
    Record {
        /// The outcome records (JSONL).
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        /// The change: owner/repo#123 or a commit.
        #[arg(long)]
        change: String,
        /// The agent setup (tool/model/config).
        #[arg(long)]
        agent: String,
        #[arg(long, value_enum)]
        lane: LaneArg,
        /// merged, reverted, rolled-back (a flag or rollout), incident, closed or open.
        #[arg(long)]
        result: String,
        #[arg(long)]
        verdict: Option<String>,
        #[arg(long)]
        judge: Option<String>,
        /// A person audited the change after an automatic approval.
        #[arg(long)]
        audited: bool,
        /// The audit found a problem the automatic path missed.
        #[arg(long)]
        missed: bool,
        /// The commit the change landed as (to match reverts against).
        #[arg(long)]
        commit: Option<String>,
        /// A component the change touched, for per-component history (repeatable).
        #[arg(long = "component", value_name = "ID")]
        components: Vec<String>,
        /// The change's classification (onus classify), whose features the learned
        /// lane model trains on.
        #[arg(long, value_name = "FILE")]
        classification: Option<PathBuf>,
        /// With --result rolled-back: rolled back on purpose (a flag turned off as
        /// planned). It counts against neither the agent setup nor the components.
        #[arg(long)]
        planned: bool,
    },
    /// Find reverts in git history and record them against the changes they revert.
    IngestReverts {
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// Only reverts after this ref.
        #[arg(long, value_name = "REF")]
        since: Option<String>,
    },
    /// Record a production incident against the change that caused it.
    Incident {
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        /// The change, as recorded (owner/repo#123 or a commit).
        #[arg(long)]
        change: String,
        /// Components and symbols the incident involved (repeatable).
        #[arg(long = "involved", value_name = "ID")]
        involved: Vec<String>,
        /// What happened.
        #[arg(long)]
        note: String,
    },
    /// Where held-out tests should go next: what incidents involved, most often first.
    Backlog {
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
    },
    /// Totals per agent setup, judge configuration and lane, the human-lane
    /// share and the audit miss rate.
    Summary {
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
    },
    /// Record a person's audit of a merged change (what `/onus audit` does).
    Audit {
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        /// The change, as recorded (owner/repo#123 or a commit).
        #[arg(long)]
        change: String,
        /// Who audited: `@login`, or a name.
        #[arg(long)]
        by: String,
        /// ok, or miss when the automatic path missed a problem.
        #[arg(long, value_enum)]
        result: AuditResultArg,
        /// What the automatic path missed, or anything worth keeping.
        #[arg(long)]
        note: Option<String>,
        /// When, in seconds since 1970; now when left out.
        #[arg(long, value_name = "SECONDS")]
        at: Option<u64>,
    },
    /// What waits for a person to audit, and the audit log, as JSON.
    Audits {
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        /// The most changes in each list.
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AuditResultArg {
    Ok,
    Miss,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum LaneArg {
    AutoMerge,
    Judge,
    Human,
    Blocked,
}

impl From<LaneArg> for Lane {
    fn from(l: LaneArg) -> Lane {
        match l {
            LaneArg::AutoMerge => Lane::AutoMerge,
            LaneArg::Judge => Lane::Judge,
            LaneArg::Human => Lane::Human,
            LaneArg::Blocked => Lane::Blocked,
        }
    }
}

pub fn git_lines(repo: &Path, args: &[&str]) -> Result<Vec<String>> {
    Ok(String::from_utf8_lossy(&git_bytes(repo, args)?)
        .lines()
        .map(str::to_string)
        .collect())
}

/// What git prints, as it is.
pub fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .context("cannot run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// What a submission is made of, besides the change itself.
#[derive(Debug, Default)]
pub struct SubmissionParts {
    /// The intent's text: YAML, or Markdown with an `onus-intent` block.
    pub intent: Option<String>,
    /// Whether `intent` is Markdown.
    pub intent_markdown: bool,
    pub evidence: Vec<onus_doors::runner::TestRun>,
    pub scope: Option<ScopeUsed>,
    pub escalations: Vec<String>,
    pub approvals: Vec<Approval>,
    pub agent: AgentSetup,
}

/// The scope a verified token grants, as a submission records it, and the
/// agent it names, if any.
pub fn scope_and_agent(
    token: &str,
    key_public: &Path,
) -> Result<(ScopeUsed, Option<onus_doors::token::AgentClaim>)> {
    let v = onus_doors::token::verify(
        token,
        &onus_doors::token::public_key(&std::fs::read_to_string(key_public)?)?,
    )?;
    Ok((
        ScopeUsed {
            task: v.task.clone(),
            rights: v.rights.iter().map(|r| r.to_string()).collect(),
            attenuations: v.attenuations.clone(),
            expires: Some(v.expires),
            fingerprint: Some(onus_lanes::provenance::fingerprint(token)),
        },
        v.agent,
    ))
}

/// Bundles the change from `base` to `head` of `repo` for review.
pub fn make_submission(
    repo: &Path,
    base: &str,
    head: &str,
    parts: SubmissionParts,
) -> Result<Submission> {
    let intent = match &parts.intent {
        Some(text) if parts.intent_markdown => onus_diff::intent::parse_markdown(text)?,
        Some(text) => onus_diff::intent::parse(text)?,
        None => None,
    };
    let pair = std::sync::Arc::new(onus_cli::materialize_pair(repo, base, head, false)?);
    let opts = onus_cli::DiffOptions {
        intent,
        base_label: onus_cli::ref_label(base, &pair.base.sha),
        head_label: onus_cli::ref_label(head, &pair.head.sha),
        base_commit: Some(pair.base.sha.clone()),
        head_commit: Some(pair.head.sha.clone()),
        changed_paths: Some(pair.changed.clone()),
        ..Default::default()
    };
    let outcome = onus_cli::diff_dirs(pair.base.dir.path(), pair.head.dir.path(), &opts)?;
    submission_with_report(repo, &pair.base.sha, &pair.head.sha, outcome.report, parts)
}

/// A submission around a report made elsewhere (by `onus report` in CI).
pub fn submission_with_report(
    repo: &Path,
    base_sha: &str,
    head_sha: &str,
    report: SemanticReport,
    parts: SubmissionParts,
) -> Result<Submission> {
    let changed_files = git_lines(
        repo,
        &["diff", "--name-only", "--no-renames", base_sha, head_sha],
    )?;
    Ok(Submission {
        schema: onus_lanes::submission::SCHEMA,
        base: base_sha.to_string(),
        head: head_sha.to_string(),
        intent: parts.intent,
        report,
        changed_files,
        evidence: parts.evidence,
        scope: parts.scope,
        escalations: parts.escalations,
        approvals: parts.approvals,
        agent: parts.agent,
    })
}

/// `<row id>=<who>` as an approval.
pub fn parse_approval(a: &str) -> Result<Approval> {
    let (row, by) = a
        .split_once('=')
        .with_context(|| format!("`{a}`: write approvals as <row id>=<who>"))?;
    Ok(Approval {
        row: row.to_string(),
        by: by.to_string(),
    })
}

pub fn submit(args: SubmitArgs) -> Result<i32> {
    let intent = match &args.intent {
        Some(p) => Some(std::fs::read_to_string(p)?),
        None => None,
    };
    let intent_markdown = args.intent.as_ref().is_some_and(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "txt"))
    });
    let evidence = args
        .evidence
        .iter()
        .map(|p| {
            serde_json::from_str(&std::fs::read_to_string(p)?)
                .with_context(|| format!("{} is not a test run from `onus run-test`", p.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    let (scope, claim) = match (&args.token, &args.key_public) {
        (Some(t), Some(k)) => {
            let (scope, claim) = scope_and_agent(t, k)?;
            (Some(scope), claim)
        }
        _ => (None, None),
    };
    let escalations = if args.escalations.is_empty() {
        Vec::new()
    } else {
        let key = args
            .key_public
            .as_deref()
            .context("escalation ids are verified with the root public key: pass --key-public")?;
        let dir = args
            .escalations_dir
            .clone()
            .unwrap_or_else(|| args.repo.join(".onus").join("escalations"));
        crate::doors::verify_escalations(
            &dir,
            &args.escalations,
            key,
            scope.as_ref().map(|s| s.task.as_str()),
        )?
    };
    // The token's agent, when it names one, over the flags.
    let agent = match claim {
        Some(a) => AgentSetup {
            tool: a.tool,
            model: a.model,
            config: a.config,
            team: a.team,
        },
        None => AgentSetup {
            tool: args.agent.agent_tool,
            model: args.agent.agent_model,
            config: args.agent.agent_config,
            team: args.agent.agent_team,
        },
    };
    let approvals = args
        .approvals
        .iter()
        .map(|a| parse_approval(a))
        .collect::<Result<Vec<_>>>()?;
    let sub = make_submission(
        &args.repo,
        &args.base,
        &args.head,
        SubmissionParts {
            intent,
            intent_markdown,
            evidence,
            scope,
            escalations,
            approvals,
            agent,
        },
    )?;
    let json = serde_json::to_string_pretty(&sub)?;
    match args.out {
        Some(p) => std::fs::write(p, format!("{json}\n"))?,
        None => println!("{json}"),
    }
    Ok(0)
}

pub fn lanes_config(repo: &Path, explicit: Option<&Path>) -> Result<LanesConfig> {
    let loaded = match explicit {
        Some(p) => Some(onus_map::config::load(p)?),
        None => onus_map::config::load_from_tree(repo)?,
    };
    Ok(loaded.and_then(|l| l.config.lanes).unwrap_or_default())
}

/// The submission or report, the policy, and the classification.
fn classify_args(args: &LaneArgs) -> Result<(Option<Submission>, LanesConfig, Classification)> {
    let config = lanes_config(&args.repo, args.config.as_deref())?;
    let (submission, report): (Option<Submission>, SemanticReport) =
        match (&args.submission, &args.report) {
            (Some(p), _) => {
                let s: Submission = serde_json::from_str(&std::fs::read_to_string(p)?)
                    .with_context(|| format!("{} is not a submission", p.display()))?;
                let r = s.report.clone();
                (Some(s), r)
            }
            (None, Some(p)) => (
                None,
                serde_json::from_str(&std::fs::read_to_string(p)?)
                    .with_context(|| format!("{} is not a report", p.display()))?,
            ),
            (None, None) => bail!("give --submission or --report"),
        };
    let c = classify_with(
        &args.repo,
        &report,
        submission.as_ref(),
        &config,
        &ClassifyOpts {
            outcomes: args.outcomes.as_deref(),
            change: args.change.as_deref(),
            model_file: args.model.as_deref(),
            ..Default::default()
        },
    )?;
    Ok((submission, config, c))
}

/// Where a classification looks for history and its learned model. Later
/// inputs add fields here.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClassifyOpts<'a> {
    /// Outcome records: the agent setup's record and per-component history.
    pub outcomes: Option<&'a Path>,
    /// The change, as owner/repo#123: history counts that repository's
    /// changes only, and leaves this one out.
    pub change: Option<&'a str>,
    /// The end of the history window, in Unix seconds (None = now).
    pub now: Option<u64>,
    /// The commit whose model file applies; default the submission's base,
    /// else the working tree.
    pub model_at: Option<&'a str>,
    /// A model file (`--model`), over both.
    pub model_file: Option<&'a Path>,
}

/// What `lanes.learned` found.
#[derive(Debug)]
pub enum ModelLoad {
    /// `lanes.learned` is not set.
    Off,
    Missing(String),
    Stale(String),
    Invalid(String),
    /// The model and its id.
    Loaded(Box<Model>, String),
}

/// The learned model a classification uses, read from `--model`, else from
/// the base commit (`opts.model_at`, else the submission's base), else from
/// the working tree when there is no commit. Reading it from the base means
/// a change cannot delete or retrain the model to avoid a raise. A model
/// that is missing, stale or does not parse is never an error: the lane
/// then comes from rules only.
pub fn load_model(
    repo: &Path,
    config: &LearnedConfig,
    opts: &ClassifyOpts,
    submission: Option<&Submission>,
) -> ModelLoad {
    let only_rules = "the lane comes from rules only";
    let (bytes, whence) = if let Some(f) = opts.model_file {
        match std::fs::read(f) {
            Ok(b) => (b, format!("`{}`", f.display())),
            Err(_) => {
                return ModelLoad::Missing(format!("no model at `{}`; {only_rules}", f.display()));
            }
        }
    } else if let Some(at) = opts.model_at.or(submission.map(|s| s.base.as_str())) {
        let short = &at[..at.len().min(12)];
        match git_bytes(repo, &["show", &format!("{at}:{}", config.model)]) {
            Ok(b) => (b, format!("`{}` at {short}", config.model)),
            Err(_) => {
                return ModelLoad::Missing(format!(
                    "no model at `{}` in the base commit {short}; {only_rules}",
                    config.model
                ));
            }
        }
    } else {
        match std::fs::read(repo.join(&config.model)) {
            Ok(b) => (b, format!("`{}`", config.model)),
            Err(_) => {
                return ModelLoad::Missing(format!("no model at `{}`; {only_rules}", config.model));
            }
        }
    };
    let model: Model = match serde_json::from_slice(&bytes) {
        Ok(m) => m,
        Err(e) => {
            return ModelLoad::Invalid(format!(
                "{whence} is not a lanes model ({e}); {only_rules}"
            ));
        }
    };
    if let Err(why) = model.usable() {
        return ModelLoad::Stale(format!(
            "{why}; retrain it with `onus lanes train`; {only_rules}"
        ));
    }
    ModelLoad::Loaded(Box::new(model), learned::model_id(&bytes))
}

/// The history of `areas` in the window ending at `now`: reverts, rollbacks
/// and incidents from the outcome records, and the production signals kept
/// beside them (`signals.jsonl` in the same folder).
pub fn area_histories(
    all: &[Outcome],
    signals_file: Option<&Path>,
    areas: &std::collections::BTreeSet<String>,
    scope: &HistoryScope,
) -> Result<AreaHistories> {
    let mut history = outcomes::area_history(all, areas, scope);
    if let Some(f) = signals_file {
        signals::extend_history(&mut history, &signals::load(f)?, areas, scope);
    }
    Ok(history)
}

/// The signals file beside an outcomes file.
fn signals_beside(outcomes_file: &Path) -> PathBuf {
    outcomes_file
        .parent()
        .unwrap_or(Path::new("."))
        .join(signals::FILE)
}

/// Every component on record in a records folder (outcomes and signals)
/// with its history in the window: what `onus signals areas` and `onus ui`
/// show.
pub fn areas_for(records: &Path, scope: &HistoryScope) -> Result<AreaHistories> {
    let all = outcomes::load(&records.join("outcomes.jsonl"))?;
    let sigs = signals::load(&records.join(signals::FILE))?;
    Ok(areas_from(&all, &sigs, scope))
}

/// [`areas_for`], from records already read.
pub fn areas_from(
    all: &[Outcome],
    sigs: &[onus_core::signal::ProductionSignal],
    scope: &HistoryScope,
) -> AreaHistories {
    let component = |i: &String| onus_core::ids::component_of(i).unwrap_or(i).to_string();
    let wanted: std::collections::BTreeSet<String> = all
        .iter()
        .flat_map(|o| {
            o.components
                .iter()
                .cloned()
                .chain(o.involved.iter().map(component))
        })
        .chain(sigs.iter().flat_map(|s| {
            s.components
                .iter()
                .cloned()
                .chain(s.symbols.iter().map(component))
        }))
        .collect();
    let mut history = outcomes::area_history(all, &wanted, scope);
    signals::extend_history(&mut history, sigs, &wanted, scope);
    history
}

/// The window and repository the history input reads for a change
/// classified at `now`.
pub fn history_scope<'a>(
    config: &LanesConfig,
    opts: &ClassifyOpts<'a>,
    now: u64,
) -> HistoryScope<'a> {
    HistoryScope {
        since: now.saturating_sub(u64::from(config.inputs.history.days) * 86_400),
        until: Some(now),
        repo: opts.change.and_then(outcomes::repo_of),
        exclude: opts.change,
    }
}

/// The rollout plan of a change in `lane`: the history of the components it
/// touches, as [`classify_with`] reads it, tightens it. For a lane the judge
/// changed after classifying.
pub fn rollout_plan(
    lane: Lane,
    report: &SemanticReport,
    config: &LanesConfig,
    opts: &ClassifyOpts,
) -> Result<rollout::RolloutPlan> {
    let all = match opts.outcomes {
        Some(f) if f.exists() => outcomes::load(f)?,
        _ => Vec::new(),
    };
    let areas: std::collections::BTreeSet<String> = report
        .changes
        .iter()
        .filter_map(area)
        .map(str::to_string)
        .collect();
    let now = opts.now.unwrap_or_else(onus_doors::gateway::now);
    let history = area_histories(
        &all,
        opts.outcomes.map(signals_beside).as_deref(),
        &areas,
        &history_scope(config, opts, now),
    )?;
    Ok(rollout::plan(
        lane,
        report,
        opts.change,
        config.rollout.as_ref(),
        &history,
        config.inputs.history.days,
    ))
}

/// The lane of a change: its report, its submission if any, the policy,
/// the agent setup's record, the history of the components it touches and
/// the learned model, with the rollout plan for that lane. `repo` is the
/// worktree the policy came from.
pub fn classify_with(
    repo: &Path,
    report: &SemanticReport,
    submission: Option<&Submission>,
    config: &LanesConfig,
    opts: &ClassifyOpts,
) -> Result<Classification> {
    // A file not written yet is an empty record.
    let all = match opts.outcomes {
        Some(f) if f.exists() => outcomes::load(f)?,
        _ => Vec::new(),
    };
    let agent = submission.map(|s| s.agent.key());
    let record = match (opts.outcomes, &agent) {
        (Some(_), Some(a)) => Some(outcomes::record_for(&all, a)),
        _ => None,
    };
    let areas: std::collections::BTreeSet<String> = report
        .changes
        .iter()
        .filter_map(area)
        .map(str::to_string)
        .collect();
    let now = opts.now.unwrap_or_else(onus_doors::gateway::now);
    let repo_name = opts.change.and_then(outcomes::repo_of);
    // Production signals kept beside the outcomes count too.
    let history = area_histories(
        &all,
        opts.outcomes.map(signals_beside).as_deref(),
        &areas,
        &history_scope(config, opts, now),
    )?;
    // What the change looks like now, for the model and for training later.
    let features = learned::features(
        report,
        submission,
        &learned::History {
            outcomes: &all,
            agent: agent.as_deref(),
            record,
            repo: repo_name,
            change: opts.change,
            until: Some(now),
        },
    );
    let load = match &config.learned {
        Some(l) => load_model(repo, l, opts, submission),
        None => ModelLoad::Off,
    };
    // The scope the change was made under (verified when the submission was
    // made), checked against every path the change writes.
    let scope = submission.and_then(|s| s.scope.as_ref()).map(|sc| {
        onus_doors::scope::Scope::new(sc.rights.iter().filter_map(|r| r.parse().ok()).collect())
    });
    let writable = |p: &str| scope.as_ref().is_some_and(|s| s.can_write(p));
    let writable_ref: Option<&dyn Fn(&str) -> bool> =
        scope.as_ref().map(|_| &writable as &dyn Fn(&str) -> bool);
    let learned_in = match (&load, &config.learned) {
        (ModelLoad::Loaded(model, id), Some(l)) => Some(Learned {
            model: model.as_ref(),
            id,
            config: l,
            features: &features,
        }),
        _ => None,
    };
    let mut c = classify(
        report,
        submission,
        config,
        &ClassifyContext {
            record,
            areas: &history,
            writable: writable_ref,
            learned: learned_in,
        },
    );
    if let Some(l) = &config.learned {
        match load {
            ModelLoad::Missing(note) => c.learned = Some(LearnedRisk::ignored("missing", l, note)),
            ModelLoad::Stale(note) => c.learned = Some(LearnedRisk::ignored("stale", l, note)),
            ModelLoad::Invalid(note) => c.learned = Some(LearnedRisk::ignored("invalid", l, note)),
            ModelLoad::Off | ModelLoad::Loaded(..) => {}
        }
    }
    c.features = Some(features);
    c.rollout = Some(rollout::plan(
        c.lane,
        report,
        opts.change,
        config.rollout.as_ref(),
        &history,
        config.inputs.history.days,
    ));
    Ok(c)
}

/// The judge's verdict on a submission, with evidence re-run in containers
/// and checked against the repository's evidence store.
pub fn judge_submission(
    repo: &Path,
    sub: &Submission,
    classification: &Classification,
    config: &LanesConfig,
) -> onus_lanes::judge::Judgment {
    let run = |repo: &Path, commit: &str, image: &str, setup: Option<&str>, command: &str| {
        onus_doors::runner::run(repo, commit, image, setup, command)
    };
    let store = onus_env::store::Store::for_repo(repo).ok();
    judge_with(sub, classification, config, repo, &run, store.as_ref())
}

pub fn classify_cmd(args: LaneArgs) -> Result<i32> {
    let (_, _, c) = classify_args(&args)?;
    println!("{}", serde_json::to_string_pretty(&c)?);
    Ok(0)
}

pub fn judge_cmd(args: LaneArgs) -> Result<i32> {
    let (submission, config, c) = classify_args(&args)?;
    let Some(sub) = submission else {
        bail!("the judge needs a submission (onus submit)");
    };
    let j = judge_submission(&args.repo, &sub, &c, &config);
    // The plan for the final lane: the judge may have moved it.
    let rollout = if j.lane == c.lane {
        c.rollout.clone()
    } else {
        Some(rollout_plan(
            j.lane,
            &sub.report,
            &config,
            &ClassifyOpts {
                outcomes: args.outcomes.as_deref(),
                change: args.change.as_deref(),
                ..Default::default()
            },
        )?)
    };
    let out = serde_json::json!({ "classification": c, "judgment": j, "rollout": rollout });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(match j.verdict {
        Verdict::Approve => 0,
        Verdict::Reject => 2,
        Verdict::Escalate => 3,
    })
}

pub const RESULTS: &[&str] = &[
    "merged",
    "reverted",
    "rolled-back",
    "incident",
    "closed",
    "open",
];

/// Appends an outcome after checking its result.
pub fn record_outcome(file: &Path, outcome: &Outcome) -> Result<()> {
    if !RESULTS.contains(&outcome.result.as_str()) {
        bail!(
            "the result is merged, reverted, rolled-back (a flag or rollout), incident, closed or open"
        );
    }
    if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    outcomes::append(file, outcome)
}

/// Records reverts found in git history; returns how many were found and
/// how many were recorded.
pub fn ingest_reverts(file: &Path, repo: &Path, since: Option<&str>) -> Result<(usize, usize)> {
    let all = if file.exists() {
        outcomes::load(file)?
    } else {
        Vec::new()
    };
    let reverts = outcomes::find_reverts(repo, since)?;
    let added = outcomes::ingest_reverts(&all, &reverts, onus_doors::gateway::now());
    for o in &added {
        outcomes::append(file, o)?;
    }
    Ok((reverts.len(), added.len()))
}

/// Records an incident against a change on record.
pub fn record_incident(
    file: &Path,
    change: &str,
    involved: Vec<String>,
    note: String,
) -> Result<()> {
    let all = outcomes::load(file)?;
    let Some(last) = outcomes::latest(&all)
        .into_iter()
        .find(|o| o.change == change)
        .cloned()
    else {
        bail!("`{change}` is not on record in {}", file.display());
    };
    outcomes::append(
        file,
        &Outcome {
            at: onus_doors::gateway::now(),
            result: "incident".into(),
            involved,
            note: Some(note),
            signal: None,
            planned: false,
            ..last
        },
    )
}

pub fn outcomes_cmd(cmd: OutcomesCmd) -> Result<i32> {
    match cmd {
        OutcomesCmd::Record {
            file,
            change,
            agent,
            lane,
            result,
            verdict,
            judge,
            audited,
            missed,
            commit,
            components,
            classification,
            planned,
        } => {
            if planned && result != "rolled-back" {
                bail!("--planned is for --result rolled-back");
            }
            let features = match &classification {
                Some(p) => {
                    let c: Classification = serde_json::from_str(&std::fs::read_to_string(p)?)
                        .with_context(|| format!("{} is not a classification", p.display()))?;
                    if c.features.is_none() {
                        bail!(
                            "{} has no features: classify the change with this onus first",
                            p.display()
                        );
                    }
                    c.features
                }
                None => None,
            };
            record_outcome(
                &file,
                &Outcome {
                    at: onus_doors::gateway::now(),
                    change,
                    agent,
                    judge,
                    lane: lane.into(),
                    verdict,
                    result,
                    audited,
                    missed,
                    commit,
                    involved: vec![],
                    note: None,
                    components,
                    reviewed_by: vec![],
                    sampled: false,
                    audit: None,
                    merged_by: None,
                    features,
                    caught: false,
                    signal: None,
                    planned,
                },
            )?;
            Ok(0)
        }
        OutcomesCmd::IngestReverts { file, repo, since } => {
            let (found, added) = ingest_reverts(&file, &repo, since.as_deref())?;
            println!("{found} reverts found, {added} recorded against changes on record");
            Ok(0)
        }
        OutcomesCmd::Incident {
            file,
            change,
            involved,
            note,
        } => {
            record_incident(&file, &change, involved, note)?;
            Ok(0)
        }
        OutcomesCmd::Backlog { file } => {
            for (what, n) in outcomes::backlog(&outcomes::load(&file)?) {
                println!("{n}\t{what}");
            }
            Ok(0)
        }
        OutcomesCmd::Summary { file } => {
            let s = outcomes::summarize(&outcomes::load(&file)?);
            println!("{}", serde_json::to_string_pretty(&s)?);
            Ok(0)
        }
        OutcomesCmd::Audit {
            file,
            change,
            by,
            result,
            note,
            at,
        } => {
            let missed = result == AuditResultArg::Miss;
            let note = note.as_deref().map(str::trim).filter(|n| !n.is_empty());
            let at = at.unwrap_or_else(onus_doors::gateway::now);
            let audit =
                outcomes::audit_record(&outcomes::load(&file)?, &change, &by, missed, note, at)?;
            record_outcome(&file, &audit)?;
            println!(
                "{}",
                serde_json::json!({
                    "change": change,
                    "missed": missed,
                    "sampled": audit.sampled,
                    "reply": format!("Audit recorded{} for {by}.", if missed { " as a miss" } else { "" }),
                })
            );
            Ok(0)
        }
        OutcomesCmd::Audits { file, limit } => {
            let q = outcomes::audits(&outcomes::load(&file)?, limit);
            println!("{}", serde_json::to_string_pretty(&q)?);
            Ok(0)
        }
    }
}

pub fn lanes_cmd(cmd: LanesCmd) -> Result<i32> {
    match cmd {
        LanesCmd::Train {
            outcomes: file,
            out,
            repo,
            config,
            settle_days,
            min_examples,
            min_positives,
            dry_run,
        } => {
            // With both --config and --out, nothing is read from --repo, so a
            // hosted service can train outside a repository.
            let learned = lanes_config(&repo, config.as_deref())?
                .learned
                .unwrap_or_default();
            let out = out.unwrap_or_else(|| {
                if repo == Path::new(".") {
                    PathBuf::from(&learned.model)
                } else {
                    repo.join(&learned.model)
                }
            });
            if !file.exists() {
                bail!("{} does not exist", file.display());
            }
            let all = outcomes::load(&file)?;
            let (examples, skipped) = learned::examples(&all, u64::from(settle_days) * 86_400);
            let model = match learned::train(
                &examples,
                skipped,
                &learned::TrainOptions {
                    settle_days,
                    min_examples,
                    min_positives,
                    judge_at: learned.judge,
                    human_at: learned.human,
                },
            ) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("onus: {e:#}");
                    return Ok(1);
                }
            };
            let text = format!("{}\n", serde_json::to_string_pretty(&model)?);
            if !dry_run {
                if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&out, &text)
                    .with_context(|| format!("cannot write {}", out.display()))?;
            }
            let mut weights: Vec<(&String, f64)> = model
                .features
                .iter()
                .zip(model.weights.iter().copied())
                .collect();
            weights.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()).then(a.0.cmp(b.0)));
            let summary = serde_json::json!({
                "out": out.display().to_string(),
                "written": !dry_run,
                "model": learned::model_id(text.as_bytes()),
                "examples": model.training.examples,
                "positives": model.training.positives,
                "skipped": model.training.skipped,
                "holdout": model.holdout,
                "weights": weights
                    .iter()
                    .map(|(f, w)| serde_json::json!({ "feature": f, "weight": w }))
                    .collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&summary)?);
            Ok(0)
        }
    }
}
