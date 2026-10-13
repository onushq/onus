//! `onus provenance`: one record of why a change was allowed through, built
//! from the facts it is given (`onus help provenance`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use onus_core::provenance::{
    IntentSource, ProvenanceRecord, SCHEMA, Token, TokenInspection, TokenSource,
};
use onus_lanes::classify::Classification;
use onus_lanes::judge::Judgment;
use onus_lanes::provenance::{self, CloudEscalations, Inputs};
use onus_lanes::submission::Submission;
use onus_lanes::{outcomes, pulls, signals};

use crate::{doors, lanes, ui};

#[derive(Debug, Args)]
pub struct ProvenanceArgs {
    /// The change: owner/repo#123 (as onus ci records it), or any id the outcome records use.
    #[arg(long)]
    change: Option<String>,
    /// A records folder (pulls.jsonl, outcomes.jsonl, signals.jsonl), as `onus ci` keeps it.
    #[arg(long, value_name = "DIR")]
    records: Option<PathBuf>,
    /// Outcome records, instead of <records>/outcomes.jsonl.
    #[arg(long, value_name = "FILE")]
    outcomes: Option<PathBuf>,
    /// Production signals, instead of <records>/signals.jsonl: the rollout's status,
    /// stages and the signals tied to the change.
    #[arg(long, value_name = "FILE")]
    signals: Option<PathBuf>,
    /// An earlier provenance.json of this change (from onus ci pr): its facts from while
    /// the change was open are kept.
    #[arg(long, value_name = "FILE")]
    from: Option<PathBuf>,
    /// A submission (from onus submit).
    #[arg(long, value_name = "FILE")]
    submission: Option<PathBuf>,
    /// A submission kept by onus ui / onus mcp in <repo>/.onus/submissions; also reads its
    /// judgment and the repository's .onus audit log, escalations and outcomes unless given.
    #[arg(long, value_name = "ID", conflicts_with = "submission")]
    submission_id: Option<String>,
    /// The classification (classification.json from onus ci pr, or onus classify).
    #[arg(long, value_name = "FILE")]
    classification: Option<PathBuf>,
    /// judgment.json, or onus ui's {classification, judgment}.
    #[arg(long, value_name = "FILE")]
    judgment: Option<PathBuf>,
    /// The task token the change was made under; verified with --key-public, and never kept.
    #[arg(long, env = "ONUS_TOKEN", value_name = "TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// The root public key that verifies --token.
    #[arg(long, value_name = "FILE")]
    key_public: Option<PathBuf>,
    /// The door's inspection of the task token ({task, rights, expires}, as Onus Cloud
    /// keeps it or `onus token inspect` prints it); never the token.
    #[arg(long, value_name = "FILE")]
    token_inspection: Option<PathBuf>,
    /// The door log (audit.jsonl); its hash chain is checked.
    #[arg(long, value_name = "FILE")]
    audit: Option<PathBuf>,
    /// Escalations: a folder of requests (.onus/escalations), or a JSON file of the
    /// decisions Onus Cloud keeps (schemas/escalation-decisions.schema.json).
    #[arg(long, value_name = "PATH")]
    escalations: Option<PathBuf>,
    /// The task whose door decisions to include (default: the token's or submission's task).
    #[arg(long)]
    task: Option<String>,
    /// The repository whose .onus folder --submission-id reads.
    #[arg(long, default_value = ".", value_name = "DIR")]
    repo: PathBuf,
    /// Write the record here instead of printing it.
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
    /// Check a record's digest and schema version instead; exit 1 when it does not match.
    #[arg(long, value_name = "FILE", conflicts_with_all = ["change", "submission", "submission_id", "from"])]
    verify: Option<PathBuf>,
}

fn read<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not {what}", path.display()))
}

/// A judgment file: `judgment.json`, or onus ui's `{classification, judgment}`.
fn read_judgment(path: &Path) -> Result<(Option<Classification>, Judgment)> {
    let v: serde_json::Value = read(path, "a judgment")?;
    if v.get("judgment").is_some() {
        let c = match v.get("classification") {
            Some(c) if !c.is_null() => Some(serde_json::from_value(c.clone())?),
            _ => None,
        };
        let j = serde_json::from_value(v["judgment"].clone())
            .with_context(|| format!("{} is not a judgment", path.display()))?;
        return Ok((c, j));
    }
    let j = serde_json::from_value(v)
        .with_context(|| format!("{} is not a judgment", path.display()))?;
    Ok((None, j))
}

