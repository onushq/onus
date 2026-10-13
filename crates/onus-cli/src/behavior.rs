//! `onus report --behavior` (ADR 0019): runs the tests the maps link to the
//! changed code at the base and at the head, each in its own environment,
//! runs again at the head what newly fails there, and runs the base versions
//! of edited tests against the head's code. `onus_diff::behavior` compares
//! the cases and writes the rows.
//!
//! Nothing here fails the report: every problem becomes a `skipped` or
//! `partial` status with a reason.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use onus_core::{
    BehaviorRun, BehaviorSideRun, BehaviorStatus, CodebaseMap, JunitCase, RunSide, SelectedTest,
    SemanticChange,
};
use onus_diff::behavior::{Observed, retry_files, rows};
use onus_env::env::{self, Grants, Ran, RunOptions};
use onus_env::spec::Spec;
use onus_env::store::{ArtifactKind, Overlay, Store, junit_cases};
use onus_index::{MapIndex, Reach};
use onus_map::LoadedConfig;

use crate::MaterializedPair;

pub const DEFAULT_BUDGET: u32 = 600;
pub const DEFAULT_MAX_FILES: u32 = 40;
pub const DEFAULT_DEPTH: u32 = 1;
pub const DEFAULT_JUNIT: &str = "reports/onus-behavior*.xml";

/// What the behavior diff needs from `onus report`.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    pub repo: &'a Path,
    pub base_sha: &'a str,
    pub head_sha: &'a str,
    /// The two trees and the paths that differ.
    pub pair: &'a MaterializedPair,
    /// The config the report used: the base tree's, or `--config`.
    pub config: Option<&'a LoadedConfig>,
    pub base_map: &'a CodebaseMap,
    pub head_map: &'a CodebaseMap,
    /// `--behavior-budget`.
    pub budget_override: Option<u32>,
}

/// The rows to add to the report, what ran, and one line for stderr.
#[derive(Debug, Clone)]
pub struct Output {
    pub rows: Vec<SemanticChange>,
    pub run: BehaviorRun,
    pub note: String,
}

