//! Phase 4 commands: submissions, lanes, the judge and outcome records
//! (ADR 0009).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use onus_core::{Lane, LanesConfig, SemanticReport};
use onus_lanes::classify::{Classification, classify};
use onus_lanes::judge::{Verdict, judge_with};
use onus_lanes::outcomes::{self, Outcome};
use onus_lanes::submission::{AgentSetup, Approval, ScopeUsed, Submission};

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
    /// An escalation granted during the task (repeatable).
    #[arg(long = "escalation", value_name = "ID")]
    escalations: Vec<String>,
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
    /// Outcome records, for the agent setup's track record.
    #[arg(long, value_name = "FILE")]
    outcomes: Option<PathBuf>,
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
        /// merged, reverted, incident, closed or open.
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

fn git_lines(repo: &Path, args: &[&str]) -> Result<Vec<String>> {
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
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

pub fn submit(args: SubmitArgs) -> Result<i32> {
    let intent_text = match &args.intent {
        Some(p) => Some(std::fs::read_to_string(p)?),
        None => None,
    };
    let intent = args
        .intent
        .as_ref()
        .map(|p| onus_cli::read_intent(p))
        .transpose()?
        .flatten();
    let pair = std::sync::Arc::new(onus_cli::materialize_pair(
        &args.repo, &args.base, &args.head, false,
    )?);
    let opts = onus_cli::DiffOptions {
        intent,
        base_label: onus_cli::ref_label(&args.base, &pair.base.sha),
        head_label: onus_cli::ref_label(&args.head, &pair.head.sha),
        base_commit: Some(pair.base.sha.clone()),
        head_commit: Some(pair.head.sha.clone()),
        changed_paths: Some(pair.changed.clone()),
        ..Default::default()
    };
    let outcome = onus_cli::diff_dirs(pair.base.dir.path(), pair.head.dir.path(), &opts)?;
    let changed_files = git_lines(
        &args.repo,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            &pair.base.sha,
            &pair.head.sha,
        ],
    )?;
    let evidence = args
        .evidence
        .iter()
        .map(|p| {
            serde_json::from_str(&std::fs::read_to_string(p)?)
                .with_context(|| format!("{} is not a test run from `onus run-test`", p.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    let scope = match (&args.token, &args.key_public) {
        (Some(t), Some(k)) => {
            let v = onus_doors::token::verify(
                t,
                &onus_doors::token::public_key(&std::fs::read_to_string(k)?)?,
            )?;
            Some(ScopeUsed {
                task: v.task.clone(),
                rights: v.rights.iter().map(|r| r.to_string()).collect(),
                attenuations: v.attenuations.clone(),
            })
        }
        _ => None,
    };
    let approvals = args
        .approvals
        .iter()
        .map(|a| {
            let (row, by) = a
                .split_once('=')
                .with_context(|| format!("`{a}`: write approvals as <row id>=<who>"))?;
            Ok(Approval {
                row: row.to_string(),
                by: by.to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let sub = Submission {
        schema: onus_lanes::submission::SCHEMA,
        base: pair.base.sha.clone(),
        head: pair.head.sha.clone(),
        intent: intent_text,
        report: outcome.report,
        changed_files,
        evidence,
        scope,
        escalations: args.escalations,
        approvals,
        agent: AgentSetup {
            tool: args.agent.agent_tool,
            model: args.agent.agent_model,
            config: args.agent.agent_config,
            team: args.agent.agent_team,
        },
    };
    let json = serde_json::to_string_pretty(&sub)?;
    match args.out {
        Some(p) => std::fs::write(p, format!("{json}\n"))?,
        None => println!("{json}"),
    }
    Ok(0)
}

fn lanes_config(repo: &Path, explicit: Option<&Path>) -> Result<LanesConfig> {
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
    let record = match (&args.outcomes, &submission) {
        (Some(f), Some(s)) => Some(outcomes::record_for(&outcomes::load(f)?, &s.agent.key())),
        _ => None,
    };
    // The scope the change was made under (verified when the submission was
    // made), checked against every path the change writes.
    let scope = submission
        .as_ref()
        .and_then(|s| s.scope.as_ref())
        .map(|sc| {
            onus_doors::scope::Scope::new(sc.rights.iter().filter_map(|r| r.parse().ok()).collect())
        });
    let writable = |p: &str| scope.as_ref().is_some_and(|s| s.can_write(p));
    let writable_ref: Option<&dyn Fn(&str) -> bool> =
        scope.as_ref().map(|_| &writable as &dyn Fn(&str) -> bool);
    let c = classify(&report, submission.as_ref(), &config, record, writable_ref);
    Ok((submission, config, c))
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
    let run = |repo: &Path, commit: &str, image: &str, setup: Option<&str>, command: &str| {
        onus_doors::runner::run(repo, commit, image, setup, command)
    };
    // Runs recorded in an environment are checked against their manifests.
    let store = onus_env::store::Store::for_repo(&args.repo).ok();
    let j = judge_with(&sub, &c, &config, &args.repo, &run, store.as_ref());
    let out = serde_json::json!({ "classification": c, "judgment": j });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(match j.verdict {
        Verdict::Approve => 0,
        Verdict::Reject => 2,
        Verdict::Escalate => 3,
    })
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
        } => {
            if !matches!(
                result.as_str(),
                "merged" | "reverted" | "incident" | "closed" | "open"
            ) {
                bail!("--result is merged, reverted, incident, closed or open");
            }
            outcomes::append(
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
                },
            )?;
            Ok(0)
        }
        OutcomesCmd::IngestReverts { file, repo, since } => {
            let all = outcomes::load(&file)?;
            let reverts = outcomes::find_reverts(&repo, since.as_deref())?;
            let added = outcomes::ingest_reverts(&all, &reverts, onus_doors::gateway::now());
            for o in &added {
                outcomes::append(&file, o)?;
            }
            println!(
                "{} reverts found, {} recorded against changes on record",
                reverts.len(),
                added.len()
            );
            Ok(0)
        }
        OutcomesCmd::Incident {
            file,
            change,
            involved,
            note,
        } => {
            let all = outcomes::load(&file)?;
            let Some(last) = outcomes::latest(&all)
                .into_iter()
                .find(|o| o.change == change)
                .cloned()
            else {
                bail!("`{change}` is not on record in {}", file.display());
            };
            outcomes::append(
                &file,
                &Outcome {
                    at: onus_doors::gateway::now(),
                    result: "incident".into(),
                    involved,
                    note: Some(note),
                    ..last
                },
            )?;
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
    }
}
