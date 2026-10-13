//! Held-out suites (ADR 0018): checks the author never sees, kept outside
//! the repository, chosen per change from the map, and the judge
//! configuration that rotates between changes.
//!
//! A change's reach is what it touches in the head map: the components of
//! its rows and changed files, its rows' subjects, and the pairs of
//! components that interact (code dependencies, file imports, or one
//! publishes an event the other consumes) with at least one side touched. A check is chosen by
//! its best reason: it covers a pair (a combination, which the author's own
//! tests rarely exercise), a subject, a touched component, everything, or a
//! neighbour.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use onus_core::{
    CheckCommand, CodebaseMap, EdgeKind, HeldOutCheck, HeldOutSuite, JudgeConfig, LanesConfig,
    SemanticReport, ids,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The file a suite directory must hold.
pub const SUITE_FILE: &str = "held-out.yaml";

/// The id `lanes.heldOut` from onus.yaml runs under.
pub const LEGACY_ID: &str = "lanes.heldOut";

/// A suite read from a directory outside the repository.
#[derive(Debug, Clone)]
pub struct LoadedSuite {
    pub suite: HeldOutSuite,
    /// The directory, canonical.
    pub dir: PathBuf,
    /// sha256 over the sorted (relative path, bytes) of every file
    /// (excluding .git).
    pub key: String,
}

impl LoadedSuite {
    /// The 12-hex prefix shown in judgments.
    pub fn short(&self) -> &str {
        &self.key[..12]
    }
}

/// The worktree `repo` is in, canonical: its top level when git knows it.
fn worktree(repo: &Path) -> Result<PathBuf> {
    let repo = repo
        .canonicalize()
        .with_context(|| format!("no repository at {}", repo.display()))?;
    let top = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .and_then(|t| t.canonicalize().ok());
    Ok(top.unwrap_or(repo))
}

/// Refuses a suite inside `repo`, without held-out.yaml, with duplicate or
/// non-slug ids, or of a version other than 1.
pub fn load(dir: &Path, repo: &Path) -> Result<LoadedSuite> {
    let canonical = dir
        .canonicalize()
        .with_context(|| format!("no held-out suite at {}", dir.display()))?;
    if canonical.starts_with(worktree(repo)?) {
        bail!(
            "the held-out suite at {} is inside the repository, where the author can read it; keep it outside (onus help lanes)",
            dir.display()
        );
    }
    read(&canonical)
}

/// Reads and checks the suite in `dir`, wherever it is.
pub fn read(dir: &Path) -> Result<LoadedSuite> {
    let canonical = dir
        .canonicalize()
        .with_context(|| format!("no held-out suite at {}", dir.display()))?;
    let file = canonical.join(SUITE_FILE);
    let text = std::fs::read_to_string(&file)
        .with_context(|| format!("{} holds no {SUITE_FILE}", dir.display()))?;
    let suite = parse(&text).with_context(|| format!("{}", file.display()))?;
    let key = suite_key(&canonical)?;
    Ok(LoadedSuite {
        suite,
        dir: canonical,
        key,
    })
}

/// Parses and checks `held-out.yaml`.
pub fn parse(text: &str) -> Result<HeldOutSuite> {
    let suite: HeldOutSuite = serde_yaml_ng::from_str(text)?;
    if suite.version != 1 {
        bail!("version must be 1, found {}", suite.version);
    }
    let mut seen = BTreeSet::new();
    for c in &suite.checks {
        if c.id.is_empty() || ids::slug(&c.id) != c.id {
            bail!(
                "the check id `{}` is not a slug (lowercase letters, digits and dashes)",
                c.id
            );
        }
        if !seen.insert(c.id.as_str()) {
            bail!("the check id `{}` is used twice", c.id);
        }
        if c.command.trim().is_empty() {
            bail!("the check `{}` has no command", c.id);
        }
    }
    Ok(suite)
}