/// Runs the behavior diff. It never fails: a problem is the run's status.
pub fn run(input: &Input) -> Output {
    let start = Instant::now();
    let config = input
        .config
        .and_then(|c| c.config.environment.as_ref())
        .and_then(|e| e.behavior.as_ref());
    let budget = input
        .budget_override
        .or(config.and_then(|b| b.budget))
        .unwrap_or(DEFAULT_BUDGET);
    let mut run = BehaviorRun {
        status: BehaviorStatus::Complete,
        reason: None,
        command: config.map(|b| b.command.clone()).unwrap_or_default(),
        budget_secs: budget,
        elapsed_secs: 0,
        selected: Vec::new(),
        truncated: 0,
        runs: Vec::new(),
        flaky: Vec::new(),
        already_failing: Vec::new(),
    };
    let skipped = |mut run: BehaviorRun, reason: String| -> Output {
        run.status = BehaviorStatus::Skipped;
        run.elapsed_secs = start.elapsed().as_secs() as u32;
        let note = format!("skipped: {reason}");
        run.reason = Some(reason);
        Output {
            rows: Vec::new(),
            run,
            note,
        }
    };
    let Some(config) = config else {
        return skipped(
            run,
            "onus.yaml has no environment.behavior (the command that runs the selected tests)"
                .into(),
        );
    };
    if !config.command.contains("{files}") {
        return skipped(
            run,
            "environment.behavior.command has no {files}, so it cannot be given the selected tests"
                .into(),
        );
    }

    let (selected, truncated) = select(
        input,
        config.depth.unwrap_or(DEFAULT_DEPTH),
        config.max_files.unwrap_or(DEFAULT_MAX_FILES),
    );
    run.selected = selected;
    run.truncated = truncated;
    if run.selected.is_empty() {
        run.elapsed_secs = start.elapsed().as_secs() as u32;
        return Output {
            rows: Vec::new(),
            run,
            note: "no tests reach the changed code".into(),
        };
    }
    if let Err(e) = onus_doors::runner::engine() {
        return skipped(run, format!("{e:#}"));
    }
    // The environment comes from the base tree, as the config does.
    if let Err(e) = input.pair.complete_base() {
        return skipped(run, format!("cannot write the base tree: {e:#}"));
    }
    let env_config = input.config.and_then(|c| c.config.environment.as_ref());
    let mut spec = match onus_env::spec::resolve(input.pair.base.dir.path(), env_config) {
        Ok(s) => s,
        Err(e) => return skipped(run, format!("no image: {e:#}")),
    };
    let junit: Vec<String> = if config.junit.is_empty() {
        vec![DEFAULT_JUNIT.to_string()]
    } else {
        config.junit.clone()
    };
    spec.evidence.extend(junit.iter().cloned());
    let store = match Store::for_repo(input.repo) {
        Ok(s) => s,
        Err(e) => return skipped(run, format!("no evidence store: {e:#}")),
    };

    let deadline = start + Duration::from_secs(u64::from(budget));
    let ctx = Ctx {
        command: &config.command,
        junit: &junit,
        store: &store,
        spec: &spec,
        deadline,
        budget,
        repo: input.repo,
    };
    let short = &input.head_sha[..input.head_sha.len().min(10)];
    let base_env = format!("bhv-{short}-{}-b", std::process::id());
    let head_env = format!("bhv-{short}-{}-h", std::process::id());
    let envs = Destroy(vec![base_env.clone(), head_env.clone()]);
    let files = |at_base: bool| -> Vec<String> {
        run.selected
            .iter()
            .filter(|s| if at_base { s.base } else { s.head })
            .map(|s| s.file.clone())
            .collect()
    };
    let (base_files, head_files) = (files(true), files(false));

    let mut problems: Vec<String> = Vec::new();
    let (base, head) = std::thread::scope(|scope| {
        let b = scope.spawn(|| ctx.side(&base_env, RunSide::Base, input.base_sha, &base_files));
        let h = scope.spawn(|| ctx.side(&head_env, RunSide::Head, input.head_sha, &head_files));
        (
            b.join()
                .unwrap_or_else(|_| Side::failed("the base run panicked")),
            h.join()
                .unwrap_or_else(|_| Side::failed("the head run panicked")),
        )
    });
    // Retrying and the cross run happen in the head's environment.
    let head_ready = head.ran_in_env;
    for side in [&base, &head] {
        problems.extend(side.problem.clone());
        run.runs.extend(side.run.clone());
    }
    let mut observed = Observed {
        base: base.cases,
        head: head.cases,
        ..Observed::default()
    };

    if observed.base.is_some() && observed.head.is_some() && head_ready {
        let retry: Vec<String> = retry_files(
            &observed,
            input.base_map,
            input.head_map,
            &files_of(&run.selected),
        )
        .into_iter()
        .filter(|f| head_files.contains(f))
        .collect();
        if !retry.is_empty() {
            let step = ctx.step(&head_env, RunSide::Retry, input.head_sha, &retry, None);
            problems.extend(step.problem);
            run.runs.extend(step.run);
            observed.retry = step.cases;
        }
        let cross: Vec<String> = run
            .selected
            .iter()
            .filter(|s| s.base && s.head && input.pair.changed.contains(&s.file))
            .map(|s| s.file.clone())
            .collect();
        if !cross.is_empty() {
            match overlay_base(input, &head_env, &cross) {
                Ok(paths) => {
                    let overlay = Overlay {
                        from_commit: input.base_sha.to_string(),
                        paths,
                    };
                    let step = ctx.step(
                        &head_env,
                        RunSide::Cross,
                        input.head_sha,
                        &cross,
                        Some(overlay),
                    );
                    problems.extend(step.problem);
                    run.runs.extend(step.run);
                    observed.cross = step.cases;
                }
                Err(e) => problems.push(format!(
                    "cannot write the base tests for the cross run: {e:#}"
                )),
            }
        }
    }
    drop(envs);

    let compared = rows(
        &observed,
        input.base_map,
        input.head_map,
        &files_of(&run.selected),
    );
    run.flaky = compared.flaky;
    run.already_failing = compared.already_failing;
    if !problems.is_empty() {
        run.status = BehaviorStatus::Partial;
        run.reason = Some(problems.join("; "));
    }
    run.elapsed_secs = start.elapsed().as_secs() as u32;
    let note = note(&run);
    Output {
        rows: compared.rows,
        run,
        note,
    }
}

fn files_of(selected: &[SelectedTest]) -> Vec<String> {
    selected.iter().map(|s| s.file.clone()).collect()
}

