//! The behavior diff's rows (ADR 0019): the test cases that reach a change,
//! as they ran at the base, at the head, once more at the head, and in their
//! base versions against the head's code, compared per case.
//!
//! Pure: the runs are made elsewhere (`onus report --behavior`), and only
//! their JUnit cases come in. Every row comes from fixed templates.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::{
    BehaviorRun, CaseOutcome, ChangeKind, ChangeLevel, CodebaseMap, Confidence, JunitCase,
    Location, OnusConfig, SemanticChange, SemanticReport, TestCase, TestNode, rank,
};

use crate::ctx::{change, join_some, sensitive_labels};

/// A case: its test file (else its JUnit `classname`), and its name.
pub type CaseKey = (String, String);

/// How a case ended in one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseResult {
    pub outcome: CaseOutcome,
    pub message: Option<String>,
    /// The test file it belongs to, when that is known.
    pub file: Option<String>,
}

pub type Cases = BTreeMap<CaseKey, CaseResult>;

/// The JUnit cases of each run; `None` for a run that did not happen or
/// wrote no JUnit file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// The base's tests on the base's code.
    pub base: Option<Vec<JunitCase>>,
    /// The head's tests on the head's code.
    pub head: Option<Vec<JunitCase>>,
    /// Head files with newly failing cases, run once more at the head.
    pub retry: Option<Vec<JunitCase>>,
    /// The base versions of edited test files on the head's code.
    pub cross: Option<Vec<JunitCase>>,
}

/// The rows, and the cases that gave none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compared {
    pub rows: Vec<SemanticChange>,
    /// Failed at the head and passed when run again: `file › case`.
    pub flaky: Vec<String>,
    /// Failed at the base and at the head with the same message.
    pub already_failing: Vec<String>,
}

const NEWLY_FAILING: &str = "behavior-newly-failing";
const CHANGED: &str = "behavior-changed";
const NEWLY_PASSING: &str = "behavior-newly-passing";
const OUTPUT_CHANGED: &str = "behavior-output-changed";

/// The component of cases no test file could be found for.
const UNATTRIBUTED: &str = "root";

/// The selected test files of both maps, to attribute cases to them.
struct Tests<'a> {
    selected: BTreeSet<&'a str>,
    base: BTreeMap<&'a str, &'a TestNode>,
    head: BTreeMap<&'a str, &'a TestNode>,
    head_map: &'a CodebaseMap,
    base_map: &'a CodebaseMap,
}

/// The last segment of a test name: `a > b › c` → `c`.
fn last_segment(name: &str) -> &str {
    [" > ", " › ", "::"]
        .iter()
        .filter_map(|sep| name.rfind(sep).map(|i| i + sep.len()))
        .max()
        .map_or(name, |i| &name[i..])
        .trim()
}

impl<'a> Tests<'a> {
    fn new(base: &'a CodebaseMap, head: &'a CodebaseMap, selected: &'a [String]) -> Tests<'a> {
        let selected: BTreeSet<&str> = selected.iter().map(String::as_str).collect();
        let pick = |map: &'a CodebaseMap| -> BTreeMap<&'a str, &'a TestNode> {
            map.tests
                .iter()
                .filter(|t| selected.contains(t.file.as_str()))
                .map(|t| (t.file.as_str(), t))
                .collect()
        };
        Tests {
            base: pick(base),
            head: pick(head),
            selected,
            head_map: head,
            base_map: base,
        }
    }

    /// The test file of a case: its `file` attribute, its `classname` when
    /// that is a selected file, or the one selected file with a case of
    /// that name.
    fn attribute(&self, c: &JunitCase) -> Option<String> {
        if let Some(f) = &c.file {
            return Some(f.clone());
        }
        if self.selected.contains(c.classname.as_str()) {
            return Some(c.classname.clone());
        }
        let wanted = last_segment(&c.name);
        let files: BTreeSet<&str> = self
            .base
            .iter()
            .chain(self.head.iter())
            .filter(|(_, t)| t.cases.iter().any(|tc| last_segment(&tc.name) == wanted))
            .map(|(f, _)| *f)
            .collect();
        match files.len() {
            1 => files.into_iter().next().map(str::to_string),
            _ => None,
        }
    }

