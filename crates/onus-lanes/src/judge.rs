//! The verifying judge: verification first, taste last.
//!
//! 1. Re-run the submission's test evidence in a fresh container.
//! 2. Compare intent with effect.
//! 3. Check contracts and boundary rules.
//! 4. Check for weakened tests.
//! 5. Run held-out checks the author never saw, chosen from the map
//!    ([`crate::heldout`]).
//! 6. Taste, optionally: a reviewer command that can raise concerns, never
//!    approve.
//!
//! The judge stops at the first failure: every later step is recorded as
//! not reached, and no more containers start. When `lanes.judges` is set,
//! one of those configurations judges the change, picked by the rotation.
//!
//! The judge reads the submission (the report, the evidence, the scope),
//! never the author's reasoning. The verdict is `approve`, `reject` with
//! reasons the agent can act on, or `escalate` to a person.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Result;
use onus_core::{ChangeKind, JudgeConfig, Lane, LanesConfig};
use onus_doors::runner::TestRun;
use onus_env::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::classify::Classification;
use crate::heldout::{self, CheckResult, HeldOutReport, LoadedSuite, Reach};
use crate::submission::Submission;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    Passed,
    Failed,
    Skipped,
    Concern,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub name: String,
    pub status: StepStatus,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Approve,
    Reject,
    Escalate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Judgment {
    pub verdict: Verdict,
    pub lane: Lane,
    pub steps: Vec<Step>,
    /// What to do about a rejection or why a person decides, in order.
    pub reasons: Vec<String>,
    /// Which judge configuration decided: a hash of the lane policy and the
    /// Onus version, as `<name>@<hash>` when `lanes.judges` is set, for
    /// outcome tracking.
    pub judge: String,
    /// The configuration from `lanes.judges` that judged, when there are any.
    /// The agent door leaves it out, and gives the policy's id as `judge`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<Rotation>,
    /// What the held-out checks did, in counts only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_out: Option<HeldOutSummary>,
}

/// The judge configuration that judged a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rotation {
    /// Its name in `lanes.judges`.
    pub config: String,
    /// Its place in `lanes.judges`, from 1.
    pub index: u32,
    /// How many configurations there are.
    pub of: u32,
    /// A person chose it (`--judge-config`) rather than the rotation.
    pub forced: bool,
}

/// The held-out step in counts: safe for the author to read, as it names
/// no check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldOutSummary {
    /// The suite's short hash; empty with `lanes.heldOut` alone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suite: String,
    pub selected: u32,
    pub ran: u32,
    pub passed: u32,
}

/// Runs a command at a commit in a container: `(repo, commit, image,
/// setup, command, extra)`, with the directory `extra` (a held-out suite)
/// copied into the test container at `/held-out`. The real one is
/// [`onus_doors::runner::run_with`].
pub type Runner<'a> =
    &'a dyn Fn(&Path, &str, &str, Option<&str>, &str, Option<&Path>) -> Result<TestRun>;

/// What the judge gets besides the submission and the policy.
#[derive(Debug, Default)]
pub struct JudgeOptions<'a> {
    /// The evidence store that runs naming a manifest are checked against.
    pub store: Option<&'a Store>,
    /// A held-out suite from outside the repository (`--held-out`).
    pub suite: Option<&'a LoadedSuite>,
    /// What the change reaches in the head map, which picks the checks.
    pub reach: Option<&'a Reach>,
    /// A configuration from `lanes.judges` to use instead of the rotation's.
    pub judge_config: Option<&'a str>,
}

/// The judgment, and what the held-out checks did for people to read.
#[derive(Debug, Clone)]
pub struct Judged {
    pub judgment: Judgment,
    /// Present when a suite was given; never shown to the author.
    pub held_out: Option<HeldOutReport>,
}