/// The test files that reach the change in either map, changed tests
/// first, at most `max_files`; and how many more there were.
fn select(input: &Input, depth: u32, max_files: u32) -> (Vec<SelectedTest>, u32) {
    let changed: Vec<String> = input.pair.changed.iter().cloned().collect();
    let mut best: BTreeMap<String, Reach> = BTreeMap::new();
    for map in [input.head_map, input.base_map] {
        for mut r in MapIndex::new(map.clone()).tests_reaching(&changed, depth) {
            match best.get_mut(&r.file) {
                Some(b) if (!b.changed, b.depth, &b.via) <= (!r.changed, r.depth, &r.via) => {
                    if b.via.is_none() {
                        b.via = r.via;
                    }
                }
                Some(b) => {
                    r.via = r.via.or(b.via.take());
                    *b = r;
                }
                None => {
                    best.insert(r.file.clone(), r);
                }
            }
        }
    }
    let mut all: Vec<Reach> = best.into_values().collect();
    all.sort_by(|a, b| (!a.changed, a.depth, &a.file).cmp(&(!b.changed, b.depth, &b.file)));
    let truncated = all.len().saturating_sub(max_files as usize) as u32;
    all.truncate(max_files as usize);
    let tests = |map: &CodebaseMap| -> BTreeSet<String> {
        map.tests.iter().map(|t| t.file.clone()).collect()
    };
    let (at_base, at_head) = (tests(input.base_map), tests(input.head_map));
    let selected = all
        .into_iter()
        .map(|r| SelectedTest {
            base: at_base.contains(&r.file),
            head: at_head.contains(&r.file),
            file: r.file,
            changed: r.changed,
            depth: r.depth,
            via: r.via,
        })
        .collect();
    (selected, truncated)
}

/// Writes the base versions of `files`, and of their changed snapshot
/// files, over the head environment's tree. Returns the paths written.
fn overlay_base(input: &Input, env_name: &str, files: &[String]) -> anyhow::Result<Vec<String>> {
    let base = input.pair.base.dir.path();
    let mut written: Vec<(String, Vec<u8>)> = Vec::new();
    for f in files {
        written.push((f.clone(), std::fs::read(base.join(f))?));
        // Jest's and vitest's snapshots: `dir/__snapshots__/name.snap`.
        let (dir, name) = f.rsplit_once('/').map_or(("", f.as_str()), |(d, n)| (d, n));
        let snap = if dir.is_empty() {
            format!("__snapshots__/{name}.snap")
        } else {
            format!("{dir}/__snapshots__/{name}.snap")
        };
        if input.pair.changed.contains(&snap)
            && let Ok(bytes) = std::fs::read(base.join(&snap))
        {
            written.push((snap, bytes));
        }
    }
    env::overlay(env_name, &written)?;
    Ok(written.into_iter().map(|(p, _)| p).collect())
}

/// Destroys the environments when dropped, also after an error or a
/// timeout, which leaves the command running in them.
struct Destroy(Vec<String>);

impl Drop for Destroy {
    fn drop(&mut self) {
        for name in &self.0 {
            let _ = env::destroy(name);
        }
    }
}

/// What every run shares.
struct Ctx<'a> {
    command: &'a str,
    junit: &'a [String],
    store: &'a Store,
    spec: &'a Spec,
    deadline: Instant,
    budget: u32,
    repo: &'a Path,
}

/// One side's environment and run.
struct Side {
    problem: Option<String>,
    run: Option<BehaviorSideRun>,
    /// None when there is nothing to compare.
    cases: Option<Vec<JunitCase>>,
    ran_in_env: bool,
}

impl Side {
    fn failed(problem: &str) -> Side {
        Side {
            problem: Some(problem.into()),
            run: None,
            cases: None,
            ran_in_env: false,
        }
    }
}