    /// The cases of one run, by key. A key reported twice keeps its worst
    /// outcome.
    fn cases(&self, run: &[JunitCase]) -> Cases {
        let mut out = Cases::new();
        for c in run {
            let file = self.attribute(c);
            let key = (
                file.clone().unwrap_or_else(|| c.classname.clone()),
                c.name.clone(),
            );
            let result = CaseResult {
                outcome: c.outcome,
                message: c.message.clone(),
                file,
            };
            match out.get(&key) {
                Some(old) if severity(old.outcome) >= severity(result.outcome) => {}
                _ => {
                    out.insert(key, result);
                }
            }
        }
        out
    }

    /// The map's case of that name in that test file.
    fn case(&self, file: &str, name: &str, side: onus_core::Side) -> Option<&'a TestCase> {
        let node = match side {
            onus_core::Side::Base => self.base.get(file),
            onus_core::Side::Head => self.head.get(file),
        }?;
        let wanted = last_segment(name);
        let mut matches = node
            .cases
            .iter()
            .filter(|tc| tc.name == name || last_segment(&tc.name) == wanted);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    fn location(&self, file: Option<&str>, name: &str, side: onus_core::Side) -> Option<Location> {
        let file = file?;
        let tc = self.case(file, name, side)?;
        Some(match side {
            onus_core::Side::Base => Location::base(file, tc.line, tc.end_line),
            onus_core::Side::Head => Location::head(file, tc.line, tc.end_line),
        })
    }

    /// The component of a test file, in the head map, else the base map.
    fn component(&self, file: Option<&str>) -> String {
        let Some(file) = file else {
            return UNATTRIBUTED.into();
        };
        let in_tests = |map: &CodebaseMap| {
            map.tests
                .iter()
                .find(|t| t.file == file)
                .and_then(|t| t.component_id.clone())
        };
        let in_files = |map: &CodebaseMap| {
            map.files
                .iter()
                .find(|f| f.path == file)
                .and_then(|f| f.component_id.clone())
        };
        in_tests(self.head_map)
            .or_else(|| in_tests(self.base_map))
            .or_else(|| in_files(self.head_map))
            .or_else(|| in_files(self.base_map))
            .unwrap_or_else(|| UNATTRIBUTED.into())
    }
}

fn severity(o: CaseOutcome) -> u8 {
    match o {
        CaseOutcome::Skipped => 0,
        CaseOutcome::Passed => 1,
        CaseOutcome::Failed | CaseOutcome::Error => 2,
    }
}

fn passed(c: Option<&CaseResult>) -> bool {
    c.is_some_and(|c| c.outcome == CaseOutcome::Passed)
}

fn failed(c: Option<&CaseResult>) -> bool {
    c.is_some_and(|c| c.outcome.failed())
}

/// Ran and did not pass or fail: absent, for every comparison.
fn absent(c: Option<&CaseResult>) -> bool {
    !passed(c) && !failed(c)
}

/// A failure message with what differs between runs of the same failure
/// taken out: colours, durations, addresses and the environment's `/work/`.
pub fn normalize_message(message: &str) -> String {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("valid regex");
    let duration = regex::Regex::new(r"\b\d+(\.\d+)?\s?(ms|s)\b").expect("valid regex");
    let address = regex::Regex::new(r"\b0x[0-9a-fA-F]+\b").expect("valid regex");
    let text = ansi.replace_all(message, "");
    let text = duration.replace_all(&text, "");
    let text = address.replace_all(&text, "");
    let text = text.replace("/work/", "");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.chars().take(300).collect()
}

