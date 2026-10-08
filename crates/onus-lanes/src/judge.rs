//! The verifying judge: verification first, taste last.
//!
//! 1. Re-run the submission's test evidence in a fresh container.
//! 2. Compare intent with effect.
//! 3. Check contracts and boundary rules.
//! 4. Check for weakened tests.
//! 5. Run held-out checks the author never saw.
//! 6. Taste, optionally: a reviewer command that can raise concerns, never
//!    approve.
//!
//! The judge reads the submission (the report, the evidence, the scope),
//! never the author's reasoning. The verdict is `approve`, `reject` with
//! reasons the agent can act on, or `escalate` to a person.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Result;
use onus_core::{ChangeKind, Lane, LanesConfig};
use onus_doors::runner::TestRun;
use onus_env::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::classify::Classification;
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
    /// Onus version, for outcome tracking and rotation.
    pub judge: String,
}

/// Runs a command at a commit in a container: `(repo, commit, image,
/// setup, command)`. The real one is [`onus_doors::runner::run`].
pub type Runner<'a> = &'a dyn Fn(&Path, &str, &str, Option<&str>, &str) -> Result<TestRun>;

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
    judge_with(sub, classification, config, repo, run, None)
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

/// [`judge`], also checking test runs that name a manifest against the
/// evidence store.
pub fn judge_with(
    sub: &Submission,
    classification: &Classification,
    config: &LanesConfig,
    repo: &Path,
    run: Runner,
    store: Option<&Store>,
) -> Judgment {
    let mut steps = Vec::new();
    let report = &sub.report;

    // 1. Evidence, re-run where the author cannot influence it.
    if sub.evidence.is_empty() {
        steps.push(step(
            "evidence",
            StepStatus::Failed,
            vec!["no test evidence: run the tests that cover the change with `onus run-test` and include the runs".into()],
        ));
    } else {
        let mut failed = Vec::new();
        let mut recorded = 0;
        for e in &sub.evidence {
            if e.failed() {
                failed.push(format!("`{}` failed when the author ran it", e.command));
                continue;
            }
            if let Some(problem) = manifest_problem(e, &sub.head, store) {
                failed.push(problem);
                continue;
            }
            if e.manifest.is_some() {
                recorded += 1;
            }
            match run(repo, &sub.head, &e.image, e.setup.as_deref(), &e.command) {
                Ok(again) if !again.failed() => {}
                Ok(again) => failed.push(format!(
                    "`{}` passed for the author but fails at {} (exit {})",
                    e.command,
                    &sub.head[..sub.head.len().min(12)],
                    again.exit_code
                )),
                Err(err) => failed.push(format!("`{}` could not be re-run: {err:#}", e.command)),
            }
        }
        steps.push(if failed.is_empty() {
            step(
                "evidence",
                StepStatus::Passed,
                std::iter::once(format!("{} test runs passed again", sub.evidence.len()))
                    .chain((recorded > 0).then(|| {
                        format!("{recorded} matched their manifests in the evidence store")
                    }))
                    .collect(),
            )
        } else {
            step("evidence", StepStatus::Failed, failed)
        });
    }

    // 2. Intent against effect.
    if sub.intent.as_deref().is_none_or(|i| i.trim().is_empty()) {
        steps.push(step(
            "intent",
            StepStatus::Failed,
            vec![
                "no stated intent: add an `onus-intent` block saying what the change touches"
                    .into(),
            ],
        ));
    } else {
        let outside: Vec<String> = report
            .changes
            .iter()
            .filter(|r| r.hints.intent_mismatch)
            .map(|r| format!("`{}` is outside the stated intent", r.title))
            .collect();
        steps.push(if outside.is_empty() {
            step("intent", StepStatus::Passed, vec![])
        } else {
            step("intent", StepStatus::Failed, outside)
        });
    }

    // 3. Contracts and boundary rules.
    let broken: Vec<String> = report
        .changes
        .iter()
        .filter(|r| r.kind == ChangeKind::Breaking || r.subkind == "rule-violation")
        .map(|r| format!("{}: {}", r.title, r.why_it_matters))
        .collect();
    steps.push(if broken.is_empty() {
        step("contracts", StepStatus::Passed, vec![])
    } else {
        step("contracts", StepStatus::Failed, broken)
    });

    // 4. Weakened tests.
    let weakened: Vec<String> = report
        .changes
        .iter()
        .filter(|r| r.subkind == "test-weakened" && !sub.approved(&r.id))
        .map(|r| format!("{}: {}", r.title, r.why_it_matters))
        .collect();
    steps.push(if weakened.is_empty() {
        step("tests", StepStatus::Passed, vec![])
    } else {
        step("tests", StepStatus::Failed, weakened)
    });

    // 5. Held-out checks. Their output is not shown: the author must not
    // learn the hidden suite from the verdict.
    match &config.held_out {
        None => steps.push(step(
            "held-out",
            StepStatus::Skipped,
            vec!["no held-out checks configured (lanes.heldOut)".into()],
        )),
        Some(check) => {
            let image = check.image.as_deref().unwrap_or("node:22");
            steps.push(
                match run(
                    repo,
                    &sub.head,
                    image,
                    check.setup.as_deref(),
                    &check.command,
                ) {
                    Ok(r) if !r.failed() => step("held-out", StepStatus::Passed, vec![]),
                    Ok(r) => step(
                        "held-out",
                        StepStatus::Failed,
                        vec![format!("a held-out check failed (exit {})", r.exit_code)],
                    ),
                    Err(err) => step(
                        "held-out",
                        StepStatus::Failed,
                        vec![format!("the held-out checks could not run: {err:#}")],
                    ),
                },
            );
        }
    }

    // 6. Taste, last and lightest, only when everything else passed.
    let verified = steps.iter().all(|s| s.status != StepStatus::Failed);
    match (&config.taste, verified) {
        (Some(taste), true) => steps.push(taste_step(&taste.command, sub)),
        (Some(_), false) => steps.push(step(
            "taste",
            StepStatus::Skipped,
            vec!["not reached: verification failed".into()],
        )),
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
    Judgment {
        verdict,
        lane,
        steps,
        reasons,
        judge: judge_id(config),
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
    use super::*;
    use crate::classify::Applied;
    use crate::submission::{AgentSetup, Submission};
    use onus_core::{CheckCommand, ReportSummary, SemanticReport, StructureNotes, TextStats};

    fn run_ok(passes: bool) -> TestRun {
        TestRun {
            commit: "h".into(),
            image: "node:22".into(),
            setup: None,
            command: "npm test".into(),
            exit_code: if passes { 0 } else { 1 },
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
            }],
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
        let passing = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str| Ok(run_ok(true));
        let failing_hidden = |_: &Path, _: &str, _: &str, _: Option<&str>, c: &str| {
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
        let flaky = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str| Ok(run_ok(false));
        let r = judge(
            &submission(),
            &lane(Lane::Judge),
            &LanesConfig::default(),
            repo,
            &flaky,
        );
        assert_eq!(r.verdict, Verdict::Reject);
        assert!(r.reasons[0].contains("passed for the author but fails"));
        // No evidence and no intent are rejections the agent can act on.
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
        assert_eq!(r.reasons.len(), 2, "{:?}", r.reasons);
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
            taste: Some(onus_core::TasteCommand {
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
            },
            artifacts: vec![],
            tests: None,
        };
        let passing = |_: &Path, _: &str, _: &str, _: Option<&str>, _: &str| Ok(run_ok(true));
        let with = |id: String| {
            let mut sub = submission();
            sub.evidence[0].commit = head.clone();
            sub.evidence[0].manifest = Some(id);
            sub
        };
        let config = LanesConfig::default();
        let repo = Path::new(".");

        let good = store.record(&manifest(&head, 0)).unwrap();
        let j = judge_with(
            &with(good.clone()),
            &lane(Lane::Judge),
            &config,
            repo,
            &passing,
            Some(&store),
        );
        assert_eq!(j.verdict, Verdict::Approve, "{j:?}");
        assert!(
            j.steps[0].details[1].contains("matched their manifests"),
            "{j:?}"
        );

        // The run failed in the environment, but the claim says it passed.
        let failed = store.record(&manifest(&head, 1)).unwrap();
        let j = judge_with(
            &with(failed),
            &lane(Lane::Judge),
            &config,
            repo,
            &passing,
            Some(&store),
        );
        assert_eq!(j.verdict, Verdict::Reject);
        assert!(j.reasons[0].contains("does not match manifest"), "{j:?}");

        // A manifest of another commit, or none at all.
        let other = store.record(&manifest("feedface", 0)).unwrap();
        let j = judge_with(
            &with(other),
            &lane(Lane::Judge),
            &config,
            repo,
            &passing,
            Some(&store),
        );
        assert!(j.reasons[0].contains("not at the head commit"), "{j:?}");
        let j = judge_with(
            &with("0".repeat(64)),
            &lane(Lane::Judge),
            &config,
            repo,
            &passing,
            Some(&store),
        );
        assert!(j.reasons[0].contains("not in the evidence store"), "{j:?}");
        let j = judge_with(
            &with(good),
            &lane(Lane::Judge),
            &config,
            repo,
            &passing,
            None,
        );
        assert!(j.reasons[0].contains("no evidence store"), "{j:?}");
    }
}
