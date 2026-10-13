//! `onus ci`: what the GitHub Action runs at each event, so outcomes are
//! recorded without anyone typing them (`onus help ci`).
//!
//! - A pull request is pushed: `onus ci pr` makes its submission (the pull
//!   request body as intent, the agent from git and GitHub), classifies it
//!   with the agent's track record, runs the judge when asked, and keeps a
//!   pull request record and its provenance.
//! - It closes: `onus ci closed` turns its last record into an outcome,
//!   merged or closed.
//! - Someone comments `/onus approve`, `/onus audit` or `/onus incident`:
//!   `onus ci comment` records it, for people with write access only. It
//!   also answers `/onus caught` and `/onus accuracy` with what was said,
//!   for a hosted service to keep. `/onus caught` from someone with write
//!   access is recorded too, for the learned lane model; accuracy is not.
//! - The default branch moves: `onus ci push` records reverts.
//! - A deploy tool reports a rollout stage with its metric samples: `onus ci
//!   rollout` decides promote, hold, rollback or complete against the
//!   change's rollout plan, and records the stage. Onus never moves traffic;
//!   the tool acts on the exit code.
//! - A rollback decided elsewhere: `onus ci rollback` records it.
//!
//! Records live in a folder that `onus ci records pull|push` syncs with a
//! branch of the repository: pull requests, outcomes and production signals.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use onus_core::provenance::IntentSource;
use onus_core::signal::{ProductionSignal, RolloutMetrics, RolloutStage, SignalKind};
use onus_core::{Lane, SemanticReport};
use onus_core::{MetricGuard, RolloutStrategy};
use onus_lanes::outcomes::{self, Outcome};
use onus_lanes::provenance;
use onus_lanes::pulls::{self, PullRecord};
use onus_lanes::review::{self, Review, ReviewState};
use onus_lanes::rollout::{self, Decision, RolloutPlan};
use onus_lanes::signals;
use onus_lanes::submission::Approval;
use onus_map::discover::{CODEOWNERS_PATHS, Codeowners};
use serde_json::json;

use crate::{lanes, records};

// Parsed once per run, so the size of `Pr` does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum CiCmd {
    /// A pull request was opened or pushed: submit, classify, judge, record.
    Pr {
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        #[arg(long, value_name = "REF")]
        base: String,
        #[arg(long, value_name = "REF")]
        head: String,
        /// The change, as `owner/repo#123`.
        #[arg(long)]
        change: String,
        /// The pull request's branch.
        #[arg(long, default_value = "")]
        branch: String,
        /// Who opened it (a GitHub login).
        #[arg(long, default_value = "")]
        author: String,
        /// The pull request body: the intent, in an onus-intent block.
        #[arg(long, value_name = "FILE")]
        body_file: Option<PathBuf>,
        /// A JSON report already made for this change (onus report --format json).
        #[arg(long, value_name = "FILE")]
        report: Option<PathBuf>,
        /// Test runs to submit as evidence (from onus run-test or onus env run).
        #[arg(long = "evidence", value_name = "FILE")]
        evidence: Vec<PathBuf>,
        /// The records folder (see `onus ci records`).
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// Run the judge when the lane is `judge` (needs Docker or Podman).
        #[arg(long)]
        judge: bool,
        /// A held-out suite for the judge: a directory outside the repository holding
        /// held-out.yaml and its tests. What it did goes to held-out.json in --out.
        #[arg(long, env = "ONUS_HELD_OUT", value_name = "DIR")]
        held_out: Option<PathBuf>,
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// GitHub reviews of the pull request (JSON array; see `onus help ci`).
        #[arg(long, value_name = "FILE")]
        reviews: Option<PathBuf>,
        /// A learned lane model to use instead of lanes.learned.model at the base commit
        /// (Onus Cloud trains one per organisation). Needs lanes.learned in the config.
        #[arg(long, value_name = "FILE")]
        model: Option<PathBuf>,
        /// Where to write the submission, classification, judgment, held-out report, provenance
        /// and comment section.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// When it happened, in seconds since 1970; now when left out. For records
        /// written after the fact, such as a hosted service catching up on history.
        #[arg(long, value_name = "SECONDS")]
        at: Option<u64>,
    },
    /// A pull request was closed: its last record becomes an outcome.
    Closed {
        #[arg(long)]
        change: String,
        /// Whether it was merged.
        #[arg(long)]
        merged: bool,
        /// The commit it landed as, to match reverts against.
        #[arg(long)]
        commit: Option<String>,
        /// Who merged it (a GitHub login), kept on the outcome for its provenance.
        #[arg(long, value_name = "LOGIN")]
        merged_by: Option<String>,
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// When it happened, in seconds since 1970; now when left out. For records
        /// written after the fact, such as a hosted service catching up on history.
        #[arg(long, value_name = "SECONDS")]
        at: Option<u64>,
    },
    /// A comment on a pull request: `/onus approve <row>`, `/onus audit ok|miss`,
    /// `/onus incident <what happened>`, `/onus caught <what>`, `/onus accuracy
    /// ok|wrong <row>|missed <what>`. Prints what to reply.
    Comment {
        #[arg(long)]
        change: String,
        #[arg(long, value_name = "FILE")]
        body_file: PathBuf,
        /// The commenter's GitHub login.
        #[arg(long)]
        author: String,
        /// The commenter's association (OWNER, MEMBER, COLLABORATOR, …); only these three may record.
        #[arg(long)]
        association: String,
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// The report the comment is about (onus report --format json). `/onus accuracy wrong <row>`
        /// is checked against its rows, and a row number is turned into the row's id.
        #[arg(long, value_name = "FILE")]
        report: Option<PathBuf>,
    },
    /// The default branch moved: record reverts of merged changes.
    Push {
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// Only commits after this one.
        #[arg(long, value_name = "REF")]
        since: Option<String>,
    },
    /// A deploy tool reports a rollout stage: Onus decides promote, hold,
    /// rollback or complete from the change's plan and the metric samples. Exit
    /// 0 promote or complete, 2 rollback, 3 hold. Without --percent, prints the plan.
    #[command(group = clap::ArgGroup::new("which").required(true).args(["change", "commit"]))]
    Rollout {
        /// The change, as `owner/repo#123`.
        #[arg(long)]
        change: Option<String>,
        /// Or the commit deployed (a sha, or at least 7 hex digits of one).
        #[arg(long)]
        commit: Option<String>,
        /// The share of production traffic the stage runs at.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: Option<u8>,
        /// The stage's metric samples (schemas/rollout-metrics.schema.json).
        #[arg(long, value_name = "FILE")]
        metrics: Option<PathBuf>,
        /// The deploy tool's id for this deploy, kept with the stage.
        #[arg(long)]
        deploy: Option<String>,
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// The repository whose onus.yaml sets the plan of a change recorded before
        /// rollout plans (lanes.rollout).
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        /// The onus.yaml to read instead of the one in --repo.
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// When, in seconds since 1970; now when left out.
        #[arg(long, value_name = "SECONDS")]
        at: Option<u64>,
    },
    /// A change was rolled back, by a person or the deploy tool's own analysis.
    #[command(group = clap::ArgGroup::new("which").required(true).args(["change", "commit"]))]
    Rollback {
        /// The change, as `owner/repo#123`.
        #[arg(long)]
        change: Option<String>,
        /// Or the commit rolled back.
        #[arg(long)]
        commit: Option<String>,
        /// What happened.
        #[arg(long)]
        reason: String,
        /// Rolled back on purpose (a flag turned off as planned): it counts
        /// against neither the agent setup nor the components.
        #[arg(long)]
        planned: bool,
        /// Components and symbols involved (repeatable).
        #[arg(long = "involved", value_name = "ID")]
        involved: Vec<String>,
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
        /// When, in seconds since 1970; now when left out.
        #[arg(long, value_name = "SECONDS")]
        at: Option<u64>,
    },
    /// Sync the records folder with a branch of the repository.
    Records {
        #[command(subcommand)]
        cmd: RecordsCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum RecordsCmd {
    /// Copy the branch's records into a folder.
    Pull {
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[arg(long, default_value = "onus/records")]
        branch: String,
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
    },
    /// Commit the folder's records to the branch and push, merging with others.
    Push {
        #[arg(long, default_value = ".", value_name = "DIR")]
        repo: PathBuf,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[arg(long, default_value = "onus/records")]
        branch: String,
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
        #[arg(long, default_value = "Onus records")]
        message: String,
    },
}

fn now() -> u64 {
    onus_doors::gateway::now()
}

fn outcomes_file(records: &Path) -> PathBuf {
    records.join("outcomes.jsonl")
}

fn pulls_file(records: &Path) -> PathBuf {
    records.join("pulls.jsonl")
}

fn load_outcomes(records: &Path) -> Result<Vec<Outcome>> {
    let f = outcomes_file(records);
    if f.exists() {
        outcomes::load(&f)
    } else {
        Ok(Vec::new())
    }
}

fn lane_label(l: Lane) -> &'static str {
    match l {
        Lane::AutoMerge => "auto-merge",
        Lane::Judge => "the judge",
        Lane::Human => "a person",
        Lane::Blocked => "blocked",
    }
}