/// Head files with cases that fail at the head and passed, or did not
/// exist, at the base: they are run once more before a row blocks.
pub fn retry_files(
    observed: &Observed,
    base: &CodebaseMap,
    head: &CodebaseMap,
    selected: &[String],
) -> Vec<String> {
    let (Some(b), Some(h)) = (&observed.base, &observed.head) else {
        return Vec::new();
    };
    let tests = Tests::new(base, head, selected);
    let (b, h) = (tests.cases(b), tests.cases(h));
    let files: BTreeSet<String> = h
        .iter()
        .filter(|(k, r)| r.outcome.failed() && !failed(b.get(*k)))
        .filter_map(|(_, r)| r.file.clone())
        .collect();
    files.into_iter().collect()
}

/// What happened to one case.
#[derive(Debug, Clone)]
struct Item {
    file: Option<String>,
    name: String,
    message: Option<String>,
    /// The base's message, for a case that fails differently.
    was: Option<String>,
    location: Option<Location>,
    /// A newly failing case that was new at the head.
    new: bool,
    /// A newly failing case that was not run again.
    unconfirmed: bool,
}

impl Item {
    fn label(&self) -> String {
        match &self.file {
            Some(f) => format!("\"{}\" (`{f}`)", self.name),
            None => format!("\"{}\"", self.name),
        }
    }
}

/// `file › case`
fn display(key: &CaseKey) -> String {
    format!("{} › {}", key.0, key.1)
}

/// Compares the runs case by case and groups what changed into rows, one
/// per kind and component.
pub fn rows(
    observed: &Observed,
    base: &CodebaseMap,
    head: &CodebaseMap,
    selected: &[String],
) -> Compared {
    let mut out = Compared::default();
    let (Some(b), Some(h)) = (&observed.base, &observed.head) else {
        return out;
    };
    let tests = Tests::new(base, head, selected);
    let b = tests.cases(b);
    let h = tests.cases(h);
    let r = observed.retry.as_ref().map(|r| tests.cases(r));
    let x = observed
        .cross
        .as_ref()
        .map(|x| tests.cases(x))
        .unwrap_or_default();
    let keys: BTreeSet<&CaseKey> = b.keys().chain(h.keys()).chain(x.keys()).collect();
    let mut groups: BTreeMap<(&str, String), Vec<Item>> = BTreeMap::new();
    let message =
        |c: Option<&CaseResult>| c.and_then(|c| c.message.as_deref()).map(normalize_message);
    for key in keys {
        let (bc, hc, xc) = (b.get(key), h.get(key), x.get(key));
        let file = hc.or(bc).or(xc).and_then(|c| c.file.clone());
        let item = |side: onus_core::Side, msg: Option<String>| Item {
            file: file.clone(),
            name: key.1.clone(),
            message: msg,
            was: None,
            location: tests.location(file.as_deref(), &key.1, side),
            new: false,
            unconfirmed: false,
        };
        let component = tests.component(file.as_deref());
        if failed(hc) {
            if failed(bc) {
                let (was, now) = (message(bc), message(hc));
                if was == now {
                    out.already_failing.push(display(key));
                } else {
                    let mut i = item(onus_core::Side::Head, now);
                    i.was = was;
                    groups
                        .entry((OUTPUT_CHANGED, component))
                        .or_default()
                        .push(i);
                }
                continue;
            }
            let rc = r.as_ref().and_then(|r| r.get(key));
            if passed(rc) {
                out.flaky.push(display(key));
                continue;
            }
            let mut i = item(
                onus_core::Side::Head,
                message(rc.filter(|c| c.outcome.failed()).or(hc)),
            );
            i.new = absent(bc);
            i.unconfirmed = !failed(rc);
            groups
                .entry((NEWLY_FAILING, component))
                .or_default()
                .push(i);
            continue;
        }
        // The head's version passes or was renamed (a failing one is above).
        if passed(bc) && failed(xc) {
            let i = item(onus_core::Side::Base, message(xc));
            groups.entry((CHANGED, component)).or_default().push(i);
            continue;
        }
        if failed(bc) && passed(hc) {
            groups
                .entry((NEWLY_PASSING, component))
                .or_default()
                .push(item(onus_core::Side::Head, None));
        }
    }
    for ((subkind, component), items) in groups {
        out.rows.push(row(subkind, &component, &items));
    }
    out
}