impl Ctx<'_> {
    fn out_of_budget(&self, before: &str) -> Option<String> {
        (Instant::now() >= self.deadline)
            .then(|| format!("the budget of {}s ran out before {before}", self.budget))
    }

    /// Creates the side's environment and runs its test files there. A side
    /// with no test files runs nothing and has no cases.
    fn side(&self, name: &str, side: RunSide, commit: &str, files: &[String]) -> Side {
        if files.is_empty() {
            return Side {
                problem: None,
                run: None,
                cases: Some(Vec::new()),
                ran_in_env: false,
            };
        }
        if let Some(p) =
            self.out_of_budget(&format!("the {} environment was created", side.as_str()))
        {
            return Side::failed(&p);
        }
        if let Err(e) = env::create(self.repo, commit, name, self.spec, &Grants::default()) {
            return Side::failed(&format!(
                "cannot create the {} environment: {e:#}",
                side.as_str()
            ));
        }
        let step = self.step(name, side, commit, files, None);
        Side {
            problem: step.problem,
            run: step.run,
            cases: step.cases,
            ran_in_env: true,
        }
    }

    /// Runs the command on `files` in the environment and reads its JUnit.
    fn step(
        &self,
        name: &str,
        side: RunSide,
        commit: &str,
        files: &[String],
        overlay: Option<Overlay>,
    ) -> Side {
        let what = side.as_str();
        if let Some(p) = self.out_of_budget(&format!("the {what} run")) {
            return Side::failed(&p);
        }
        // Earlier JUnit files go first, so this run's are the ones recorded.
        let stale: Vec<&str> = self
            .junit
            .iter()
            .map(String::as_str)
            .filter(|g| plain_glob(g))
            .collect();
        if !stale.is_empty() {
            let _ = env::exec(name, &format!("rm -f -- {}", stale.join(" ")));
        }
        let command = self.command.replace(
            "{files}",
            &files.iter().map(|f| quote(f)).collect::<Vec<_>>().join(" "),
        );
        let options = RunOptions {
            timeout: Some(self.deadline.saturating_duration_since(Instant::now())),
            overlay,
        };
        let empty = |problem: String, run: Option<BehaviorSideRun>| Side {
            problem: Some(problem),
            run,
            cases: None,
            ran_in_env: true,
        };
        let (id, manifest) = match env::run_with(name, &command, self.store, &options) {
            Err(e) => return empty(format!("the {what} run failed: {e:#}"), None),
            Ok(Ran::TimedOut) => {
                return empty(
                    format!(
                        "the budget of {}s ran out during the {what} run",
                        self.budget
                    ),
                    Some(BehaviorSideRun {
                        side,
                        commit: commit.to_string(),
                        files: files.len() as u32,
                        manifest: None,
                        exit_code: None,
                        timed_out: true,
                        cases: 0,
                        failed: 0,
                        skipped: 0,
                    }),
                );
            }
            Ok(Ran::Done(id, manifest)) => (id, manifest),
        };
        let mut cases = Vec::new();
        let mut found = false;
        for a in manifest
            .artifacts
            .iter()
            .filter(|a| a.kind == ArtifactKind::Junit)
        {
            if let Ok(bytes) = self.store.get(&a.sha256) {
                found = true;
                cases.extend(junit_cases(&String::from_utf8_lossy(&bytes)));
            }
        }
        let run = BehaviorSideRun {
            side,
            commit: commit.to_string(),
            files: files.len() as u32,
            manifest: Some(id.clone()),
            exit_code: Some(manifest.exit_code),
            timed_out: false,
            cases: cases.len() as u32,
            failed: cases.iter().filter(|c| c.outcome.failed()).count() as u32,
            skipped: cases
                .iter()
                .filter(|c| c.outcome == onus_core::CaseOutcome::Skipped)
                .count() as u32,
        };
        if !found {
            return empty(
                format!(
                    "the {what} run wrote no JUnit file matching {} (exit code {}; `onus evidence show {}`)",
                    self.junit.join(", "),
                    manifest.exit_code,
                    &id[..12]
                ),
                Some(run),
            );
        }
        Side {
            problem: None,
            run: Some(run),
            cases: Some(cases),
            ran_in_env: true,
        }
    }
}

/// A glob the shell can expand as it is: no spaces, quotes or `..`.
fn plain_glob(g: &str) -> bool {
    !g.is_empty()
        && !g.starts_with(['/', '-'])
        && !g.split('/').any(|p| p == "..")
        && g.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./*?@+-".contains(c))
}

/// A path as one shell word.
fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// `3 test files selected (2 at base, 3 at head); base 6/6 passed, head 6/6
/// passed, cross 1/2 failed · 48s`
fn note(run: &BehaviorRun) -> String {
    let at = |base: bool| {
        run.selected
            .iter()
            .filter(|s| if base { s.base } else { s.head })
            .count()
    };
    let mut parts: Vec<String> = Vec::new();
    for r in &run.runs {
        let side = r.side.as_str();
        parts.push(if r.timed_out {
            format!("{side} timed out")
        } else if r.side == RunSide::Cross {
            format!("{side} {}/{} failed", r.failed, r.cases)
        } else {
            format!(
                "{side} {}/{} passed",
                r.cases - r.failed - r.skipped,
                r.cases
            )
        });
    }
    let mut text = format!(
        "{} test file{} selected ({} at base, {} at head)",
        run.selected.len(),
        if run.selected.len() == 1 { "" } else { "s" },
        at(true),
        at(false)
    );
    if !parts.is_empty() {
        text.push_str("; ");
        text.push_str(&parts.join(", "));
    }
    if let Some(reason) = &run.reason {
        text.push_str(&format!("; partial: {reason}"));
    }
    format!("{text} · {}s", run.elapsed_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_quoted_and_globs_checked() {
        assert_eq!(quote("src/a.test.ts"), "src/a.test.ts");
        assert_eq!(quote("src/it's a.test.ts"), r"'src/it'\''s a.test.ts'");
        assert!(plain_glob("reports/onus-behavior*.xml"));
        assert!(!plain_glob("reports/$(rm -rf).xml"));
        assert!(!plain_glob("../x.xml"));
        assert!(!plain_glob("/etc/x.xml"));
    }
}