/// sha256 over every file under `dir` but `.git` directories: each one's
/// relative path, its length and its bytes, in path order. A symbolic link
/// counts as its target's name.
pub fn suite_key(dir: &Path) -> Result<String> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().to_string();
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let meta = std::fs::symlink_metadata(e.path())?;
            if meta.is_dir() {
                if name != ".git" {
                    walk(&e.path(), &path, out)?;
                }
            } else if meta.file_type().is_symlink() {
                let target = std::fs::read_link(e.path())?;
                out.insert(path, target.to_string_lossy().as_bytes().to_vec());
            } else {
                out.insert(path, std::fs::read(e.path())?);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    walk(dir, "", &mut files)?;
    let mut h = Sha256::new();
    for (path, bytes) in &files {
        h.update(path.as_bytes());
        h.update([0]);
        h.update((bytes.len() as u64).to_be_bytes());
        h.update(bytes);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// What a change reaches in the head map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    /// Components the change's rows and files are in.
    pub touched: BTreeSet<String>,
    /// The rows' subjects.
    #[serde(skip)]
    pub subjects: BTreeSet<String>,
    /// Components that interact, at least one of them touched: `(a, b)`,
    /// `a < b`.
    pub pairs: BTreeSet<(String, String)>,
    /// Whether the head map was read: `false` when it could not be built.
    /// Absent when no check covers anything, so the map was not needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<bool>,
}

impl Reach {
    /// The reach of `report`, whose changed files are `changed`, in `head`.
    pub fn of(report: &SemanticReport, changed: &[String], head: Option<&CodebaseMap>) -> Reach {
        let mut touched: BTreeSet<String> = report
            .changes
            .iter()
            .filter_map(|r| r.component.clone())
            .collect();
        let subjects = report.changes.iter().map(|r| r.subject.clone()).collect();
        let mut pairs = BTreeSet::new();
        if let Some(map) = head {
            let paths: BTreeSet<&str> = changed.iter().map(String::as_str).collect();
            touched.extend(
                map.files
                    .iter()
                    .filter(|f| paths.contains(f.path.as_str()))
                    .filter_map(|f| f.component_id.clone()),
            );
            let mut publishers: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            let mut consumers: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            let mut interact = BTreeSet::new();
            // A file's imports, resolved to files: they still link two
            // components when the change broke the symbols between them.
            let component_of_file: BTreeMap<&str, &str> = map
                .files
                .iter()
                .filter_map(|f| Some((f.path.as_str(), f.component_id.as_deref()?)))
                .collect();
            for f in &map.files {
                let Some(from) = f.component_id.as_deref() else {
                    continue;
                };
                for to in f
                    .imports
                    .iter()
                    .filter_map(|i| component_of_file.get(i.as_str()))
                {
                    interact.insert((from, *to));
                }
            }
            for e in &map.edges {
                let Some(from) = ids::component_of(&e.from) else {
                    continue;
                };
                match e.kind {
                    EdgeKind::Publishes => {
                        publishers.entry(e.to.as_str()).or_default().insert(from);
                    }
                    EdgeKind::Consumes => {
                        consumers.entry(e.to.as_str()).or_default().insert(from);
                    }
                    k if k.is_code_dependency() => {
                        if let Some(to) = ids::component_of(&e.to) {
                            interact.insert((from, to));
                        }
                    }
                    _ => {}
                }
            }
            for (event, from) in &publishers {
                for to in consumers.get(event).into_iter().flatten() {
                    for p in from {
                        interact.insert((p, to));
                    }
                }
            }
            for (a, b) in interact {
                if a == b || !(touched.contains(a) || touched.contains(b)) {
                    continue;
                }
                let (a, b) = if a < b { (a, b) } else { (b, a) };
                pairs.insert((a.to_string(), b.to_string()));
            }
        }
        Reach {
            touched,
            subjects,
            pairs,
            map: Some(head.is_some()),
        }
    }

    /// Components in a pair that the change does not touch.
    pub fn neighbours(&self) -> BTreeSet<&str> {
        self.pairs
            .iter()
            .flat_map(|(a, b)| [a.as_str(), b.as_str()])
            .filter(|c| !self.touched.contains(*c))
            .collect()
    }
}

/// Why a check was chosen, most relevant first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Why {
    /// Two components it covers interact, and the change touches one.
    Combination,
    /// It covers a row's subject, or the module of one.
    Subject,
    /// It covers a component the change touches.
    Touched,
    /// It covers nothing in particular, so it runs for every change.
    Always,
    /// It covers a component that interacts with one the change touches.
    Neighbour,
}