/// Every escalation request in `dir` (`<id>.json`; `<id>.cloud.json` and
/// tokens beside them are not requests).
fn requests_in(dir: &Path) -> Result<Vec<onus_doors::escalation::Request>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|x| x.to_str()) == Some("json")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| !s.contains('.'))
        })
        .collect();
    paths.sort();
    paths.iter().map(|p| doors::load_request(p)).collect()
}

/// The change's commits, base excluded, for joining Onus Cloud's
/// escalations to it; none when git cannot list them here.
pub(crate) fn change_commits(repo: &Path, base: &str, head: &str) -> Vec<String> {
    if base.is_empty() || head.is_empty() {
        return Vec::new();
    }
    crate::lanes::git_lines(
        repo,
        &["rev-list", "--max-count=5000", &format!("{base}..{head}")],
    )
    .unwrap_or_default()
}

/// Escalations as Onus Cloud keeps them, from a file (never a folder). Each
/// one that cannot be read is warned about, and left out.
pub(crate) fn decisions_at(path: &Path) -> Result<CloudEscalations> {
    if path.is_dir() {
        bail!(
            "{} is a folder: give a file of escalations as Onus Cloud keeps them (schemas/escalation-decisions.schema.json)",
            path.display()
        );
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let read = provenance::parse_decisions(&text).with_context(|| {
        format!(
            "{} is not escalations as Onus Cloud keeps them (schemas/escalation-decisions.schema.json)",
            path.display()
        )
    })?;
    for note in &read.unread {
        eprintln!(
            "warning: {}: {note}; it is left out, and escalations are a gap",
            path.display()
        );
    }
    Ok(read)
}

/// The door's inspection of a task token, from a file.
pub(crate) fn inspection_at(path: &Path) -> Result<TokenInspection> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    provenance::parse_inspection(&text)
        .with_context(|| format!("{} is not a token inspection", path.display()))
}

fn verify(path: &Path) -> Result<i32> {
    let v: serde_json::Value = read(path, "a provenance record")?;
    let schema = v["schema"].as_u64();
    if schema != Some(u64::from(SCHEMA)) {
        eprintln!(
            "onus: {} is schema {}; this onus reads schema {SCHEMA}",
            path.display(),
            schema.map_or("none".into(), |s| s.to_string())
        );
        return Ok(1);
    }
    let r: ProvenanceRecord = serde_json::from_value(v)
        .with_context(|| format!("{} is not a provenance record", path.display()))?;
    if !provenance::digest_ok(&r) {
        eprintln!(
            "onus: {}: the digest does not match; the record was changed after it was made",
            path.display()
        );
        return Ok(1);
    }
    println!(
        "{}: {} ({}), digest matches",
        path.display(),
        r.change,
        serde_json::to_value(r.stage)?.as_str().unwrap_or_default()
    );
    Ok(0)
}