pub fn run(cmd: CiCmd) -> Result<i32> {
    match cmd {
        CiCmd::Pr {
            repo,
            base,
            head,
            change,
            branch,
            author,
            body_file,
            report,
            evidence,
            records,
            judge,
            held_out,
            config,
            reviews,
            model,
            out,
            at,
        } => pr(PrArgs {
            repo,
            base,
            head,
            change,
            branch,
            author,
            body_file,
            report,
            evidence,
            records,
            judge,
            held_out,
            config,
            reviews,
            model,
            out,
            at,
        }),
        CiCmd::Closed {
            change,
            merged,
            commit,
            merged_by,
            records,
            at,
        } => {
            // Only a merge has someone who merged it.
            let merged_by = merged_by
                .filter(|_| merged)
                .map(|l| l.trim().trim_start_matches('@').to_string())
                .filter(|l| !l.is_empty())
                .map(|l| format!("@{l}"));
            closed(
                &change,
                merged,
                commit,
                merged_by,
                &records,
                at.unwrap_or_else(now),
            )
        }
        CiCmd::Comment {
            change,
            body_file,
            author,
            association,
            records,
            report,
        } => {
            let body = std::fs::read_to_string(&body_file)?;
            let report: Option<SemanticReport> = match &report {
                Some(p) => Some(
                    serde_json::from_str(&std::fs::read_to_string(p)?)
                        .with_context(|| format!("{} is not a report", p.display()))?,
                ),
                None => None,
            };
            let answer = comment(
                &change,
                &body,
                &author,
                &association,
                &records,
                report.as_ref(),
            )?;
            println!("{}", serde_json::to_string_pretty(&answer)?);
            Ok(0)
        }
        CiCmd::Push {
            repo,
            records,
            since,
        } => {
            std::fs::create_dir_all(&records)?;
            let (found, added) =
                lanes::ingest_reverts(&outcomes_file(&records), &repo, since.as_deref())?;
            println!("{found} reverts found, {added} recorded");
            Ok(0)
        }
        CiCmd::Rollout {
            change,
            commit,
            percent,
            metrics,
            deploy,
            records,
            repo,
            config,
            at,
        } => rollout_stage(RolloutArgs {
            change,
            commit,
            percent,
            metrics,
            deploy,
            records,
            repo,
            config,
            at: at.unwrap_or_else(now),
        }),
        CiCmd::Rollback {
            change,
            commit,
            reason,
            planned,
            involved,
            records,
            at,
        } => rollback(
            change.as_deref(),
            commit.as_deref(),
            &reason,
            planned,
            involved,
            &records,
            at.unwrap_or_else(now),
        ),
        CiCmd::Records { cmd } => match cmd {
            RecordsCmd::Pull {
                repo,
                remote,
                branch,
                dir,
            } => {
                let existed = records::pull(&repo, &remote, &branch, &dir)?;
                eprintln!(
                    "onus: {} from {remote}/{branch}",
                    if existed {
                        "records pulled"
                    } else {
                        "no records yet; starting"
                    }
                );
                Ok(0)
            }
            RecordsCmd::Push {
                repo,
                remote,
                branch,
                dir,
                message,
            } => {
                let pushed = records::push(&repo, &remote, &branch, &dir, &message)?;
                eprintln!(
                    "onus: {}",
                    if pushed {
                        "records pushed"
                    } else {
                        "records unchanged"
                    }
                );
                Ok(0)
            }
        },
    }
}