fn row(subkind: &str, component: &str, items: &[Item]) -> SemanticChange {
    let labels: Vec<String> = items.iter().map(Item::label).collect();
    let cases = join_some(&labels, 4);
    let one = items.len() == 1;
    let first_message = items.iter().find_map(|i| i.message.clone());
    let quoted = |m: &Option<String>| m.as_ref().map(|m| format!(": {m}")).unwrap_or_default();
    let locations: Vec<Location> = items.iter().filter_map(|i| i.location.clone()).collect();
    let (kind, label, title, why) = match subkind {
        NEWLY_FAILING => {
            let verb = if items.iter().all(|i| i.new) {
                if one {
                    "is new and fails at the head"
                } else {
                    "are new and fail at the head"
                }
            } else if items.iter().all(|i| !i.new) {
                if one {
                    "passed at the base and fails at the head"
                } else {
                    "passed at the base and fail at the head"
                }
            } else {
                "passed at the base or are new, and fail at the head"
            };
            let unconfirmed = if items.iter().any(|i| i.unconfirmed) {
                "; not re-run to rule out a flaky test"
            } else {
                ""
            };
            (
                ChangeKind::Internal,
                "Behavior change, tests fail",
                format!("Tests fail after the change in `{component}`"),
                format!(
                    "{cases} {verb}{}{unconfirmed}; the change cannot merge until {} or a person approves this row",
                    quoted(&first_message),
                    if one { "it passes" } else { "they pass" }
                ),
            )
        }
        CHANGED => (
            ChangeKind::Test,
            "Behavior change, tests updated",
            format!("Behavior pinned by tests changed in `{component}`"),
            if one {
                format!(
                    "The base version of {cases} fails against the new code{}. The change edited the test to match; confirm the new behavior is intended",
                    quoted(&first_message)
                )
            } else {
                format!(
                    "The base versions of {cases} fail against the new code{}. The change edited the tests to match; confirm the new behavior is intended",
                    quoted(&first_message)
                )
            },
        ),
        NEWLY_PASSING => (
            ChangeKind::Internal,
            "Behavior change, fix",
            format!("Tests pass after the change in `{component}`"),
            format!(
                "{cases} failed at the base and {} at the head",
                if one { "passes" } else { "pass" }
            ),
        ),
        _ => {
            let first = items
                .iter()
                .find(|i| i.was.is_some() || i.message.is_some());
            let said =
                |m: Option<&String>| m.map_or("no message".to_string(), |m| format!("\"{m}\""));
            (
                ChangeKind::Internal,
                "Behavior change, output",
                format!("Failing tests fail differently in `{component}`"),
                format!(
                    "{cases} {} at the base and at the head, with a different message: was {}, now {}",
                    if one { "fails" } else { "fail" },
                    said(first.and_then(|i| i.was.as_ref())),
                    said(first.and_then(|i| i.message.as_ref()))
                ),
            )
        }
    };
    let mut row = change(
        kind,
        subkind,
        ChangeLevel::Behavior,
        component,
        Some(component),
        label,
        title,
        why,
        locations,
    );
    row.hints.confidence = if subkind == NEWLY_FAILING && items.iter().any(|i| i.unconfirmed) {
        Confidence::Inferred
    } else {
        Confidence::Traced
    };
    match subkind {
        NEWLY_FAILING => row.hints.needs_person = true,
        CHANGED => {
            row.hints.rules_of_the_game = true;
            row.hints.needs_person = true;
        }
        _ => {}
    }
    row
}