pub fn run(args: ProvenanceArgs) -> Result<i32> {
    if let Some(file) = &args.verify {
        return verify(file);
    }
    // What onus ui and onus mcp keep in the repository, for --submission-id.
    let kept = args.submission_id.as_ref().map(|_| ui::places(&args.repo));
    let (submission, kept_judgment): (Option<Submission>, Option<PathBuf>) =
        match (&args.submission, &args.submission_id, &kept) {
            (Some(p), _, _) => (Some(read(p, "a submission")?), None),
            (None, Some(id), Some(k)) => {
                if id.len() < 6 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
                    bail!("`{id}` is not a submission id");
                }
                let path = k.submissions.join(format!("{id}.json"));
                if !path.exists() {
                    bail!("no submission {id} in {}", k.submissions.display());
                }
                (
                    Some(read(&path, "a submission")?),
                    Some(k.submissions.join(format!("{id}.judgment.json"))),
                )
            }
            _ => (None, None),
        };
    let from: Option<ProvenanceRecord> = match &args.from {
        Some(p) => Some(read(p, "a provenance record")?),
        None => None,
    };
    let Some(change) = args
        .change
        .clone()
        .or_else(|| from.as_ref().map(|f| f.change.clone()))
        .or_else(|| submission.as_ref().map(|s| s.head.clone()))
    else {
        bail!("name the change: give --change, or --submission, --submission-id or --from");
    };
    let judgment_file = args
        .judgment
        .clone()
        .or(kept_judgment.filter(|p| p.exists()));
    let (mut classification, judgment) = match &judgment_file {
        Some(p) => {
            let (c, j) = read_judgment(p)?;
            (c, Some(j))
        }
        None => (None, None),
    };
    if let Some(p) = &args.classification {
        classification = Some(read(p, "a classification")?);
    }
    let pulls = match &args.records {
        Some(dir) => pulls::load(&dir.join("pulls.jsonl"))?,
        None => Vec::new(),
    };
    let outcomes_file = args
        .outcomes
        .clone()
        .or_else(|| args.records.as_ref().map(|d| d.join("outcomes.jsonl")))
        .or_else(|| kept.as_ref().map(|k| k.outcomes.clone()));
    let outcomes = match &outcomes_file {
        Some(f) => outcomes::load(f)?,
        None => Vec::new(),
    };
    let signals_file = args
        .signals
        .clone()
        .or_else(|| args.records.as_ref().map(|d| d.join(signals::FILE)));
    let signals = match &signals_file {
        Some(f) => signals::load(f)?,
        None => Vec::new(),
    };
    // A token that does not verify is an error, never a gap.
    let token = match (&args.token, &args.key_public) {
        (Some(t), Some(key)) => {
            let (scope, _) = lanes::scope_and_agent(t, key)
                .context("the token does not verify with --key-public")?;
            Some(Token {
                task: scope.task,
                rights: scope.rights,
                attenuations: scope.attenuations,
                expires: scope.expires,
                fingerprint: scope.fingerprint,
                source: TokenSource::Token,
            })
        }
        (Some(_), None) => {
            bail!("a token is verified with the root public key: give --key-public")
        }
        _ => None,
    };
    let audit_file = args.audit.clone().or_else(|| {
        kept.as_ref()
            .map(|k| k.audit.clone())
            .filter(|p| p.exists())
    });
    let audit = match &audit_file {
        Some(f) => Some(provenance::read_audit(f)?),
        None => None,
    };
    // A folder of requests, or Onus Cloud's decisions in a file, read with
    // the requests kept beside a submission. The kept folder may not exist
    // yet: it is made when a request is filed.
    let kept_requests = || match &kept {
        Some(k) => requests_in(&k.escalations),
        None => Ok(Vec::new()),
    };
    let (requests, decisions) = match &args.escalations {
        Some(dir) if dir.is_dir() => (requests_in(dir)?, None),
        Some(file) => (kept_requests()?, Some(decisions_at(file)?)),
        None => (kept_requests()?, None),
    };
    // Onus Cloud's escalations that name no pull request are joined by the
    // commits they were filed at.
    let commits = match &decisions {
        Some(_) => {
            let pull = pulls::latest(&pulls, &change);
            let (base, head) = pull
                .map(|p| (p.base.clone(), p.head.clone()))
                .or_else(|| {
                    submission
                        .as_ref()
                        .map(|s| (s.base.clone(), s.head.clone()))
                })
                .or_else(|| {
                    from.as_ref()
                        .map(|f| (f.commits.base.clone(), f.commits.head.clone()))
                })
                .unwrap_or_default();
            change_commits(&args.repo, &base, &head)
        }
        None => Vec::new(),
    };
    let inspection = args
        .token_inspection
        .as_deref()
        .map(inspection_at)
        .transpose()?;
    if let (Some(t), Some(task)) = (&inspection, &args.task)
        && t.task != *task
    {
        bail!("the token inspection is for task {}, not {task}", t.task);
    }
    let record = provenance::build(Inputs {
        from,
        submission: submission.as_ref(),
        intent_source: IntentSource::Submission,
        classification: classification.as_ref(),
        judgment: judgment.as_ref(),
        policy: judgment.as_ref().map(|j| j.judge.clone()),
        pulls: &pulls,
        outcomes: &outcomes,
        signals: &signals,
        token,
        audit,
        requests: &requests,
        decisions: decisions.as_ref().map(|d| d.list.as_slice()),
        unread: decisions.as_ref().map_or(&[], |d| d.unread.as_slice()),
        inspection,
        commits: &commits,
        task: args.task.as_deref(),
        ..Inputs::new(&change, onus_doors::gateway::now())
    });
    let json = serde_json::to_string_pretty(&record)?;
    match &args.out {
        Some(p) => std::fs::write(p, format!("{json}\n"))
            .with_context(|| format!("cannot write {}", p.display()))?,
        None => println!("{json}"),
    }
    Ok(0)
}