struct PrArgs {
    repo: PathBuf,
    base: String,
    head: String,
    change: String,
    branch: String,
    author: String,
    body_file: Option<PathBuf>,
    report: Option<PathBuf>,
    evidence: Vec<PathBuf>,
    records: PathBuf,
    judge: bool,
    held_out: Option<PathBuf>,
    config: Option<PathBuf>,
    reviews: Option<PathBuf>,
    model: Option<PathBuf>,
    out: PathBuf,
    at: Option<u64>,
}

fn verdict_name(v: onus_lanes::judge::Verdict) -> &'static str {
    use onus_lanes::judge::Verdict;
    match v {
        Verdict::Approve => "approve",
        Verdict::Reject => "reject",
        Verdict::Escalate => "escalate",
    }
}

fn pr(a: PrArgs) -> Result<i32> {
    check_change(&a.change)?;
    // A bad suite fails the run before anything is written.
    let suite = lanes::load_held_out(a.held_out.as_deref(), &a.repo)?;
    std::fs::create_dir_all(&a.out)?;
    std::fs::create_dir_all(&a.records)?;
    let sha = |r: &str| -> Result<String> {
        Ok(lanes::git_lines(
            &a.repo,
            &["rev-parse", "--verify", &format!("{r}^{{commit}}")],
        )?
        .first()
        .cloned()
        .unwrap_or_default())
    };
    let (base, head) = (sha(&a.base)?, sha(&a.head)?);
    let message = lanes::git_lines(&a.repo, &["log", "-1", "--format=%B", &head])?.join("\n");
    let (agent, agent_source) = pulls::detect_agent_source(&message, &a.branch, &a.author);
    let body = match &a.body_file {
        Some(p) => std::fs::read_to_string(p).unwrap_or_default(),
        None => String::new(),
    };
    let intent = body.contains("```onus-intent").then_some(body);
    let evidence = a
        .evidence
        .iter()
        .map(|p| {
            serde_json::from_str(&std::fs::read_to_string(p)?)
                .with_context(|| format!("{} is not a test run", p.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    let reviews: Vec<Review> = match &a.reviews {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)
            .with_context(|| format!("{} is not a list of reviews", p.display()))?,
        None => Vec::new(),
    };
    let all_pulls = pulls::load(&pulls_file(&a.records))?;
    let previous = pulls::latest(&all_pulls, &a.change).cloned();
    let previous = previous.as_ref();
    let approvals = previous.map(|r| r.approvals.clone()).unwrap_or_default();
    let asked_before = previous.map(|r| r.requested.clone()).unwrap_or_default();
    let parts = lanes::SubmissionParts {
        intent_markdown: intent.is_some(),
        intent,
        evidence,
        approvals: approvals.clone(),
        agent: agent.clone(),
        ..Default::default()
    };
    let sub = match &a.report {
        Some(p) => {
            let report: SemanticReport = serde_json::from_str(&std::fs::read_to_string(p)?)
                .with_context(|| format!("{} is not a report", p.display()))?;
            lanes::submission_with_report(&a.repo, &base, &head, report, parts)?
        }
        None => lanes::make_submission(&a.repo, &base, &head, parts)?,
    };
    let config = lanes::lanes_config(&a.repo, a.config.as_deref())?;
    let at = a.at.unwrap_or_else(now);
    let c = lanes::classify_with(
        &a.repo,
        &sub.report,
        Some(&sub),
        &config,
        &lanes::ClassifyOpts {
            outcomes: Some(&outcomes_file(&a.records)),
            change: Some(&a.change),
            // A backfill sees the history as it was then.
            now: Some(at),
            // The model the base commit holds, so the change cannot alter it.
            model_at: Some(&base),
            model_file: a.model.as_deref(),
        },
    )?;
    let judged = (a.judge && c.lane == Lane::Judge)
        .then(|| {
            lanes::judge_submission(
                &a.repo,
                &sub,
                &c,
                &config,
                lanes::JudgeRun {
                    held_out: suite.as_ref(),
                    judge_config: None,
                    config_path: a.config.as_deref(),
                },
            )
        })
        .transpose()?;
    let (judgment, held_out) = match judged {
        Some(j) => (Some(j.judgment), j.held_out),
        None => (None, None),
    };
    // The plan for the final lane: the judge may have moved it.
    let plan = match (&judgment, &c.rollout) {
        (Some(j), _) if j.lane != c.lane => lanes::rollout_plan(
            j.lane,
            &sub.report,
            &config,
            &lanes::ClassifyOpts {
                outcomes: Some(&outcomes_file(&a.records)),
                change: Some(&a.change),
                now: Some(at),
                ..Default::default()
            },
        )?,
        (_, Some(p)) => p.clone(),
        (_, None) => rollout::plan(
            c.lane,
            &sub.report,
            Some(&a.change),
            config.rollout.as_ref(),
            &Default::default(),
            config.inputs.history.days,
        ),
    };
    std::fs::write(
        a.out.join("submission.json"),
        serde_json::to_vec_pretty(&sub)?,
    )?;
    std::fs::write(
        a.out.join("classification.json"),
        serde_json::to_vec_pretty(&c)?,
    )?;
    if let Some(j) = &judgment {
        std::fs::write(a.out.join("judgment.json"), serde_json::to_vec_pretty(j)?)?;
    }
    // For people only: it names the held-out checks.
    if let Some(h) = &held_out {
        std::fs::write(a.out.join("held-out.json"), serde_json::to_vec_pretty(h)?)?;
    }
    std::fs::write(
        a.out.join("rollout.json"),
        serde_json::to_vec_pretty(&plan)?,
    )?;
    let all_outcomes = load_outcomes(&a.records)?;
    let record = outcomes::record_for(&all_outcomes, &agent.key());
    let lane = judgment.as_ref().map_or(c.lane, |j| j.lane);
    // The areas the history input reads: code outside every component is
    // `root`.
    let mut components: Vec<String> = sub
        .report
        .changes
        .iter()
        .filter_map(onus_lanes::classify::area)
        .map(str::to_string)
        .collect();
    components.sort();
    components.dedup();
    let owners = review_owners(&a.repo, &head, &sub.report);
    let review = review::evaluate(lane, &head, &a.author, &reviews, owners, &asked_before);
    let mut requested = asked_before;
    requested.extend(review.request.iter().cloned());
    requested.sort();
    requested.dedup();
    pulls::append(
        &pulls_file(&a.records),
        &PullRecord {
            at,
            change: a.change.clone(),
            base,
            head,
            agent: agent.clone(),
            lane,
            verdict: judgment
                .as_ref()
                .map(|j| verdict_name(j.verdict).to_string()),
            // The configuration that decided, only when it did.
            judge: judgment.as_ref().map(|j| j.judge.clone()),
            approvals,
            components,
            requested: requested.clone(),
            reviewed_by: review.approved_by.clone(),
            // Recorded for `ci closed`; nothing shows it before the merge.
            audit_after_merge: Some(config.audit_after_merge),
            features: c.features.clone(),
            caught: previous.is_some_and(|r| r.caught),
            rollout: Some(plan.clone()),
        },
    )?;
    // The change's provenance while it is open: the facts from this run, with
    // the records as they now stand.
    let record_of = provenance::build(provenance::Inputs {
        submission: Some(&sub),
        intent_source: IntentSource::PullRequest,
        agent_source: Some(agent_source),
        classification: Some(&c),
        judgment: judgment.as_ref(),
        policy: Some(onus_lanes::judge::judge_id(&config)),
        pulls: &pulls::load(&pulls_file(&a.records))?,
        outcomes: &all_outcomes,
        signals: &signals::load(&a.records.join(signals::FILE))?,
        ..provenance::Inputs::new(&a.change, at)
    });
    std::fs::write(
        a.out.join("provenance.json"),
        serde_json::to_vec_pretty(&record_of)?,
    )?;

    // The section the comment shows under the report.
    let mut md = String::new();
    md.push_str(&format!(
        "\n### Lane: `{}` ({})\n\n",
        lane.as_str(),
        lane_label(lane)
    ));
    for ap in &c.applied {
        md.push_str(&format!(
            "- {} → `{}`: {}\n",
            ap.source,
            ap.lane.as_str(),
            ap.reason
        ));
    }
    if let Some(l) = &c.learned {
        md.push_str(&learned_markdown(l));
    }
    md.push_str(&format!(
        "\n**Agent:** `{}` · {} merged on record, {} incidents in its last {}{}{}\n",
        agent.key(),
        record.changes,
        record.recent_incidents,
        outcomes::RECENT,
        if record.recent_rollbacks > 0 {
            format!(" · {} recent rollbacks", record.recent_rollbacks)
        } else {
            String::new()
        },
        if record.changes < config.min_record {
            format!(" · auto-merge needs {} merged", config.min_record)
        } else {
            String::new()
        }
    ));
    md.push_str(&rollout_markdown(&plan));
    if let Some(j) = &judgment {
        md.push_str(&format!(
            "\n**Judge:** `{}`{}\n",
            verdict_name(j.verdict),
            j.rotation
                .as_ref()
                .map(|r| format!(" · configuration `{}`", r.config))
                .unwrap_or_default()
        ));
        for r in &j.reasons {
            md.push_str(&format!("- {r}\n"));
        }
    }
    if review.required {
        md.push_str(&review_markdown(&review, &requested));
    }
    if !sub.approvals.is_empty() {
        md.push_str("\n**Approved:** ");
        md.push_str(
            &sub.approvals
                .iter()
                .map(|a| format!("`{}` by {}", a.row, a.by))
                .collect::<Vec<_>>()
                .join(", "),
        );
        md.push('\n');
    }
    let needing: Vec<&str> = sub
        .report
        .changes
        .iter()
        .filter(|r| r.hints.needs_person && !sub.approved(&r.id))
        .map(|r| r.id.as_str())
        .collect();
    md.push_str("\n<details><summary>Commands</summary>\n\n");
    if !needing.is_empty() {
        md.push_str("Approve a row that needs a person once you have looked at it:\n\n");
        for id in &needing {
            md.push_str(&format!("    /onus approve {id}\n"));
        }
        md.push('\n');
    }
    md.push_str("After merging: `/onus audit ok` or `/onus audit miss <what>` when you audited it, `/onus incident <what happened>` when it caused one. Only people with write access are recorded.\n\n</details>\n");
    std::fs::write(a.out.join("lane.md"), &md)?;

    let summary = json!({
        "lane": lane.as_str(),
        "verdict": judgment.as_ref().map(|j| j.verdict),
        "agent": agent.key(),
        "record": {
            "merged": record.changes,
            "recentIncidents": record.recent_incidents,
            "recentRollbacks": record.recent_rollbacks,
        },
        "rollout": {
            "strategy": plan.strategy,
            "steps": plan.steps,
            "bakeMinutes": plan.bake_minutes,
            "automaticRollback": plan.automatic_rollback,
        },
        // Always present: how a host tells this binary from one without reviews.
        "review": review,
        "risk": c.learned.as_ref().and_then(|l| l.risk),
    });
    println!("{}", serde_json::to_string(&summary)?);
    Ok(0)
}

/// A guard as the comment says it: `error_rate > 1.5× baseline`.
fn guard_text(g: &MetricGuard) -> String {
    let mut parts = Vec::new();
    if let Some(r) = g.max_ratio {
        parts.push(format!("{} > {r}× baseline", g.metric));
    }
    if let Some(r) = g.min_ratio {
        parts.push(format!("{} < {r}× baseline", g.metric));
    }
    if let Some(m) = g.max {
        parts.push(format!("{} > {m}", g.metric));
    }
    if let Some(m) = g.min {
        parts.push(format!("{} < {m}", g.metric));
    }
    parts.join(" or ")
}

/// The **Rollout:** paragraph of the lane section, with the reasons after
/// the lane's own plan.
fn rollout_markdown(p: &RolloutPlan) -> String {
    let mut md = String::from("\n**Rollout:** ");
    if p.strategy == RolloutStrategy::None {
        md.push_str(&format!("`none`: {}.\n", p.reasons.join("; ")));
        return md;
    }
    md.push_str(&format!("`{}`", p.strategy.as_str()));
    if let Some(flag) = &p.flag {
        md.push_str(&format!(" behind `{flag}`"));
    }
    let steps: Vec<String> = p.steps.iter().map(|s| format!("{s}%")).collect();
    md.push_str(&format!(
        " {}, {} min at each step; ",
        steps.join(" → "),
        p.bake_minutes
    ));
    if p.automatic_rollback {
        let guards: Vec<String> = p.guards.iter().map(guard_text).collect();
        md.push_str(&format!(
            "rolls back automatically when {}.\n",
            guards.join(" or ")
        ));
    } else {
        md.push_str("no automatic rollback: no metric guards.\n");
    }
    for r in p.reasons.iter().skip(1) {
        md.push_str(&format!("- {r}\n"));
    }
    md
}

/// The **Learned risk:** line of the lane section.
fn learned_markdown(l: &onus_lanes::classify::LearnedRisk) -> String {
    match (l.risk, &l.note) {
        (Some(risk), _) => format!(
            "\n**Learned risk:** {risk:.2} · the judge from {:.2}, a person from {:.2} · model `{}`\n",
            l.judge_at,
            l.human_at,
            l.model.as_deref().unwrap_or("?")
        ),
        (None, Some(note)) => format!("\n**Learned risk:** {note}\n"),
        (None, None) => String::new(),
    }
}

/// Whom to ask to review a change: the owners of the rows that need a
/// person, with CODEOWNERS at `head` for those rows that name none; the
/// owners of every row when that finds nobody. Sorted.
fn review_owners(repo: &Path, head: &str, report: &SemanticReport) -> Vec<String> {
    let needing: Vec<_> = report
        .changes
        .iter()
        .filter(|r| r.hints.needs_person)
        .collect();
    let mut owners: BTreeSet<String> = needing
        .iter()
        .flat_map(|r| r.hints.owners.iter().cloned())
        .collect();
    let unowned: Vec<_> = needing
        .iter()
        .filter(|r| r.hints.owners.is_empty())
        .collect();
    if !unowned.is_empty() {
        let codeowners = CODEOWNERS_PATHS
            .iter()
            .find_map(|p| lanes::git_lines(repo, &["show", &format!("{head}:{p}")]).ok())
            .map(|lines| Codeowners::parse(&lines.join("\n")))
            .unwrap_or_default();
        for r in unowned {
            for l in &r.locations {
                owners.extend(codeowners.owners_of_file(&l.file));
            }
        }
    }
    if owners.is_empty() {
        owners = report
            .changes
            .iter()
            .flat_map(|r| r.hints.owners.iter().cloned())
            .collect();
    }
    owners.into_iter().collect()
}

/// The **Review:** paragraph of the lane section, for the human lane.
fn review_markdown(review: &ReviewState, asked: &[String]) -> String {
    if review.satisfied {
        return format!(
            "\n**Review:** approved by {}.\n",
            review.approved_by.join(", ")
        );
    }
    let mut md = String::from(
        "\n**Review:** waiting for an approving review from a person with write access who did not open this pull request.",
    );
    if !asked.is_empty() {
        md.push_str(&format!(" Asked: {}.", asked.join(", ")));
    } else if !review.owners.is_empty() {
        md.push_str(&format!(" Owners: {}.", review.owners.join(", ")));
    } else {
        md.push_str(
            " No owners are declared for what changed; add owners in onus.yaml or CODEOWNERS.",
        );
    }
    if !review.changes_requested_by.is_empty() {
        md.push_str(&format!(
            " Changes requested by {}.",
            review.changes_requested_by.join(", ")
        ));
    }
    if !review.ignored.is_empty() {
        let names: Vec<String> = review
            .ignored
            .iter()
            // "no write access (CONTRIBUTOR)" reads "(no write access, CONTRIBUTOR)".
            .map(|i| {
                let why = i.why.replace(" (", ", ");
                format!("{} ({})", i.by, why.trim_end_matches(')'))
            })
            .collect();
        md.push_str(&format!(" Not counted: {}.", names.join(", ")));
    }
    md.push('\n');
    md
}

fn closed(
    change: &str,
    merged: bool,
    commit: Option<String>,
    merged_by: Option<String>,
    records: &Path,
    at: u64,
) -> Result<i32> {
    check_change(change)?;
    std::fs::create_dir_all(records)?;
    let all = pulls::load(&pulls_file(records))?;
    let Some(last) = pulls::latest(&all, change) else {
        println!("no record of {change}; it was not classified while open");
        return Ok(0);
    };
    let result = if merged { "merged" } else { "closed" };
    let existing = load_outcomes(records)?;
    // A rerun of the same event records nothing new.
    if outcomes::latest(&existing)
        .into_iter()
        .any(|o| o.change == change && o.result == result)
    {
        println!("{change} is already on record as {result}");
        return Ok(0);
    }
    // Sampled for a person to audit by a hash of the merge commit, which
    // nobody knew before the merge. Records from before `auditAfterMerge`
    // are not sampled. The printed line says nothing: CI logs can be public.
    let sampled = merged
        && outcomes::approved_without_person(last.lane, last.verdict.as_deref())
        && onus_lanes::classify::audited_after_merge(
            commit.as_deref().unwrap_or(&last.head),
            last.audit_after_merge.unwrap_or(0.0),
        );
    lanes::record_outcome(
        &outcomes_file(records),
        &Outcome {
            at,
            change: change.to_string(),
            agent: last.agent.key(),
            judge: last.judge.clone(),
            lane: last.lane,
            verdict: last.verdict.clone(),
            result: result.into(),
            audited: false,
            missed: false,
            commit,
            involved: vec![],
            note: None,
            components: last.components.clone(),
            reviewed_by: last.reviewed_by.clone(),
            sampled,
            audit: None,
            merged_by,
            features: last.features.clone(),
            caught: last.caught,
            signal: None,
            planned: false,
        },
    )?;
    println!(
        "{change}: {result} in the {} lane, by {}",
        last.lane.as_str(),
        last.agent.key()
    );
    Ok(0)
}

/// A reply for a `/onus` comment, and whether the pull request should be
/// classified again. `report` is the report the comment is about, to check
/// `/onus accuracy wrong <row>` against.
pub fn comment(
    change: &str,
    body: &str,
    author: &str,
    association: &str,
    records: &Path,
    report: Option<&SemanticReport>,
) -> Result<serde_json::Value> {
    check_change(change)?;
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let Some(rest) = line.strip_prefix("/onus") else {
        return Ok(json!({ "command": null }));
    };
    let mut words = rest.split_whitespace();
    let command = words.next().unwrap_or("").to_string();
    let args: Vec<&str> = words.collect();
    let by = format!("@{author}");
    let write_access = ["OWNER", "MEMBER", "COLLABORATOR"].contains(&association);
    if command == "caught" {
        // Anyone may say it: scripts/onus-metrics.sh counts the comment
        // itself, and a hosted service keeps the note. From someone with
        // write access it is also recorded, as a problem the learned lane
        // model trains on.
        let note = args.join(" ");
        let recorded = write_access && record_caught(change, &by, &note, records)?;
        return Ok(json!({
            "command": "caught",
            "ok": true,
            "reply": null,
            "note": (!note.is_empty()).then_some(note),
            "by": by,
            "recorded": recorded,
        }));
    }
    let reply = |ok: bool, text: String, rerun: bool| json!({ "command": command, "ok": ok, "reply": text, "rerun": rerun });
    if !write_access {
        return Ok(reply(
            false,
            format!("Only people with write access can `/onus {command}`; nothing was recorded."),
            false,
        ));
    }
    if command == "accuracy" {
        return Ok(accuracy(&args, &by, report));
    }
    std::fs::create_dir_all(records)?;
    match command.as_str() {
        "approve" => {
            if args.is_empty() {
                return Ok(reply(false, "Name the row: `/onus approve <row id>` (the ids are under Commands in the report).".into(), false));
            }
            let all = pulls::load(&pulls_file(records))?;
            let Some(last) = pulls::latest(&all, change).cloned() else {
                return Ok(reply(false, "Onus has no record of this pull request yet; push to it first.".into(), false));
            };
            let mut next = last;
            for row in &args {
                if !next.approvals.iter().any(|a| a.row == *row) {
                    next.approvals.push(Approval { row: row.to_string(), by: by.clone() });
                }
            }
            next.at = now();
            pulls::append(&pulls_file(records), &next)?;
            Ok(reply(true, format!("Approved {} for {by}. The lane is worked out again.", args.iter().map(|r| format!("`{r}`")).collect::<Vec<_>>().join(", ")), true))
        }
        "audit" => {
            let missed = match args.first().copied() {
                Some("ok") => false,
                Some("miss") => true,
                _ => return Ok(reply(false, "Write `/onus audit ok`, or `/onus audit miss <what was missed>`.".into(), false)),
            };
            let all = load_outcomes(records)?;
            let note = args[1..].join(" ");
            let Ok(audit) = outcomes::audit_record(&all, change, &by, missed, (!note.is_empty()).then_some(note.as_str()), now()) else {
                return Ok(reply(false, "This pull request has no outcome yet; audit it after it merges.".into(), false));
            };
            lanes::record_outcome(&outcomes_file(records), &audit)?;
            Ok(reply(true, format!("Audit recorded{} for {by}.", if missed { " as a miss" } else { "" }), false))
        }
        "incident" => {
            let text = args.join(" ");
            let (note, involved) = match text.split_once("involved:") {
                Some((n, i)) => (n.trim().to_string(), i.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
                None => (text.trim().to_string(), Vec::new()),
            };
            if note.is_empty() {
                return Ok(reply(false, "Say what happened: `/onus incident <what happened> involved: <component>, …`".into(), false));
            }
            match lanes::record_incident(&outcomes_file(records), change, involved, format!("{note} (reported by {by})")) {
                Ok(()) => Ok(reply(true, format!("Incident recorded against this change for {by}. Its agent setup cannot auto-merge until it has a clean record again."), false)),
                Err(e) => Ok(reply(false, format!("{e:#}"), false)),
            }
        }
        _ => Ok(reply(false, "Onus knows `/onus approve <row>`, `/onus audit ok|miss`, `/onus incident <what happened>`, `/onus accuracy ok|wrong|missed` and `/onus caught <what>`.".into(), false)),
    }
}

/// Records `/onus caught` against the change: on its outcome when it has
/// one, else on its pull record. False when Onus has no record of it.
fn record_caught(change: &str, by: &str, note: &str, records: &Path) -> Result<bool> {
    std::fs::create_dir_all(records)?;
    let all = load_outcomes(records)?;
    if let Some(last) = outcomes::latest(&all)
        .into_iter()
        .find(|o| o.change == change)
        .cloned()
    {
        lanes::record_outcome(
            &outcomes_file(records),
            &Outcome {
                at: now(),
                caught: true,
                signal: None,
                note: Some(if note.is_empty() {
                    format!("caught by {by}")
                } else {
                    format!("caught by {by}: {note}")
                }),
                ..last
            },
        )?;
        return Ok(true);
    }
    let pulls = pulls::load(&pulls_file(records))?;
    if let Some(last) = pulls::latest(&pulls, change).cloned() {
        pulls::append(
            &pulls_file(records),
            &PullRecord {
                at: now(),
                caught: true,
                ..last
            },
        )?;
        return Ok(true);
    }
    Ok(false)
}

/// `/onus accuracy ok|wrong <row>|missed <what>`: a person's check of the
/// report. It says what was said, with `wrong 3` turned into the third row's
/// id when the report is known, and records nothing.
fn accuracy(args: &[&str], by: &str, report: Option<&SemanticReport>) -> serde_json::Value {
    let refuse =
        |text: String| json!({ "command": "accuracy", "ok": false, "reply": text, "rerun": false });
    let answer = |verdict: &str, row: Option<String>, note: String| {
        json!({
            "command": "accuracy",
            "ok": true,
            "reply": null,
            "rerun": false,
            "verdict": verdict,
            "row": row,
            "note": (!note.is_empty()).then_some(note),
            "by": by,
        })
    };
    let rest = |from: usize| args.get(from..).unwrap_or_default().join(" ");
    match args.first().copied() {
        Some("ok") => answer("ok", None, rest(1)),
        Some("wrong") => {
            let Some(row) = args.get(1).copied() else {
                return refuse(
                    "Name the row: `/onus accuracy wrong <row number or id> <what is wrong>`."
                        .into(),
                );
            };
            let row = match report {
                Some(report) => match report_row(report, row) {
                    Some(id) => id,
                    None => {
                        return refuse(format!(
                            "This report has no row `{row}`: it has {}. Count rows from 1 in the table, or use a row id.",
                            report.changes.len()
                        ));
                    }
                },
                None => row.to_string(),
            };
            answer("wrong", Some(row), rest(2))
        }
        Some("missed") => {
            let note = rest(1);
            if note.is_empty() {
                return refuse(
                    "Say what the report left out: `/onus accuracy missed <what>`.".into(),
                );
            }
            answer("missed", None, note)
        }
        _ => refuse(
            "Write `/onus accuracy ok`, `/onus accuracy wrong <row> <what>` or `/onus accuracy missed <what>`."
                .into(),
        ),
    }
}

/// The id of a report row named by its number in the table (from 1, `#` allowed)
/// or by its id.
fn report_row(report: &SemanticReport, row: &str) -> Option<String> {
    let n = row.strip_prefix('#').unwrap_or(row).parse::<usize>().ok();
    if let Some(c) = n
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| report.changes.get(i))
    {
        return Some(c.id.clone());
    }
    report
        .changes
        .iter()
        .find(|c| c.id == row)
        .map(|c| c.id.clone())
}

/// Fails unless `s` is a change id, `owner/repo#123`.
pub fn check_change(s: &str) -> Result<()> {
    if !signals::is_change(s) {
        bail!("`{s}` is not a change: write owner/repo#123");
    }
    Ok(())
}

struct RolloutArgs {
    change: Option<String>,
    commit: Option<String>,
    percent: Option<u8>,
    metrics: Option<PathBuf>,
    deploy: Option<String>,
    records: PathBuf,
    repo: PathBuf,
    config: Option<PathBuf>,
    at: u64,
}

/// The change a deploy event is about, and how it was found (`change` or
/// `commit`): one on record, or an error.
fn resolve_change(
    change: Option<&str>,
    commit: Option<&str>,
    outcomes: &[Outcome],
    pulls: &[PullRecord],
) -> Result<(String, &'static str)> {
    if let Some(c) = change {
        check_change(c)?;
    }
    if let Some(c) = commit
        && !signals::is_commit(c)
    {
        bail!("`{c}` is not a commit: write at least 7 hex digits");
    }
    let probe = ProductionSignal {
        change: change.map(str::to_string),
        commit: commit.map(str::to_string),
        ..signal("probe", SignalKind::Rollout, 0)
    };
    match signals::attribute(&probe, outcomes, pulls) {
        (Some(c), Some(by)) => Ok((c, by)),
        _ => bail!(
            "{} is not on record: no pull request or outcome names it",
            change.or(commit).unwrap_or_default()
        ),
    }
}

/// A bare signal written by Onus itself.
fn signal(id: &str, kind: SignalKind, at: u64) -> ProductionSignal {
    ProductionSignal {
        id: id.into(),
        kind,
        at: Some(at),
        source: "onus".into(),
        severity: None,
        change: None,
        commit: None,
        deploy: None,
        environment: None,
        components: vec![],
        symbols: vec![],
        note: None,
        metrics: vec![],
        calls: vec![],
        rollout: None,
        attributed_by: None,
    }
}

/// `onus ci rollout`: the plan, or the decision on one stage.
fn rollout_stage(a: RolloutArgs) -> Result<i32> {
    let (outcomes_path, signals_path) = (outcomes_file(&a.records), a.records.join(signals::FILE));
    let all = load_outcomes(&a.records)?;
    let pulls = pulls::load(&pulls_file(&a.records))?;
    let (change, by) = resolve_change(a.change.as_deref(), a.commit.as_deref(), &all, &pulls)?;
    let pull = pulls::latest(&pulls, &change);
    let last = outcomes::latest(&all)
        .into_iter()
        .find(|o| o.change == change);
    let lane = pull.map(|p| p.lane).or(last.map(|o| o.lane));
    if lane == Some(Lane::Blocked) {
        bail!("{change} is in the blocked lane: a blocked change has no rollout");
    }
    let plan = match pull.and_then(|p| p.rollout.clone()) {
        Some(p) => p,
        // Records from before rollout plans: the plan the lane gets now,
        // from the repository's policy and its components' history.
        None => {
            let config = lanes::lanes_config(&a.repo, a.config.as_deref())?;
            let components = pull
                .map(|p| p.components.clone())
                .or(last.map(|o| o.components.clone()))
                .unwrap_or_default();
            let history = lanes::area_histories(
                &all,
                Some(&signals_path),
                &components.iter().cloned().collect(),
                &lanes::history_scope(
                    &config,
                    &lanes::ClassifyOpts {
                        change: Some(&change),
                        ..Default::default()
                    },
                    a.at,
                ),
            )?;
            rollout::plan_for(
                lane.unwrap_or(Lane::Judge),
                components,
                rollout::flag_key(Some(&change), ""),
                config.rollout.as_ref(),
                &history,
                config.inputs.history.days,
            )
        }
    };
    let Some(percent) = a.percent else {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(0);
    };
    let samples = match &a.metrics {
        Some(p) => {
            let m: RolloutMetrics = serde_json::from_str(&std::fs::read_to_string(p)?)
                .with_context(|| {
                    format!(
                        "{} is not rollout metrics (schemas/rollout-metrics.schema.json)",
                        p.display()
                    )
                })?;
            m.metrics
        }
        None => Vec::new(),
    };
    let on_record = signals::load(&signals_path)?;
    // The stage started when it was first reported.
    let started = on_record
        .iter()
        .filter(|s| {
            s.kind == SignalKind::Rollout
                && s.change.as_deref() == Some(change.as_str())
                && s.rollout.as_ref().is_some_and(|r| r.percent == percent)
        })
        .filter_map(|s| s.at)
        .min()
        .unwrap_or(a.at);
    let d = rollout::evaluate(&change, &plan, percent, &samples, started, a.at);
    let id = format!("rollout:{change}:{percent}:{}", a.at);
    if !on_record.iter().any(|s| s.id == id) {
        signals::append(
            &signals_path,
            &ProductionSignal {
                change: Some(change.clone()),
                commit: a.commit.clone(),
                deploy: a.deploy.clone(),
                components: plan.components.clone(),
                note: Some(d.reasons.join("; ")),
                metrics: samples,
                rollout: Some(RolloutStage {
                    percent,
                    decision: Some(d.decision.as_str().into()),
                    flag: plan.flag.clone(),
                }),
                attributed_by: Some(by.into()),
                ..signal(&id, SignalKind::Rollout, a.at)
            },
        )?;
    }
    if d.decision == Decision::Rollback {
        let rolled = outcomes::latest(&all)
            .into_iter()
            .any(|o| o.change == change && o.result == "rolled-back" && !o.planned);
        if !rolled && let Some(o) = signals::outcome_of(&change, &all, &pulls, a.at, "rolled-back")
        {
            lanes::record_outcome(
                &outcomes_path,
                &Outcome {
                    note: Some(d.reasons.join("; ")),
                    involved: plan.components.clone(),
                    signal: Some(id),
                    ..o
                },
            )?;
        }
    }
    println!("{}", serde_json::to_string(&d)?);
    Ok(match d.decision {
        Decision::Promote | Decision::Complete => 0,
        Decision::Rollback => 2,
        Decision::Hold => 3,
    })
}

/// `onus ci rollback`: a rollback decided elsewhere.
fn rollback(
    change: Option<&str>,
    commit: Option<&str>,
    reason: &str,
    planned: bool,
    involved: Vec<String>,
    records: &Path,
    at: u64,
) -> Result<i32> {
    let all = load_outcomes(records)?;
    let pulls = pulls::load(&pulls_file(records))?;
    let (change, _) = resolve_change(change, commit, &all, &pulls)?;
    // A rollback that was not planned still counts after a planned one.
    if outcomes::latest(&all)
        .into_iter()
        .any(|o| o.change == change && o.result == "rolled-back" && (planned || !o.planned))
    {
        println!("{change}: already rolled back");
        return Ok(0);
    }
    if planned {
        // On purpose: an outcome that counts against no one, and no signal.
        let Some(o) = signals::outcome_of(&change, &all, &pulls, at, "rolled-back") else {
            bail!("{change} is not on record");
        };
        lanes::record_outcome(
            &outcomes_file(records),
            &Outcome {
                note: Some(reason.to_string()),
                involved,
                planned: true,
                ..o
            },
        )?;
    } else {
        signals::ingest(
            records,
            vec![ProductionSignal {
                change: Some(change.clone()),
                note: Some(reason.to_string()),
                components: involved,
                ..signal(&format!("rollback:{change}:{at}"), SignalKind::Rollback, at)
            }],
            at,
        )?;
    }
    println!("{change}: rolled back");
    Ok(0)
}