/// Adds the behavior rows and what ran to a report: labels and owners from
/// the head map's components, unique ids, ranked again, the summary counted
/// again. Applying it twice gives the same report.
pub fn apply(
    report: &mut SemanticReport,
    rows: Vec<SemanticChange>,
    block: BehaviorRun,
    head: &CodebaseMap,
    config: Option<&OnusConfig>,
) {
    report
        .changes
        .retain(|r| !r.subkind.starts_with("behavior-"));
    let sensitive = sensitive_labels(config);
    let mut ids: BTreeSet<String> = report.changes.iter().map(|r| r.id.clone()).collect();
    for mut r in rows {
        if let Some(c) = r
            .component
            .as_deref()
            .and_then(|c| head.components.iter().find(|k| k.id == c))
        {
            r.hints.labels = c
                .labels
                .iter()
                .filter(|l| sensitive.contains(*l))
                .cloned()
                .collect();
            r.hints.labels.sort();
            r.hints.owners = c.owners.clone();
            r.hints.owners.sort();
            r.hints.owners.dedup();
        }
        if r.hints.rules_of_the_game {
            r.hints.needs_person = true;
        }
        let mut id = r.id.clone();
        let mut n = 1;
        while ids.contains(&id) {
            n += 1;
            id = format!("{}#{n}", r.id);
        }
        ids.insert(id.clone());
        r.id = id;
        report.changes.push(r);
    }
    rank::rank(&mut report.changes);
    report.summary = crate::summarize(&report.changes);
    report.behavior = Some(block);
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_core::{BehaviorStatus, Side};

    fn case(file: &str, name: &str, outcome: CaseOutcome, message: Option<&str>) -> JunitCase {
        JunitCase {
            name: name.into(),
            classname: file.into(),
            file: None,
            outcome,
            message: message.map(str::to_string),
        }
    }

    use CaseOutcome::{Failed, Passed, Skipped};

    fn pass(name: &str) -> JunitCase {
        case("src/a.test.ts", name, Passed, None)
    }

    fn fail(name: &str, message: &str) -> JunitCase {
        case("src/a.test.ts", name, Failed, Some(message))
    }

    /// A map with one component, `shop`, and `src/a.test.ts` with cases.
    fn map(cases: &[&str]) -> CodebaseMap {
        serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "commit": "c",
            "builtWith": { "onus": "0", "providers": {}, "configHash": "" },
            "components": [{ "id": "shop", "kind": "package", "roots": ["src/**"],
                             "publicEntrypoints": [], "owners": ["@shop-team"],
                             "labels": ["payments", "fun"], "confidence": "declared" }],
            "files": [], "symbols": [], "edges": [], "externals": [], "packages": [],
            "tests": [{
                "id": "shop:a.test.ts", "componentId": "shop", "file": "src/a.test.ts",
                "cases": cases.iter().enumerate().map(|(i, n)| serde_json::json!({
                    "name": n, "line": 10 * (i + 1), "endLine": 10 * (i + 1) + 3,
                    "assertions": [], "fingerprint": "f"
                })).collect::<Vec<_>>(),
                "exercises": []
            }],
            "rules": [], "diagnostics": []
        }))
        .unwrap()
    }

    fn compare(observed: &Observed) -> Compared {
        let m = map(&["a", "b", "c"]);
        rows(observed, &m, &m, &["src/a.test.ts".to_string()])
    }

    fn observed(
        base: Vec<JunitCase>,
        head: Vec<JunitCase>,
        retry: Option<Vec<JunitCase>>,
        cross: Option<Vec<JunitCase>>,
    ) -> Observed {
        Observed {
            base: Some(base),
            head: Some(head),
            retry,
            cross,
        }
    }

    fn subkinds(c: &Compared) -> Vec<&str> {
        c.rows.iter().map(|r| r.subkind.as_str()).collect()
    }

    #[test]
    fn rules() {
        // Newly failing, confirmed by the retry.
        let c = compare(&observed(
            vec![pass("a")],
            vec![fail("a", "expected 0, received 10")],
            Some(vec![fail("a", "expected 0, received 10")]),
            None,
        ));
        assert_eq!(subkinds(&c), [NEWLY_FAILING]);
        assert_eq!(c.rows[0].hints.confidence, Confidence::Traced);
        assert!(c.rows[0].hints.needs_person);
        // Not re-run: inferred, and says so.
        let c = compare(&observed(
            vec![pass("a")],
            vec![fail("a", "boom")],
            None,
            None,
        ));
        assert_eq!(subkinds(&c), [NEWLY_FAILING]);
        assert_eq!(c.rows[0].hints.confidence, Confidence::Inferred);
        assert!(
            c.rows[0]
                .why_it_matters
                .contains("not re-run to rule out a flaky test")
        );
        // A new head case that fails.
        let c = compare(&observed(
            vec![pass("a")],
            vec![pass("a"), fail("b", "boom")],
            Some(vec![fail("b", "boom")]),
            None,
        ));
        assert_eq!(subkinds(&c), [NEWLY_FAILING]);
        assert!(
            c.rows[0]
                .why_it_matters
                .contains("is new and fails at the head")
        );
        // Flaky: fails at the head, passes again.
        let c = compare(&observed(
            vec![pass("a")],
            vec![fail("a", "boom")],
            Some(vec![pass("a")]),
            None,
        ));
        assert!(c.rows.is_empty());
        assert_eq!(c.flaky, ["src/a.test.ts › a"]);
        // Changed: the base version fails on the head's code, the head's passes.
        let c = compare(&observed(
            vec![pass("a")],
            vec![pass("a")],
            None,
            Some(vec![fail("a", "expected 0, received 10")]),
        ));
        assert_eq!(subkinds(&c), [CHANGED]);
        assert!(c.rows[0].hints.rules_of_the_game);
        // Also when the head renamed the case.
        let c = compare(&observed(
            vec![pass("a")],
            vec![pass("a2")],
            None,
            Some(vec![fail("a", "expected 0, received 10")]),
        ));
        assert_eq!(subkinds(&c), [CHANGED]);
        // Not when the head's version fails too: that is newly failing.
        let c = compare(&observed(
            vec![pass("a")],
            vec![fail("a", "boom")],
            Some(vec![fail("a", "boom")]),
            Some(vec![fail("a", "boom")]),
        ));
        assert_eq!(subkinds(&c), [NEWLY_FAILING]);
        // Newly passing.
        let c = compare(&observed(
            vec![fail("a", "bug")],
            vec![pass("a")],
            None,
            None,
        ));
        assert_eq!(subkinds(&c), [NEWLY_PASSING]);
        assert_eq!(c.rows[0].hints.confidence, Confidence::Traced);
        assert!(!c.rows[0].hints.needs_person);
        // Output changed, against the same normalised message.
        let c = compare(&observed(
            vec![fail("a", "expected 4.99 in 12ms")],
            vec![fail("a", "expected 5 in 3ms")],
            None,
            None,
        ));
        assert_eq!(subkinds(&c), [OUTPUT_CHANGED]);
        assert!(
            c.rows[0]
                .why_it_matters
                .contains("was \"expected 4.99 in\", now \"expected 5 in\"")
        );
        let c = compare(&observed(
            vec![fail("a", "expected 5 in 12ms")],
            vec![fail("a", "expected 5 in 3ms")],
            None,
            None,
        ));
        assert!(c.rows.is_empty());
        assert_eq!(c.already_failing, ["src/a.test.ts › a"]);
        // A case only at the base gives no row (tests_diff owns removed
        // cases), and a skipped case counts as absent.
        let c = compare(&observed(
            vec![pass("a"), pass("b")],
            vec![pass("a"), case("src/a.test.ts", "b", Skipped, None)],
            None,
            None,
        ));
        assert!(c.rows.is_empty() && c.flaky.is_empty() && c.already_failing.is_empty());
        // Without a base or a head run nothing is compared.
        let mut o = observed(vec![], vec![fail("a", "x")], None, None);
        o.base = None;
        assert_eq!(compare(&o), Compared::default());
        // The retry targets the files with newly failing cases.
        let o = observed(
            vec![pass("a"), fail("c", "old")],
            vec![fail("a", "x"), pass("b"), fail("c", "new")],
            None,
            None,
        );
        let m = map(&["a", "b", "c"]);
        assert_eq!(
            retry_files(&o, &m, &m, &["src/a.test.ts".to_string()]),
            ["src/a.test.ts"]
        );
    }

    #[test]
    fn attribution_and_text() {
        let m = map(&["discount › applies at the threshold", "gives 10 above"]);
        let selected = ["src/a.test.ts".to_string()];
        let tests = Tests::new(&m, &m, &selected);
        let named = |name: &str, classname: &str, file: Option<&str>| JunitCase {
            name: name.into(),
            classname: classname.into(),
            file: file.map(str::to_string),
            outcome: Passed,
            message: None,
        };
        // 1. The file attribute.
        assert_eq!(
            tests.attribute(&named("x", "suite", Some("src/other.test.ts"))),
            Some("src/other.test.ts".into())
        );
        // 2. The classname, when it is a selected file (vitest).
        assert_eq!(
            tests.attribute(&named("x", "src/a.test.ts", None)),
            Some("src/a.test.ts".into())
        );
        // 3. A unique name match (Node's reporter), by the last segment.
        assert_eq!(
            tests.attribute(&named("discount > applies at the threshold", "test", None)),
            Some("src/a.test.ts".into())
        );
        assert_eq!(
            tests.attribute(&named("applies at the threshold", "test", None)),
            Some("src/a.test.ts".into())
        );
        // 4. Otherwise unattributed: the root component, no location.
        assert_eq!(tests.attribute(&named("nowhere", "test", None)), None);
        let node = |name: &str, outcome: CaseOutcome| JunitCase {
            name: name.into(),
            classname: "test".into(),
            file: None,
            outcome,
            message: Some("\x1b[31mexpected 0, received 10\x1b[0m".into()),
        };
        let c = rows(
            &Observed {
                base: Some(vec![
                    node("applies at the threshold", Passed),
                    node("nowhere", Passed),
                ]),
                head: Some(vec![
                    node("applies at the threshold", Passed),
                    node("nowhere", Passed),
                ]),
                retry: None,
                cross: Some(vec![
                    node("applies at the threshold", Failed),
                    node("nowhere", Failed),
                ]),
            },
            &m,
            &m,
            &selected,
        );
        let ids: Vec<&str> = c.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["behavior-changed:root", "behavior-changed:shop"]);
        let shop = &c.rows[1];
        assert_eq!(shop.component.as_deref(), Some("shop"));
        assert_eq!(shop.level, ChangeLevel::Behavior);
        assert_eq!(shop.hints.confidence, Confidence::Traced);
        assert_eq!(shop.locations, [Location::base("src/a.test.ts", 10, 13)]);
        assert_eq!(shop.title, "Behavior pinned by tests changed in `shop`");
        assert_eq!(
            shop.why_it_matters,
            "The base version of \"applies at the threshold\" (`src/a.test.ts`) fails against the new code: expected 0, received 10. The change edited the test to match; confirm the new behavior is intended"
        );
        assert!(c.rows[0].locations.is_empty());

        // Many cases: at most 4 named, then how many more.
        let names = ["c1", "c2", "c3", "c4", "c5", "c6"];
        let many = |o: CaseOutcome| -> Vec<JunitCase> {
            names
                .iter()
                .map(|n| case("src/a.test.ts", n, o, Some("bug")))
                .collect()
        };
        let c = rows(
            &observed(many(Failed), many(Passed), None, None),
            &m,
            &m,
            &selected,
        );
        assert_eq!(
            c.rows[0].why_it_matters,
            "\"c1\" (`src/a.test.ts`), \"c2\" (`src/a.test.ts`), \"c3\" (`src/a.test.ts`), \"c4\" (`src/a.test.ts`) and 2 more failed at the base and pass at the head"
        );

        // Labels and owners come from the head map's component, sensitive
        // labels only.
        let mut report: SemanticReport = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "base": "b", "head": "h",
            "summary": { "meaningChanges": 0, "needsAttention": 0, "secrets": 0,
                         "newRuleViolations": 0, "intentMismatches": 0 },
            "changes": [], "ruleViolations": [],
            "structure": { "movedFiles": [], "formattingOnly": [] },
            "textStats": { "files": 0, "linesAdded": 0, "linesRemoved": 0 },
            "mapDiagnostics": []
        }))
        .unwrap();
        apply(&mut report, c.rows, block(), &m, None);
        assert_eq!(report.changes[0].hints.labels, ["payments"]);
        assert_eq!(report.changes[0].hints.owners, ["@shop-team"]);
        // Cases that ran at the head point at the head's lines.
        let c = rows(
            &observed(
                vec![case("src/a.test.ts", "gives 10 above", Failed, Some("bug"))],
                vec![case("src/a.test.ts", "gives 10 above", Passed, None)],
                None,
                None,
            ),
            &m,
            &m,
            &selected,
        );
        assert_eq!(
            c.rows[0].locations,
            [Location::head("src/a.test.ts", 20, 23)]
        );
        assert_eq!(c.rows[0].locations[0].side, Side::Head);
    }

    fn block() -> BehaviorRun {
        BehaviorRun {
            status: BehaviorStatus::Complete,
            reason: None,
            command: "npx vitest run {files}".into(),
            budget_secs: 600,
            elapsed_secs: 12,
            selected: vec![],
            truncated: 0,
            runs: vec![],
            flaky: vec![],
            already_failing: vec![],
        }
    }

    #[test]
    fn apply_reranks_and_recounts() {
        let m = map(&["a", "b"]);
        let mut report: SemanticReport = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "base": "b", "head": "h",
            "summary": { "meaningChanges": 1, "needsAttention": 0, "secrets": 0,
                         "newRuleViolations": 0, "intentMismatches": 0 },
            "changes": [{
                "id": "internal-changes:shop", "kind": "internal",
                "subkind": "internal-changes", "level": "behavior", "subject": "shop",
                "component": "shop", "kindLabel": "Internal",
                "title": "5 changed lines of code inside `shop`", "whyItMatters": "Stays inside",
                "hints": { "labels": [], "blastRadius": 0, "novelty": [], "confidence": "static",
                           "rulesOfTheGame": false, "intentMismatch": false, "needsPerson": false },
                "locations": []
            }],
            "ruleViolations": [],
            "structure": { "movedFiles": [], "formattingOnly": [] },
            "textStats": { "files": 1, "linesAdded": 5, "linesRemoved": 0 },
            "mapDiagnostics": []
        }))
        .unwrap();
        let c = rows(
            &observed(
                vec![pass("a"), fail("b", "bug")],
                vec![pass("a"), pass("b")],
                None,
                Some(vec![fail("a", "expected 0")]),
            ),
            &m,
            &m,
            &["src/a.test.ts".to_string()],
        );
        let mut changed = c.rows.clone();
        // A row that loses its needsPerson is given it back.
        changed[0].hints.needs_person = false;
        apply(&mut report, changed, block(), &m, None);
        let ids: Vec<&str> = report.changes.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "behavior-changed:shop",
                "behavior-newly-passing:shop",
                "internal-changes:shop"
            ]
        );
        assert_eq!(report.summary, crate::summarize(&report.changes));
        assert_eq!(report.summary.meaning_changes, 3);
        assert_eq!(report.summary.needs_attention, 1);
        assert_eq!(report.behavior, Some(block()));
        // Applying again replaces the rows rather than adding to them.
        let once = report.clone();
        apply(&mut report, c.rows.clone(), block(), &m, None);
        assert_eq!(report, once);
        // An id another row already has gets a suffix.
        let mut clash = c.rows[0].clone();
        clash.subkind = "test-weakened".into();
        report.changes.push(clash);
        apply(&mut report, c.rows, block(), &m, None);
        assert!(
            report
                .changes
                .iter()
                .any(|r| r.id == "behavior-changed:shop#2")
        );
    }

    #[test]
    fn normalize_message() {
        assert_eq!(
            super::normalize_message(
                "\x1b[31mAssertionError\x1b[0m:  expected 0\n\n  to be 10 (12.5 ms, 3s)\n at 0x7ffee4 /work/src/a.ts:3"
            ),
            "AssertionError: expected 0 to be 10 (, ) at src/a.ts:3"
        );
        assert_eq!(
            super::normalize_message(&"x ".repeat(400)).chars().count(),
            300
        );
    }
}