/// A check chosen for a change.
#[derive(Debug, Clone)]
pub struct Selected<'a> {
    pub check: Cow<'a, HeldOutCheck>,
    pub why: Why,
    /// The components or ids that made it relevant.
    pub with: Vec<String>,
}

/// The component a `covers` entry stands for: a bare component id is
/// itself, a map id its component; a global id has none.
fn covered_component(entry: &str) -> Option<&str> {
    if ids::is_global(entry) {
        None
    } else if entry.contains(':') {
        ids::component_of(entry)
    } else {
        Some(entry)
    }
}

/// Why `check` is relevant to `reach`, if it is.
fn relevance(check: &HeldOutCheck, reach: &Reach) -> Option<(Why, Vec<String>)> {
    if check.covers.is_empty() {
        return Some((Why::Always, vec![]));
    }
    let covered: BTreeSet<&str> = check
        .covers
        .iter()
        .filter_map(|c| covered_component(c))
        .collect();
    if let Some((a, b)) = reach
        .pairs
        .iter()
        .find(|(a, b)| covered.contains(a.as_str()) && covered.contains(b.as_str()))
    {
        return Some((Why::Combination, vec![a.clone(), b.clone()]));
    }
    // Map ids only: a bare component id speaks through the components.
    let subjects: Vec<String> = check
        .covers
        .iter()
        .filter(|c| c.contains(':'))
        .filter(|c| {
            reach
                .subjects
                .iter()
                .any(|s| s == *c || ids::module_of(s) == c.as_str())
        })
        .cloned()
        .collect();
    if !subjects.is_empty() {
        return Some((Why::Subject, subjects));
    }
    let touched: Vec<String> = covered
        .iter()
        .filter(|c| reach.touched.contains(**c))
        .map(|c| c.to_string())
        .collect();
    if !touched.is_empty() {
        return Some((Why::Touched, touched));
    }
    let neighbours = reach.neighbours();
    let near: Vec<String> = covered
        .iter()
        .filter(|c| neighbours.contains(**c))
        .map(|c| c.to_string())
        .collect();
    (!near.is_empty()).then_some((Why::Neighbour, near))
}

/// Checks to run, most relevant first, `lanes.heldOut` included as an
/// untagged check. Within a reason, checks keep their order in the file.
/// `limit` cuts the list.
pub fn select<'a>(
    suite: Option<&'a HeldOutSuite>,
    legacy: Option<&CheckCommand>,
    reach: &Reach,
    limit: Option<u32>,
) -> Vec<Selected<'a>> {
    let mut out: Vec<Selected<'a>> = suite
        .into_iter()
        .flat_map(|s| &s.checks)
        .filter_map(|c| {
            relevance(c, reach).map(|(why, with)| Selected {
                check: Cow::Borrowed(c),
                why,
                with,
            })
        })
        .collect();
    if let Some(l) = legacy {
        out.push(Selected {
            check: Cow::Owned(HeldOutCheck {
                id: LEGACY_ID.into(),
                command: l.command.clone(),
                image: l.image.clone(),
                setup: l.setup.clone(),
                covers: vec![],
            }),
            why: Why::Always,
            with: vec![],
        });
    }
    // Stable: the file's order within a reason.
    out.sort_by_key(|s| s.why);
    if let Some(n) = limit {
        out.truncate(n as usize);
    }
    out
}

/// The limit on checks: the configuration's, else the suite's.
pub fn limit_for(suite: Option<&HeldOutSuite>, chosen: Option<&JudgeConfig>) -> Option<u32> {
    chosen
        .and_then(|j| j.held_out_limit)
        .or_else(|| suite.and_then(|s| s.limit))
}

