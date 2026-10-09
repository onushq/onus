//! `onus ci`: what the GitHub Action runs at each event, so outcomes are
//! recorded without anyone typing them (`onus help ci`).
//!
//! - A pull request is pushed: `onus ci pr` makes its submission (the pull
//!   request body as intent, the agent from git and GitHub), classifies it
//!   with the agent's track record, runs the judge when asked, and keeps a
//!   pull request record.
//! - It closes: `onus ci closed` turns its last record into an outcome,
//!   merged or closed.
//! - Someone comments `/onus approve`, `/onus audit` or `/onus incident`:
//!   `onus ci comment` records it, for people with write access only.
//! - The default branch moves: `onus ci push` records reverts.
//!
//! Records live in a folder that `onus ci records pull|push` syncs with a
//! branch of the repository.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use onus_core::{Lane, SemanticReport};
use onus_lanes::outcomes::{self, Outcome};
use onus_lanes::pulls::{self, PullRecord};
use onus_lanes::submission::Approval;
use serde_json::json;

use crate::{lanes, records};

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
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Where to write the submission, classification, judgment and comment section.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
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
        #[arg(long, value_name = "DIR")]
        records: PathBuf,
    },
    /// A comment on a pull request: `/onus approve <row>`, `/onus audit ok|miss`,
    /// `/onus incident <what happened>`. Prints what to reply.
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
            config,
            out,
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
            config,
            out,
        }),
        CiCmd::Closed {
            change,
            merged,
            commit,
            records,
        } => closed(&change, merged, commit, &records),
        CiCmd::Comment {
            change,
            body_file,
            author,
            association,
            records,
        } => {
            let body = std::fs::read_to_string(&body_file)?;
            let answer = comment(&change, &body, &author, &association, &records)?;
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
    config: Option<PathBuf>,
    out: PathBuf,
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
    let agent = pulls::detect_agent(&message, &a.branch, &a.author);
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
    let all_pulls = pulls::load(&pulls_file(&a.records))?;
    let approvals = pulls::latest(&all_pulls, &a.change)
        .map(|r| r.approvals.clone())
        .unwrap_or_default();
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
    let c = lanes::classify_with(
        &sub.report,
        Some(&sub),
        &config,
        Some(&outcomes_file(&a.records)),
    )?;
    let judgment = (a.judge && c.lane == Lane::Judge)
        .then(|| lanes::judge_submission(&a.repo, &sub, &c, &config));
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
    let all_outcomes = load_outcomes(&a.records)?;
    let record = outcomes::record_for(&all_outcomes, &agent.key());
    let lane = judgment.as_ref().map_or(c.lane, |j| j.lane);
    pulls::append(
        &pulls_file(&a.records),
        &PullRecord {
            at: now(),
            change: a.change.clone(),
            base,
            head,
            agent: agent.clone(),
            lane,
            verdict: judgment
                .as_ref()
                .map(|j| verdict_name(j.verdict).to_string()),
            judge: Some(onus_lanes::judge::judge_id(&config)),
            approvals,
        },
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
    md.push_str(&format!(
        "\n**Agent:** `{}` · {} merged on record, {} incidents in its last {}{}\n",
        agent.key(),
        record.changes,
        record.recent_incidents,
        outcomes::RECENT,
        if record.changes < config.min_record {
            format!(" · auto-merge needs {} merged", config.min_record)
        } else {
            String::new()
        }
    ));
    if let Some(j) = &judgment {
        md.push_str(&format!("\n**Judge:** `{}`\n", verdict_name(j.verdict)));
        for r in &j.reasons {
            md.push_str(&format!("- {r}\n"));
        }
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
        "record": { "merged": record.changes, "recentIncidents": record.recent_incidents },
    });
    println!("{}", serde_json::to_string(&summary)?);
    Ok(0)
}

fn closed(change: &str, merged: bool, commit: Option<String>, records: &Path) -> Result<i32> {
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
    lanes::record_outcome(
        &outcomes_file(records),
        &Outcome {
            at: now(),
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
/// classified again.
pub fn comment(
    change: &str,
    body: &str,
    author: &str,
    association: &str,
    records: &Path,
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
    if command == "caught" {
        // Counted from the comment itself by scripts/onus-metrics.sh.
        return Ok(json!({ "command": "caught", "ok": true, "reply": null }));
    }
    let reply = |ok: bool, text: String, rerun: bool| json!({ "command": command, "ok": ok, "reply": text, "rerun": rerun });
    if !["OWNER", "MEMBER", "COLLABORATOR"].contains(&association) {
        return Ok(reply(
            false,
            format!("Only people with write access can `/onus {command}`; nothing was recorded."),
            false,
        ));
    }
    std::fs::create_dir_all(records)?;
    let by = format!("@{author}");
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
            let Some(last) = outcomes::latest(&all).into_iter().find(|o| o.change == change).cloned() else {
                return Ok(reply(false, "This pull request has no outcome yet; audit it after it merges.".into(), false));
            };
            let note = args[1..].join(" ");
            lanes::record_outcome(
                &outcomes_file(records),
                &Outcome {
                    at: now(),
                    audited: true,
                    missed,
                    note: Some(format!("audited by {by}{}", if note.is_empty() { String::new() } else { format!(": {note}") })),
                    ..last
                },
            )?;
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
        _ => Ok(reply(false, "Onus knows `/onus approve <row>`, `/onus audit ok|miss`, `/onus incident <what happened>` and `/onus caught <what>`.".into(), false)),
    }
}

/// Fails unless `s` is a change id, `owner/repo#123`.
pub fn check_change(s: &str) -> Result<()> {
    let ok = s
        .split_once('#')
        .is_some_and(|(repo, n)| repo.contains('/') && n.parse::<u64>().is_ok());
    if !ok {
        bail!("`{s}` is not a change: write owner/repo#123");
    }
    Ok(())
}