/// The judge configuration id.
pub fn judge_id(config: &LanesConfig) -> String {
    let mut h = Sha256::new();
    h.update(format!("onus {} ", env!("CARGO_PKG_VERSION")).as_bytes());
    h.update(serde_json::to_string(config).unwrap_or_default().as_bytes());
    h.finalize()[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The id of the configuration `chosen` from `lanes.judges`: `<name>@<hash>`,
/// the hash over the policy with that one configuration. Without one, the
/// policy's [`judge_id`].
pub fn judge_id_for(config: &LanesConfig, chosen: Option<&JudgeConfig>) -> String {
    match chosen {
        None => judge_id(config),
        Some(j) => format!(
            "{}@{}",
            j.name,
            judge_id(&LanesConfig {
                judges: vec![j.clone()],
                ..config.clone()
            })
        ),
    }
}

fn step(name: &str, status: StepStatus, details: Vec<String>) -> Step {
    Step {
        name: name.into(),
        status,
        details,
    }
}

pub fn judge(
    sub: &Submission,
    classification: &Classification,
    config: &LanesConfig,
    repo: &Path,
    run: Runner,
) -> Judgment {
    let chosen = heldout::choose(config, &sub.head, "", None).ok().flatten();
    judge_chosen(
        sub,
        classification,
        config,
        repo,
        run,
        &JudgeOptions::default(),
        chosen,
    )
    .judgment
}

/// A claimed run checked against its manifest in the evidence store: it
/// must have run at the head commit, and say what the manifest recorded.
fn manifest_problem(e: &TestRun, head: &str, store: Option<&Store>) -> Option<String> {
    let id = e.manifest.as_deref()?;
    let Some(store) = store else {
        return Some(format!(
            "`{}` names evidence manifest {id}, but there is no evidence store",
            e.command
        ));
    };
    let (full, m) = match store.manifest(id) {
        Ok(found) => found,
        Err(err) => return Some(format!("`{}`: {err:#}", e.command)),
    };
    let short = &full[..12];
    if m.commit != head {
        Some(format!(
            "`{}`: manifest {short} ran at {}, not at the head commit",
            e.command,
            &m.commit[..m.commit.len().min(12)]
        ))
    } else if m.command != e.command || m.exit_code != e.exit_code || m.commit != e.commit {
        Some(format!(
            "`{}`: the run does not match manifest {short} (it recorded `{}`, exit {})",
            e.command, m.command, m.exit_code
        ))
    } else {
        None
    }
}

/// [`judge`], with test runs that name a manifest checked against the
/// evidence store, a held-out suite, and a forced configuration. It fails
/// only when `judge_config` names no configuration in `lanes.judges`.
pub fn judge_with(
    sub: &Submission,
    classification: &Classification,
    config: &LanesConfig,
    repo: &Path,
    run: Runner,
    opts: &JudgeOptions,
) -> Result<Judged> {
    let key = opts.suite.map_or("", |s| s.key.as_str());
    let chosen = heldout::choose(config, &sub.head, key, opts.judge_config)?;
    Ok(judge_chosen(
        sub,
        classification,
        config,
        repo,
        run,
        opts,
        chosen,
    ))
}

/// 1. Evidence, re-run where the author cannot influence it. It stops at
///    the first run that fails.
fn evidence_step(sub: &Submission, repo: &Path, run: Runner, store: Option<&Store>) -> Step {
    if sub.evidence.is_empty() {
        return step(
            "evidence",
            StepStatus::Failed,
            vec!["no test evidence: run the tests that cover the change with `onus run-test` and include the runs".into()],
        );
    }
    let mut recorded = 0;
    for (i, e) in sub.evidence.iter().enumerate() {
        let problem = if e.failed() {
            Some(format!("`{}` failed when the author ran it", e.command))
        } else if let Some(problem) = manifest_problem(e, &sub.head, store) {
            Some(problem)
        } else {
            if e.manifest.is_some() {
                recorded += 1;
            }
            match run(
                repo,
                &sub.head,
                &e.image,
                e.setup.as_deref(),
                &e.command,
                None,
            ) {
                Ok(again) if !again.failed() => None,
                Ok(again) => Some(format!(
                    "`{}` passed for the author but fails at {} (exit {})",
                    e.command,
                    &sub.head[..sub.head.len().min(12)],
                    again.exit_code
                )),
                Err(err) => Some(format!("`{}` could not be re-run: {err:#}", e.command)),
            }
        };
        if let Some(problem) = problem {
            let left = sub.evidence.len() - i - 1;
            let mut details = vec![problem];
            match left {
                0 => {}
                1 => details.push("1 more run not re-run".into()),
                n => details.push(format!("{n} more runs not re-run")),
            }
            return step("evidence", StepStatus::Failed, details);
        }
    }
    step(
        "evidence",
        StepStatus::Passed,
        std::iter::once(format!("{} test runs passed again", sub.evidence.len()))
            .chain(
                (recorded > 0)
                    .then(|| format!("{recorded} matched their manifests in the evidence store")),
            )
            .collect(),
    )
}

/// 2. Intent against effect.
fn intent_step(sub: &Submission) -> Step {
    if sub.intent.as_deref().is_none_or(|i| i.trim().is_empty()) {
        return step(
            "intent",
            StepStatus::Failed,
            vec![
                "no stated intent: add an `onus-intent` block saying what the change touches"
                    .into(),
            ],
        );
    }
    let outside: Vec<String> = sub
        .report
        .changes
        .iter()
        .filter(|r| r.hints.intent_mismatch)
        .map(|r| format!("“{}” is outside the stated intent", r.title))
        .collect();
    if outside.is_empty() {
        step("intent", StepStatus::Passed, vec![])
    } else {
        step("intent", StepStatus::Failed, outside)
    }
}

/// 3. Contracts and boundary rules.
fn contracts_step(sub: &Submission) -> Step {
    let broken: Vec<String> = sub
        .report
        .changes
        .iter()
        .filter(|r| r.kind == ChangeKind::Breaking || r.subkind == "rule-violation")
        .map(|r| format!("{}: {}", r.title, r.why_it_matters))
        .collect();
    if broken.is_empty() {
        step("contracts", StepStatus::Passed, vec![])
    } else {
        step("contracts", StepStatus::Failed, broken)
    }
}

/// 4. Weakened tests.
fn tests_step(sub: &Submission) -> Step {
    let weakened: Vec<String> = sub
        .report
        .changes
        .iter()
        .filter(|r| r.subkind == "test-weakened" && !sub.approved(&r.id))
        .map(|r| format!("{}: {}", r.title, r.why_it_matters))
        .collect();
    if weakened.is_empty() {
        step("tests", StepStatus::Passed, vec![])
    } else {
        step("tests", StepStatus::Failed, weakened)
    }
}

/// The judge under the configuration `chosen` from `lanes.judges`.
fn judge_chosen(
    sub: &Submission,
    classification: &Classification,
    config: &LanesConfig,
    repo: &Path,
    run: Runner,
    opts: &JudgeOptions,
    chosen: Option<(usize, &JudgeConfig, bool)>,
) -> Judged {
    let chosen_config = chosen.map(|(_, j, _)| j);
    let judge = judge_id_for(config, chosen_config);
    let mut steps = Vec::new();
    // The first step that failed: every later one is not reached.
    let mut failed: Option<&'static str> = None;
    let not_reached = |f: &str| vec![format!("not reached: {f} failed")];
    for name in ["evidence", "intent", "contracts", "tests"] {
        let s = match failed {
            Some(f) => step(name, StepStatus::Skipped, not_reached(f)),
            None => match name {
                "evidence" => evidence_step(sub, repo, run, opts.store),
                "intent" => intent_step(sub),
                "contracts" => contracts_step(sub),
                _ => tests_step(sub),
            },
        };
        if s.status == StepStatus::Failed {
            failed = Some(name);
        }
        steps.push(s);
    }

    // 5. Held-out checks, in order of relevance, up to the first failure.
    // Their ids and output are not shown: the author must not learn the
    // hidden suite from the verdict.
    let suite = opts.suite;
    let legacy = config.held_out.as_ref();
    let no_reach = Reach::default();
    let reach = opts.reach.unwrap_or(&no_reach);
    let limit = heldout::limit_for(suite.map(|s| &s.suite), chosen_config);
    let selected = heldout::select(suite.map(|s| &s.suite), legacy, reach, limit);
    let mut results: Vec<CheckResult> = selected
        .iter()
        .map(|s| CheckResult {
            id: s.check.id.clone(),
            why: s.why,
            with: s.with.clone(),
            status: "not-run".into(),
            exit_code: None,
            output_tail: String::new(),
        })
        .collect();
    let (mut ran, mut passed) = (0u32, 0u32);
    let held_out = if let Some(f) = failed {
        step("held-out", StepStatus::Skipped, not_reached(f))
    } else if suite.is_none() && legacy.is_none() {
        step(
            "held-out",
            StepStatus::Skipped,
            vec!["no held-out checks configured (--held-out or lanes.heldOut)".into()],
        )
    } else if selected.is_empty() {
        step(
            "held-out",
            StepStatus::Skipped,
            vec!["no held-out check covers this change".into()],
        )
    } else {
        let mut problem = None;
        for (s, result) in selected.iter().zip(&mut results) {
            let c = &s.check;
            // lanes.heldOut has its own image and setup, and no suite.
            let in_suite = suite.filter(|_| c.id != heldout::LEGACY_ID);
            let image = c
                .image
                .as_deref()
                .or(in_suite.and_then(|s| s.suite.image.as_deref()))
                .unwrap_or("node:22");
            let setup = c
                .setup
                .as_deref()
                .or(in_suite.and_then(|s| s.suite.setup.as_deref()));
            let extra = in_suite.map(|s| s.dir.as_path());
            ran += 1;
            match run(repo, &sub.head, image, setup, &c.command, extra) {
                Ok(r) => {
                    result.exit_code = Some(r.exit_code);
                    result.output_tail = r.output_tail.clone();
                    if r.failed() {
                        result.status = "failed".into();
                        problem = Some(format!("a held-out check failed (exit {})", r.exit_code));
                        break;
                    }
                    result.status = "passed".into();
                    passed += 1;
                }
                Err(err) => {
                    result.status = "error".into();
                    result.output_tail = format!("{err:#}");
                    // A suite's error can name its image or where it is kept:
                    // people read it in the report, the author gets none of it.
                    problem = Some(match in_suite {
                        Some(_) => "the held-out checks could not run (the error is in the held-out report, for people)".into(),
                        None => format!("the held-out checks could not run: {err:#}"),
                    });
                    break;
                }
            }
        }
        match problem {
            Some(p) => step("held-out", StepStatus::Failed, vec![p]),
            None => step(
                "held-out",
                StepStatus::Passed,
                vec![format!("{passed} held-out checks passed")],
            ),
        }
    };
    if held_out.status == StepStatus::Failed {
        failed = Some("held-out");
    }
    steps.push(held_out);

    // 6. Taste, last and lightest, only when everything else passed.
    let taste = chosen_config
        .and_then(|j| j.taste.as_ref())
        .or(config.taste.as_ref());
    match (taste, failed) {
        (Some(taste), None) => steps.push(taste_step(&taste.command, sub)),
        (Some(_), Some(f)) => steps.push(step("taste", StepStatus::Skipped, not_reached(f))),
        (None, _) => {}
    }

    let failed: Vec<String> = steps
        .iter()
        .filter(|s| s.status == StepStatus::Failed)
        .flat_map(|s| s.details.iter().map(move |d| format!("{}: {d}", s.name)))
        .collect();
    let concerns: Vec<String> = steps
        .iter()
        .filter(|s| s.status == StepStatus::Concern)
        .flat_map(|s| s.details.clone())
        .collect();
    let lane = classification.lane;
    let (verdict, reasons) = if lane == Lane::Blocked {
        let mut r: Vec<String> = classification
            .applied
            .iter()
            .filter(|a| a.lane == Lane::Blocked)
            .map(|a| a.reason.clone())
            .collect();
        r.extend(failed);
        (Verdict::Reject, r)
    } else if !failed.is_empty() {
        (Verdict::Reject, failed)
    } else if lane >= Lane::Human || !concerns.is_empty() {
        let mut r: Vec<String> = classification
            .applied
            .iter()
            .filter(|a| a.lane >= Lane::Human)
            .map(|a| a.reason.clone())
            .collect();
        r.extend(concerns);
        (Verdict::Escalate, r)
    } else {
        (Verdict::Approve, vec![])
    };
    let short = suite.map_or(String::new(), |s| s.short().to_string());
    let summary = (suite.is_some() || legacy.is_some()).then(|| HeldOutSummary {
        suite: short.clone(),
        selected: selected.len() as u32,
        ran,
        passed,
    });
    let report = suite.map(|s| HeldOutReport {
        suite: short,
        judge: judge.clone(),
        reach: reach.clone(),
        checks: results,
        not_selected: heldout::not_selected(Some(&s.suite), &selected),
    });
    Judged {
        judgment: Judgment {
            verdict,
            lane,
            steps,
            reasons,
            judge,
            rotation: chosen.map(|(i, j, forced)| Rotation {
                config: j.name.clone(),
                index: i as u32 + 1,
                of: config.judges.len() as u32,
                forced,
            }),
            held_out: summary,
        },
        held_out: report,
    }
}

/// Runs the reviewer with the submission on stdin; `concern:` lines become
/// concerns. A failing or silent reviewer raises nothing and approves
/// nothing.
fn taste_step(command: &[String], sub: &Submission) -> Step {
    let Some((program, args)) = command.split_first() else {
        return step("taste", StepStatus::Skipped, vec!["no command".into()]);
    };
    let input = serde_json::to_vec(sub).unwrap_or_default();
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(&input);
            }
            child.wait_with_output()
        });
    match out {
        Ok(out) => {
            let concerns: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.trim().strip_prefix("concern:"))
                .map(|c| format!("taste: {}", c.trim()))
                .collect();
            if concerns.is_empty() {
                step("taste", StepStatus::Passed, vec![])
            } else {
                step("taste", StepStatus::Concern, concerns)
            }
        }
        Err(e) => step(
            "taste",
            StepStatus::Skipped,
            vec![format!("the reviewer could not run: {e}")],
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::*;
    use crate::classify::Applied;
    use crate::submission::{AgentSetup, Submission};
    use onus_core::{
        CheckCommand, HeldOutCheck, HeldOutSuite, ReportSummary, SemanticReport, StructureNotes,
        TasteCommand, TextStats,
    };

    fn run_ok(passes: bool) -> TestRun {
        run_exit(if passes { 0 } else { 1 })
    }

    fn run_exit(exit_code: i32) -> TestRun {
        TestRun {
            commit: "h".into(),
            image: "node:22".into(),
            setup: None,
            command: "npm test".into(),
            exit_code,
            output_tail: String::new(),
            manifest: None,
        }
    }

    fn submission() -> Submission {
        Submission {
            schema: 1,
            base: "b".into(),
            head: "0123456789abcdef".into(),
            intent: Some("touches: [notifications]".into()),
            report: SemanticReport {
                schema_version: 1,
                base: "b".into(),
                head: "h".into(),
                summary: ReportSummary {
                    meaning_changes: 0,
                    needs_attention: 0,
                    secrets: 0,
                    new_rule_violations: 0,
                    intent_mismatches: 0,
                },
                changes: vec![],
                intent_check: None,
                rule_violations: vec![],
                structure: StructureNotes::default(),
                text_stats: TextStats::default(),
                map_diagnostics: vec![],
                coverage: None,
            },
            changed_files: vec![],
            evidence: vec![run_ok(true)],
            scope: None,
            escalations: vec![],
            approvals: vec![],
            agent: AgentSetup::default(),
        }
    }

    fn lane(l: Lane) -> Classification {
        Classification {
            lane: l,
            applied: vec![Applied {
                lane: l,
                source: "default".into(),
                reason: "test".into(),
                rows: vec![],
            }],
            learned: None,
            features: None,
            rollout: None,
        }
    }

    /// A suite of checks covering `notifications`, as if read from /suite.
    fn suite(ids: &[&str]) -> LoadedSuite {
        LoadedSuite {
            suite: HeldOutSuite {
                version: 1,
                image: Some("alpine:3.20".into()),
                setup: None,
                limit: None,
                checks: ids
                    .iter()
                    .map(|id| HeldOutCheck {
                        id: id.to_string(),
                        command: format!("sh /held-out/{id}.sh"),
                        image: None,
                        setup: None,
                        covers: vec!["notifications".into(), "user-preferences".into()],
                    })
                    .collect(),
            },
            dir: PathBuf::from("/suite"),
            key: "3f9a0c12d4e5".repeat(5) + "abcd",
        }
    }

    fn touched() -> Reach {
        Reach {
            touched: BTreeSet::from(["notifications".to_string()]),
            ..Reach::default()
        }
    }

    #[test]
    fn verification_comes_first_and_taste_cannot_approve() {
        let config = LanesConfig {
            held_out: Some(CheckCommand {
                command: "npm run hidden".into(),
                image: None,
                setup: None,
            }),
            ..LanesConfig::default()
        };
        let passing = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str, _: Option<&Path>| {
            Ok(run_ok(true))
        };
        let failing_hidden =
            |_: &Path, _: &str, _: &str, _: Option<&str>, c: &str, _: Option<&Path>| {
                Ok(run_ok(!c.contains("hidden")))
            };
        let repo = Path::new(".");
        let ok = judge(&submission(), &lane(Lane::Judge), &config, repo, &passing);
        assert_eq!(ok.verdict, Verdict::Approve, "{ok:?}");
        // A held-out failure rejects without revealing the suite.
        let hidden = judge(
            &submission(),
            &lane(Lane::Judge),
            &config,
            repo,
            &failing_hidden,
        );
        assert_eq!(hidden.verdict, Verdict::Reject);
        assert_eq!(
            hidden.reasons,
            ["held-out: a held-out check failed (exit 1)"]
        );
        // Evidence that passed for the author must pass again.
        let flaky = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str, _: Option<&Path>| {
            Ok(run_ok(false))
        };
        let r = judge(
            &submission(),
            &lane(Lane::Judge),
            &LanesConfig::default(),
            repo,
            &flaky,
        );
        assert_eq!(r.verdict, Verdict::Reject);
        assert!(r.reasons[0].contains("passed for the author but fails"));
        // No evidence and no intent: the first thing to fix, and nothing after it.
        let mut bare = submission();
        bare.evidence.clear();
        bare.intent = None;
        let r = judge(
            &bare,
            &lane(Lane::Judge),
            &LanesConfig::default(),
            repo,
            &passing,
        );
        assert_eq!(r.reasons.len(), 1, "{:?}", r.reasons);
        assert!(r.reasons[0].contains("no test evidence"), "{:?}", r.reasons);
        assert_eq!(r.steps[1].status, StepStatus::Skipped);
        assert_eq!(r.steps[1].details, ["not reached: evidence failed"]);
        // The human lane escalates even when everything passes.
        let r = judge(
            &submission(),
            &lane(Lane::Human),
            &LanesConfig::default(),
            repo,
            &passing,
        );
        assert_eq!(r.verdict, Verdict::Escalate);
        // A reviewer's concern escalates; it never turns a rejection into approval.
        let taste = LanesConfig {
            taste: Some(TasteCommand {
                command: vec![
                    "sh".into(),
                    "-c".into(),
                    "cat >/dev/null; echo 'concern: naming'".into(),
                ],
            }),
            ..LanesConfig::default()
        };
        let r = judge(&submission(), &lane(Lane::Judge), &taste, repo, &passing);
        assert_eq!(r.verdict, Verdict::Escalate);
        assert_eq!(r.reasons, ["taste: naming"]);
    }

    #[test]
    fn stops_at_the_first_failure() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("tasted");
        let config = LanesConfig {
            held_out: Some(CheckCommand {
                command: "npm run hidden".into(),
                image: None,
                setup: None,
            }),
            taste: Some(TasteCommand {
                command: vec![
                    "sh".into(),
                    "-c".into(),
                    format!("cat >/dev/null; touch {}", marker.display()),
                ],
            }),
            ..LanesConfig::default()
        };
        let calls = Cell::new(0u32);
        let failing = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str, _: Option<&Path>| {
            calls.set(calls.get() + 1);
            Ok(run_ok(false))
        };
        let s = suite(&["one"]);
        let reach = touched();
        let j = judge_with(
            &submission(),
            &lane(Lane::Judge),
            &config,
            Path::new("."),
            &failing,
            &JudgeOptions {
                suite: Some(&s),
                reach: Some(&reach),
                ..JudgeOptions::default()
            },
        )
        .unwrap();
        // One container: the evidence. Nothing held out ran, no reviewer was asked.
        assert_eq!(calls.get(), 1);
        assert!(!marker.exists());
        let names: Vec<(&str, StepStatus)> = j
            .judgment
            .steps
            .iter()
            .map(|s| (s.name.as_str(), s.status))
            .collect();
        assert_eq!(
            names,
            [
                ("evidence", StepStatus::Failed),
                ("intent", StepStatus::Skipped),
                ("contracts", StepStatus::Skipped),
                ("tests", StepStatus::Skipped),
                ("held-out", StepStatus::Skipped),
                ("taste", StepStatus::Skipped),
            ]
        );
        assert!(
            j.judgment.steps[1..]
                .iter()
                .all(|s| s.details == ["not reached: evidence failed"])
        );
        assert_eq!(j.judgment.reasons.len(), 1, "{:?}", j.judgment.reasons);
        let summary = j.judgment.held_out.unwrap();
        assert_eq!((summary.selected, summary.ran), (2, 0));
        let report = j.held_out.unwrap();
        assert!(report.checks.iter().all(|c| c.status == "not-run"));
    }

    #[test]
    fn evidence_stops_at_the_first_failing_run() {
        let mut sub = submission();
        sub.evidence = ["a", "b", "c"]
            .iter()
            .map(|c| TestRun {
                command: c.to_string(),
                ..run_ok(true)
            })
            .collect();
        let calls = Cell::new(0u32);
        let second_fails =
            |_: &Path, _: &str, _: &str, _: Option<&str>, c: &str, _: Option<&Path>| {
                calls.set(calls.get() + 1);
                Ok(run_ok(c != "b"))
            };
        let j = judge(
            &sub,
            &lane(Lane::Judge),
            &LanesConfig::default(),
            Path::new("."),
            &second_fails,
        );
        assert_eq!(calls.get(), 2);
        assert_eq!(j.steps[0].status, StepStatus::Failed);
        assert!(j.steps[0].details[0].contains("`b` passed for the author but fails"));
        assert_eq!(j.steps[0].details[1], "1 more run not re-run");
    }

    #[test]
    fn held_out_checks_run_in_order_and_stop() {
        let s = suite(&["first", "second", "third"]);
        let reach = touched();
        let ran = RefCell::new(Vec::new());
        let second_fails =
            |_: &Path, _: &str, _: &str, _: Option<&str>, c: &str, _: Option<&Path>| {
                ran.borrow_mut().push(c.to_string());
                let mut r = run_exit(if c.contains("second") { 7 } else { 0 });
                r.output_tail = format!("output of {c}");
                Ok(r)
            };
        let j = judge_with(
            &submission(),
            &lane(Lane::Judge),
            &LanesConfig::default(),
            Path::new("."),
            &second_fails,
            &JudgeOptions {
                suite: Some(&s),
                reach: Some(&reach),
                ..JudgeOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            *ran.borrow(),
            [
                "npm test",
                "sh /held-out/first.sh",
                "sh /held-out/second.sh"
            ]
        );
        assert_eq!(j.judgment.verdict, Verdict::Reject);
        assert_eq!(
            j.judgment.reasons,
            ["held-out: a held-out check failed (exit 7)"]
        );
        let summary = j.judgment.held_out.clone().unwrap();
        assert_eq!(
            summary,
            HeldOutSummary {
                suite: "3f9a0c12d4e5".into(),
                selected: 3,
                ran: 2,
                passed: 1,
            }
        );
        let report = j.held_out.unwrap();
        let statuses: Vec<(&str, &str)> = report
            .checks
            .iter()
            .map(|c| (c.id.as_str(), c.status.as_str()))
            .collect();
        assert_eq!(
            statuses,
            [
                ("first", "passed"),
                ("second", "failed"),
                ("third", "not-run")
            ]
        );
        assert_eq!(report.checks[1].exit_code, Some(7));
        assert_eq!(
            report.checks[1].output_tail,
            "output of sh /held-out/second.sh"
        );
        assert_eq!(report.judge, j.judgment.judge);
        // With every check passing, the step says how many.
        let passing = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str, _: Option<&Path>| {
            Ok(run_ok(true))
        };
        let j = judge_with(
            &submission(),
            &lane(Lane::Judge),
            &LanesConfig::default(),
            Path::new("."),
            &passing,
            &JudgeOptions {
                suite: Some(&s),
                reach: Some(&reach),
                ..JudgeOptions::default()
            },
        )
        .unwrap();
        assert_eq!(j.judgment.steps[4].details, ["3 held-out checks passed"]);
        // A change no check covers skips the step.
        let elsewhere = Reach {
            touched: BTreeSet::from(["billing".to_string()]),
            ..Reach::default()
        };
        let j = judge_with(
            &submission(),
            &lane(Lane::Judge),
            &LanesConfig::default(),
            Path::new("."),
            &passing,
            &JudgeOptions {
                suite: Some(&s),
                reach: Some(&elsewhere),
                ..JudgeOptions::default()
            },
        )
        .unwrap();
        assert_eq!(j.judgment.steps[4].status, StepStatus::Skipped);
        assert_eq!(
            j.judgment.steps[4].details,
            ["no held-out check covers this change"]
        );
        assert_eq!(j.held_out.unwrap().not_selected.len(), 3);
    }

    #[test]
    fn the_judgment_never_names_a_held_out_check() {
        let s = suite(&["sms-respects-opt-out", "money-in-receipts"]);
        let reach = touched();
        for exit in [0, 1] {
            let run = |_: &Path, _: &str, _: &str, _: Option<&str>, c: &str, _: Option<&Path>| {
                let mut r = run_exit(if c.contains("money") { exit } else { 0 });
                r.output_tail = "SECRET-OUTPUT".into();
                Ok(r)
            };
            let j = judge_with(
                &submission(),
                &lane(Lane::Judge),
                &LanesConfig::default(),
                Path::new("."),
                &run,
                &JudgeOptions {
                    suite: Some(&s),
                    reach: Some(&reach),
                    ..JudgeOptions::default()
                },
            )
            .unwrap();
            let text = serde_json::to_string(&j.judgment).unwrap();
            for secret in [
                "sms-respects-opt-out",
                "money-in-receipts",
                "/held-out/",
                "user-preferences",
                "SECRET-OUTPUT",
            ] {
                assert!(!text.contains(secret), "{secret} in {text}");
            }
            // People read it all.
            let report = serde_json::to_string(&j.held_out.unwrap()).unwrap();
            assert!(report.contains("money-in-receipts") && report.contains("SECRET-OUTPUT"));
        }
        // An engine error can name the suite's image or where it is kept.
        let broken = |_: &Path, _: &str, _: &str, _: Option<&str>, c: &str, x: Option<&Path>| {
            if x.is_some() {
                anyhow::bail!("cannot pull registry.invalid/secret-image:1 for /keep/suite")
            }
            Ok(run_ok(!c.is_empty()))
        };
        let j = judge_with(
            &submission(),
            &lane(Lane::Judge),
            &LanesConfig::default(),
            Path::new("."),
            &broken,
            &JudgeOptions {
                suite: Some(&s),
                reach: Some(&reach),
                ..JudgeOptions::default()
            },
        )
        .unwrap();
        let text = serde_json::to_string(&j.judgment).unwrap();
        assert!(!text.contains("secret-image") && !text.contains("/keep/suite"));
        assert_eq!(j.judgment.steps[4].status, StepStatus::Failed);
        let report = j.held_out.unwrap();
        assert_eq!(report.checks[0].status, "error");
        assert!(report.checks[0].output_tail.contains("secret-image"));
    }

    #[test]
    fn only_suite_checks_get_the_suite() {
        let config = LanesConfig {
            held_out: Some(CheckCommand {
                command: "npm run hidden".into(),
                image: Some("node:22-alpine".into()),
                setup: None,
            }),
            ..LanesConfig::default()
        };
        let s = suite(&["one"]);
        let reach = touched();
        let seen = RefCell::new(Vec::new());
        let run =
            |_: &Path, _: &str, image: &str, _: Option<&str>, c: &str, extra: Option<&Path>| {
                seen.borrow_mut().push((
                    c.to_string(),
                    image.to_string(),
                    extra.map(Path::to_path_buf),
                ));
                Ok(run_ok(true))
            };
        let j = judge_with(
            &submission(),
            &lane(Lane::Judge),
            &config,
            Path::new("."),
            &run,
            &JudgeOptions {
                suite: Some(&s),
                reach: Some(&reach),
                ..JudgeOptions::default()
            },
        )
        .unwrap();
        assert_eq!(j.judgment.verdict, Verdict::Approve, "{:?}", j.judgment);
        assert_eq!(
            *seen.borrow(),
            [
                ("npm test".into(), "node:22".into(), None),
                (
                    "sh /held-out/one.sh".into(),
                    "alpine:3.20".into(),
                    Some(PathBuf::from("/suite"))
                ),
                ("npm run hidden".into(), "node:22-alpine".into(), None),
            ]
        );
    }

    #[test]
    fn rotation_is_recorded_and_its_taste_used() {
        let reviewer = |says: &str| TasteCommand {
            command: vec![
                "sh".into(),
                "-c".into(),
                format!("cat >/dev/null; echo 'concern: {says}'"),
            ],
        };
        let config = LanesConfig {
            taste: Some(reviewer("default")),
            judges: vec![
                JudgeConfig {
                    name: "first".into(),
                    taste: Some(reviewer("first")),
                    held_out_limit: None,
                },
                JudgeConfig {
                    name: "second".into(),
                    taste: Some(reviewer("second")),
                    held_out_limit: None,
                },
            ],
            ..LanesConfig::default()
        };
        let passing = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str, _: Option<&Path>| {
            Ok(run_ok(true))
        };
        let mut seen = BTreeSet::new();
        for i in 0..16 {
            let mut sub = submission();
            sub.head = format!("{i:040x}");
            let j = judge_with(
                &sub,
                &lane(Lane::Judge),
                &config,
                Path::new("."),
                &passing,
                &JudgeOptions::default(),
            )
            .unwrap()
            .judgment;
            let rotation = j.rotation.clone().unwrap();
            assert_eq!(rotation.of, 2);
            assert!(!rotation.forced);
            assert_eq!(j.reasons, [format!("taste: {}", rotation.config)]);
            assert!(
                j.judge.starts_with(&format!("{}@", rotation.config)),
                "{}",
                j.judge
            );
            assert_eq!(
                j.judge,
                judge_id_for(&config, Some(&config.judges[rotation.index as usize - 1]))
            );
            seen.insert(rotation.config);
        }
        assert_eq!(seen.len(), 2, "{seen:?}");
        // A person can force one, and an unknown name is an error.
        let forced = |name: &str| {
            judge_with(
                &submission(),
                &lane(Lane::Judge),
                &config,
                Path::new("."),
                &passing,
                &JudgeOptions {
                    judge_config: Some(name),
                    ..JudgeOptions::default()
                },
            )
        };
        let j = forced("second").unwrap().judgment;
        assert_eq!(
            j.rotation,
            Some(Rotation {
                config: "second".into(),
                index: 2,
                of: 2,
                forced: true,
            })
        );
        assert!(forced("third").is_err());
    }

    #[test]
    fn judge_id_is_unchanged_without_judges() {
        let c = LanesConfig::default();
        assert_eq!(judge_id_for(&c, None), judge_id(&c));
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("judges"), "{json}");
        // Each configuration has its own id, named.
        let a = JudgeConfig {
            name: "a".into(),
            taste: None,
            held_out_limit: None,
        };
        let b = JudgeConfig {
            name: "b".into(),
            ..a.clone()
        };
        let both = LanesConfig {
            judges: vec![a.clone(), b.clone()],
            ..c.clone()
        };
        let (ida, idb) = (judge_id_for(&both, Some(&a)), judge_id_for(&both, Some(&b)));
        assert!(ida.starts_with("a@") && idb.starts_with("b@"));
        assert_ne!(ida[2..], idb[2..]);
        // A judgment without rotation or held-out checks reads as before.
        let j = judge(
            &submission(),
            &lane(Lane::Judge),
            &c,
            Path::new("."),
            &|_: &Path, _: &str, _: &str, _: Option<&str>, _: &str, _: Option<&Path>| {
                Ok(run_ok(true))
            },
        );
        let text = serde_json::to_string(&j).unwrap();
        assert!(
            !text.contains("\"rotation\"") && !text.contains("\"heldOut\""),
            "{text}"
        );
    }

    #[test]
    fn runs_that_name_a_manifest_must_match_it() {
        use onus_env::store::{EnvironmentInfo, Manifest};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let head = submission().head;
        let manifest = |commit: &str, exit_code: i32| Manifest {
            schema: 1,
            commit: commit.into(),
            command: "npm test".into(),
            exit_code,
            started_at: 1,
            finished_at: 2,
            environment: EnvironmentInfo {
                name: "t".into(),
                image: "node:22".into(),
                warm_image: None,
                setup: None,
                seed: None,
                hosts: vec![],
                secrets: vec![],
                task: None,
            },
            artifacts: vec![],
            tests: None,
        };
        let passing = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str, _: Option<&Path>| {
            Ok(run_ok(true))
        };
        let with = |id: String| {
            let mut sub = submission();
            sub.evidence[0].commit = head.clone();
            sub.evidence[0].manifest = Some(id);
            sub
        };
        let config = LanesConfig::default();
        let repo = Path::new(".");
        let judged = |sub: &Submission, store: Option<&Store>| {
            judge_with(
                sub,
                &lane(Lane::Judge),
                &config,
                repo,
                &passing,
                &JudgeOptions {
                    store,
                    ..JudgeOptions::default()
                },
            )
            .unwrap()
            .judgment
        };

        let good = store.record(&manifest(&head, 0)).unwrap();
        let j = judged(&with(good.clone()), Some(&store));
        assert_eq!(j.verdict, Verdict::Approve, "{j:?}");
        assert!(
            j.steps[0].details[1].contains("matched their manifests"),
            "{j:?}"
        );

        // The run failed in the environment, but the claim says it passed.
        let failed = store.record(&manifest(&head, 1)).unwrap();
        let j = judged(&with(failed), Some(&store));
        assert_eq!(j.verdict, Verdict::Reject);
        assert!(j.reasons[0].contains("does not match manifest"), "{j:?}");

        // A manifest of another commit, or none at all.
        let other = store.record(&manifest("feedface", 0)).unwrap();
        let j = judged(&with(other), Some(&store));
        assert!(j.reasons[0].contains("not at the head commit"), "{j:?}");
        let j = judged(&with("0".repeat(64)), Some(&store));
        assert!(j.reasons[0].contains("not in the evidence store"), "{j:?}");
        let j = judged(&with(good), None);
        assert!(j.reasons[0].contains("no evidence store"), "{j:?}");
    }
}