/// The suite's checks that were not chosen, in file order.
pub fn not_selected(suite: Option<&HeldOutSuite>, selected: &[Selected]) -> Vec<String> {
    let chosen: BTreeSet<&str> = selected.iter().map(|s| s.check.id.as_str()).collect();
    suite
        .into_iter()
        .flat_map(|s| &s.checks)
        .filter(|c| !chosen.contains(c.id.as_str()))
        .map(|c| c.id.clone())
        .collect()
}

/// The configuration the rotation picks, or the forced one: `(index,
/// configuration, forced)`. `None` when `lanes.judges` is empty. The pick
/// hashes the head commit with the suite's full key, so an author who
/// cannot read the suite cannot predict it. It can still learn it from a
/// judgment and amend until a lenient one comes up, so the agent door hides
/// it.
pub fn choose<'a>(
    config: &'a LanesConfig,
    head: &str,
    suite_key: &str,
    forced: Option<&str>,
) -> Result<Option<(usize, &'a JudgeConfig, bool)>> {
    let judges = &config.judges;
    if let Some(name) = forced {
        let Some(i) = judges.iter().position(|j| j.name == name) else {
            let known: Vec<&str> = judges.iter().map(|j| j.name.as_str()).collect();
            if known.is_empty() {
                bail!("no judge configuration `{name}`: lanes.judges is empty");
            }
            bail!(
                "no judge configuration `{name}` in lanes.judges ({})",
                known.join(", ")
            );
        };
        return Ok(Some((i, &judges[i], true)));
    }
    if judges.is_empty() {
        return Ok(None);
    }
    let mut h = Sha256::new();
    h.update(b"onus judge rotation\n");
    h.update(head.as_bytes());
    h.update(b"\n");
    h.update(suite_key.as_bytes());
    let digest = h.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    let i = (u64::from_be_bytes(first) % judges.len() as u64) as usize;
    Ok(Some((i, &judges[i], false)))
}

/// What the judge did with the held-out checks, for people only: the
/// author sees counts in the judgment, never this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldOutReport {
    /// The suite's short hash; empty with `lanes.heldOut` alone.
    pub suite: String,
    /// The judge configuration that ran them.
    pub judge: String,
    pub reach: Reach,
    /// The chosen checks, in the order they run.
    pub checks: Vec<CheckResult>,
    /// The suite's checks left out for this change.
    pub not_selected: Vec<String>,
}

/// One chosen check and how it went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub id: String,
    pub why: Why,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub with: Vec<String>,
    /// `passed`, `failed`, `not-run` or `error`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output_tail: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_core::{
        ChangeKind, ChangeLevel, Confidence, Hints, ReportSummary, SemanticChange, StructureNotes,
        TextStats,
    };
    use serde_json::json;

    fn map() -> CodebaseMap {
        let component = |id: &str| {
            json!({
                "id": id, "kind": "service", "roots": [format!("{id}/**")],
                "publicEntrypoints": [], "owners": [], "labels": [], "confidence": "declared",
            })
        };
        // e only imports a's file: a symbol it used is gone at the head.
        let file = |c: &str| {
            let imports: Vec<&str> = if c == "e" {
                vec!["a/src/index.ts"]
            } else {
                vec![]
            };
            json!({
                "path": format!("{c}/src/index.ts"), "componentId": c, "language": "typescript",
                "isTest": false, "lines": 1, "contentHash": "x", "imports": imports,
            })
        };
        let edge = |from: &str, to: &str, kind: &str| json!({ "from": from, "to": to, "kind": kind, "confidence": "static", "sites": [] });
        let names = ["a", "b", "c", "d", "e"];
        let components: Vec<_> = names.iter().map(|c| component(c)).collect();
        let files: Vec<_> = names.iter().map(|c| file(c)).collect();
        serde_json::from_value(json!({
            "schemaVersion": 1, "commit": "h",
            "builtWith": { "onus": "test", "providers": {}, "configHash": "inferred" },
            "components": components,
            "files": files,
            "symbols": [],
            "edges": [
                edge("a:src/index.ts", "b:src/index.ts#f", "imports"),
                edge("a:src/index.ts#send", "event:Sent", "publishes"),
                edge("c:src/index.ts#on", "event:Sent", "consumes"),
                edge("d:src/index.ts#x", "event:Other", "publishes"),
            ],
            "externals": [], "packages": [], "tests": [], "rules": [], "diagnostics": [],
        }))
        .unwrap()
    }

    fn row(component: &str, subject: &str) -> SemanticChange {
        SemanticChange {
            id: format!("x:{subject}"),
            kind: ChangeKind::Internal,
            subkind: "x".into(),
            level: ChangeLevel::Structure,
            subject: subject.into(),
            component: Some(component.into()),
            kind_label: String::new(),
            title: String::new(),
            why_it_matters: String::new(),
            hints: Hints {
                labels: vec![],
                blast_radius: 0,
                novelty: vec![],
                confidence: Confidence::Static,
                rules_of_the_game: false,
                intent_mismatch: false,
                needs_person: false,
                dependent_components: 0,
                dependents: vec![],
                tested: None,
                owners: vec![],
                coverage: None,
            },
            locations: vec![],
            stats: None,
        }
    }

    fn report(rows: Vec<SemanticChange>) -> SemanticReport {
        SemanticReport {
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
            changes: rows,
            intent_check: None,
            rule_violations: vec![],
            structure: StructureNotes::default(),
            text_stats: TextStats::default(),
            map_diagnostics: vec![],
            coverage: None,
        }
    }

    fn check(id: &str, covers: &[&str]) -> HeldOutCheck {
        HeldOutCheck {
            id: id.into(),
            command: format!("run {id}"),
            image: None,
            setup: None,
            covers: covers.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn suite(checks: Vec<HeldOutCheck>) -> HeldOutSuite {
        HeldOutSuite {
            version: 1,
            image: None,
            setup: None,
            limit: None,
            checks,
        }
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.into(), b.into())
    }

    #[test]
    fn reach_counts_rows_files_imports_and_events() {
        let m = map();
        // A row in a, no changed files: a imports b, c consumes a's event,
        // and e imports a's file.
        let r = Reach::of(&report(vec![row("a", "a:src/index.ts#f")]), &[], Some(&m));
        assert_eq!(r.touched, BTreeSet::from(["a".to_string()]));
        let reached = BTreeSet::from([pair("a", "b"), pair("a", "c"), pair("a", "e")]);
        assert_eq!(r.pairs, reached);
        assert_eq!(r.neighbours(), BTreeSet::from(["b", "c", "e"]));
        assert_eq!(r.map, Some(true));
        // A changed file of a, with no rows, reaches the same.
        let r = Reach::of(&report(vec![]), &["a/src/index.ts".into()], Some(&m));
        assert_eq!(r.pairs, reached);
        // d interacts with nobody: touching it reaches no pair.
        let r = Reach::of(&report(vec![row("d", "d:x")]), &[], Some(&m));
        assert!(r.pairs.is_empty());
        // Without the map, only the rows.
        let r = Reach::of(
            &report(vec![row("a", "a:x")]),
            &["b/src/index.ts".into()],
            None,
        );
        assert_eq!(r.touched, BTreeSet::from(["a".to_string()]));
        assert!(r.pairs.is_empty() && r.map == Some(false));
    }

    #[test]
    fn selection_ranks_combinations_subjects_touched_always_neighbours() {
        let m = map();
        let reach = Reach::of(&report(vec![row("a", "a:src/index.ts#f")]), &[], Some(&m));
        let s = suite(vec![
            check("near", &["c"]),
            check("smoke", &[]),
            check("unrelated", &["d"]),
            check("mine", &["a"]),
            check("symbol", &["a:src/index.ts#f"]),
            check("pair", &["a", "b"]),
        ]);
        let picked = select(Some(&s), None, &reach, None);
        let order: Vec<(&str, Why, Vec<String>)> = picked
            .iter()
            .map(|p| (p.check.id.as_str(), p.why, p.with.clone()))
            .collect();
        assert_eq!(
            order,
            [
                ("pair", Why::Combination, vec!["a".into(), "b".into()]),
                ("symbol", Why::Subject, vec!["a:src/index.ts#f".into()]),
                ("mine", Why::Touched, vec!["a".into()]),
                ("smoke", Why::Always, vec![]),
                ("near", Why::Neighbour, vec!["c".into()]),
            ]
        );
        assert_eq!(not_selected(Some(&s), &picked), ["unrelated"]);
        // lanes.heldOut runs as an untagged check.
        let legacy = CheckCommand {
            command: "npm run hidden".into(),
            image: None,
            setup: None,
        };
        let picked = select(Some(&s), Some(&legacy), &reach, None);
        assert_eq!(picked[4].check.id, LEGACY_ID);
        assert_eq!(picked[4].why, Why::Always);
    }

    #[test]
    fn limit_cuts_the_least_relevant() {
        let m = map();
        let reach = Reach::of(&report(vec![row("a", "a:x")]), &[], Some(&m));
        let s = suite(vec![
            check("near", &["c"]),
            check("smoke", &[]),
            check("pair", &["a", "c"]),
        ]);
        let picked = select(Some(&s), None, &reach, Some(2));
        let ids: Vec<&str> = picked.iter().map(|p| p.check.id.as_str()).collect();
        assert_eq!(ids, ["pair", "smoke"]);
        assert_eq!(not_selected(Some(&s), &picked), ["near"]);
        // The configuration's limit comes before the suite's.
        let mut limited = s.clone();
        limited.limit = Some(1);
        assert_eq!(limit_for(Some(&limited), None), Some(1));
        let strict = JudgeConfig {
            name: "strict".into(),
            taste: None,
            held_out_limit: Some(3),
        };
        assert_eq!(limit_for(Some(&limited), Some(&strict)), Some(3));
    }

    #[test]
    fn a_symbol_cover_ranks_as_subject_only_when_a_row_is_about_it() {
        let s = suite(vec![check("send", &["d:src/sms.ts#sendSms"])]);
        let about = Reach::of(&report(vec![row("d", "d:src/sms.ts#sendSms")]), &[], None);
        let picked = select(Some(&s), None, &about, None);
        assert_eq!(picked[0].why, Why::Subject);
        // The module of the subject counts too.
        let module = suite(vec![check("module", &["d:src/sms.ts"])]);
        assert_eq!(
            select(Some(&module), None, &about, None)[0].why,
            Why::Subject
        );
        // Another row in the same component: the symbol stands for d.
        let other = Reach::of(&report(vec![row("d", "d:src/other.ts#x")]), &[], None);
        let picked = select(Some(&s), None, &other, None);
        assert_eq!(picked[0].why, Why::Touched);
        // A global id counts only through subjects.
        let event = suite(vec![check("event", &["event:Sent"])]);
        assert!(select(Some(&event), None, &other, None).is_empty());
        let sent = Reach::of(&report(vec![row("a", "event:Sent")]), &[], None);
        assert_eq!(select(Some(&event), None, &sent, None)[0].why, Why::Subject);
    }

    fn write(dir: &Path, files: &[(&str, &str)]) {
        for (path, text) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
    }

    const YAML: &str = "version: 1\nchecks:\n  - { id: one, command: \"true\", covers: [a] }\n";

    #[test]
    fn load_refuses_a_suite_inside_the_repository() {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), &[("held-out/held-out.yaml", YAML)]);
        let err = load(&repo.path().join("held-out"), repo.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("inside the repository"),
            "{err:#}"
        );
        // Through a symbolic link from outside, too.
        let outside = tempfile::tempdir().unwrap();
        let link = outside.path().join("suite");
        std::os::unix::fs::symlink(repo.path().join("held-out"), &link).unwrap();
        let err = load(&link, repo.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("inside the repository"),
            "{err:#}"
        );
        // And from a folder of the repository given as --repo.
        std::fs::create_dir_all(repo.path().join("sub")).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        let err = load(&repo.path().join("held-out"), &repo.path().join("sub")).unwrap_err();
        assert!(
            format!("{err:#}").contains("inside the repository"),
            "{err:#}"
        );
        // Outside, it loads.
        write(outside.path(), &[("real/held-out.yaml", YAML)]);
        let s = load(&outside.path().join("real"), repo.path()).unwrap();
        assert_eq!(s.suite.checks.len(), 1);
        assert_eq!(s.short().len(), 12);
    }

    #[test]
    fn load_needs_held_out_yaml() {
        let repo = tempfile::tempdir().unwrap();
        let suite = tempfile::tempdir().unwrap();
        write(suite.path(), &[("check.test.mjs", "")]);
        let err = load(suite.path(), repo.path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("holds no held-out.yaml"),
            "{err:#}"
        );
        let err = load(&suite.path().join("missing"), repo.path()).unwrap_err();
        assert!(format!("{err:#}").contains("no held-out suite"), "{err:#}");
    }

    #[test]
    fn duplicate_or_bad_ids_are_refused() {
        let twice = "version: 1\nchecks:\n  - { id: a, command: x }\n  - { id: a, command: y }\n";
        assert!(format!("{:#}", parse(twice).unwrap_err()).contains("used twice"));
        let bad = "version: 1\nchecks:\n  - { id: Not A Slug, command: x }\n";
        assert!(format!("{:#}", parse(bad).unwrap_err()).contains("not a slug"));
        assert!(parse("version: 2\nchecks: []\n").is_err());
        assert!(parse("version: 1\nchecks: []\nsecret: 1\n").is_err());
        assert!(parse("version: 1\nchecks:\n  - { id: a, command: x, cover: [b] }\n").is_err());
        assert!(parse(YAML).is_ok());
    }

    #[test]
    fn the_key_changes_with_any_file_and_ignores_git() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            &[("held-out.yaml", YAML), ("t/a.test.mjs", "1")],
        );
        let first = suite_key(dir.path()).unwrap();
        assert_eq!(first.len(), 64);
        write(
            dir.path(),
            &[(".git/HEAD", "ref: x"), (".git/objects/a", "y")],
        );
        assert_eq!(suite_key(dir.path()).unwrap(), first);
        write(dir.path(), &[("t/a.test.mjs", "2")]);
        let second = suite_key(dir.path()).unwrap();
        assert_ne!(second, first);
        write(dir.path(), &[("t/b.test.mjs", "")]);
        assert_ne!(suite_key(dir.path()).unwrap(), second);
    }

    fn judges(names: &[&str]) -> LanesConfig {
        LanesConfig {
            judges: names
                .iter()
                .map(|n| JudgeConfig {
                    name: n.to_string(),
                    taste: None,
                    held_out_limit: None,
                })
                .collect(),
            ..LanesConfig::default()
        }
    }

    #[test]
    fn rotation_is_stable_for_a_head_and_spread_over_heads() {
        let config = judges(&["a", "b"]);
        let key = "k".repeat(64);
        let mut counts = [0u32; 2];
        for i in 0..400 {
            let head = format!("{i:040x}");
            let (n, j, forced) = choose(&config, &head, &key, None).unwrap().unwrap();
            assert!(!forced);
            assert_eq!(config.judges[n], *j);
            counts[n] += 1;
            // The same head and key always give the same configuration.
            assert_eq!(choose(&config, &head, &key, None).unwrap().unwrap().0, n);
        }
        assert!(counts.iter().all(|c| *c >= 150), "{counts:?}");
        // Another suite reshuffles the choices.
        let moved = (0..400)
            .filter(|i| {
                let head = format!("{i:040x}");
                choose(&config, &head, &key, None).unwrap().unwrap().0
                    != choose(&config, &head, "other", None).unwrap().unwrap().0
            })
            .count();
        assert!(moved > 100, "{moved}");
        // No configurations, no rotation.
        assert!(
            choose(&LanesConfig::default(), "h", "", None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn forcing_an_unknown_configuration_is_an_error() {
        let config = judges(&["a", "b"]);
        let (i, j, forced) = choose(&config, "h", "", Some("b")).unwrap().unwrap();
        assert_eq!((i, j.name.as_str(), forced), (1, "b", true));
        let err = choose(&config, "h", "", Some("c")).unwrap_err();
        assert!(
            format!("{err}").contains("no judge configuration `c`"),
            "{err}"
        );
        assert!(choose(&LanesConfig::default(), "h", "", Some("a")).is_err());
    }
}
