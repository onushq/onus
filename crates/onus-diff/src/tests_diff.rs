//! Step 7: tests. New cases count as coverage gains (reported in the
//! component's internal row). Removed cases, fewer assertions, new
//! skip/only/todo markers, edited expected values and source code that
//! special-cases literals from the tests all count as weakened tests: rules
//! of the game.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::{ChangeKind, ChangeLevel, FactSite, Location, SemanticChange, TestCase, TestNode};

use crate::ctx::{Ctx, change, plural};
use crate::matching::Pairs;

#[derive(Debug, Default)]
pub struct TestsResult {
    pub rows: Vec<SemanticChange>,
    /// Component → number of new test cases.
    pub added_cases: BTreeMap<String, u32>,
}

#[derive(Debug, Default)]
struct Weakening {
    notes: Vec<String>,
    locations: Vec<Location>,
}

fn multiset_minus<'a>(a: &'a [FactSite], b: &[FactSite]) -> Vec<&'a FactSite> {
    let mut counts: BTreeMap<&str, i32> = BTreeMap::new();
    for x in b {
        *counts.entry(x.text.as_str()).or_default() += 1;
    }
    let mut out = Vec::new();
    for x in a {
        let c = counts.entry(x.text.as_str()).or_default();
        if *c > 0 {
            *c -= 1;
        } else {
            out.push(x);
        }
    }
    out
}

fn quote(name: &str) -> String {
    format!("\"{name}\"")
}

fn compare_cases(base: &TestNode, head: Option<&TestNode>, w: &mut Weakening) -> u32 {
    let head_cases: Vec<&TestCase> = head.map(|h| h.cases.iter().collect()).unwrap_or_default();
    // Cases can share a title (`it.each(...)("rejects %d")` twice): pair the
    // n-th base case of a title with the n-th head case of that title.
    let mut by_name: BTreeMap<&str, Vec<&TestCase>> = BTreeMap::new();
    for h in &head_cases {
        by_name.entry(h.name.as_str()).or_default().push(h);
    }
    for list in by_name.values_mut() {
        list.reverse();
    }
    let mut pairs: Vec<(&TestCase, Option<&TestCase>)> = base
        .cases
        .iter()
        .map(|b| (b, by_name.get_mut(b.name.as_str()).and_then(|l| l.pop())))
        .collect();
    let mut unmatched_head: Vec<&TestCase> = by_name.into_values().flatten().collect();
    unmatched_head.sort_by_key(|c| (c.line, c.name.clone()));
    let mut removed = Vec::new();
    for (b, h) in pairs.iter_mut() {
        let h = h.or_else(|| {
            // A renamed case keeps its body.
            let pos = unmatched_head
                .iter()
                .position(|h| !b.fingerprint.is_empty() && h.fingerprint == b.fingerprint)?;
            Some(unmatched_head.remove(pos))
        });
        let Some(h) = h else {
            removed.push(*b);
            continue;
        };
        let b = *b;
        let lost = multiset_minus(&b.assertions, &h.assertions);
        let gained = multiset_minus(&h.assertions, &b.assertions);
        if h.assertions.len() < b.assertions.len() {
            let n = (b.assertions.len() - h.assertions.len()) as u32;
            w.notes.push(format!(
                "{} removed from {}",
                plural(n, "assertion", "assertions"),
                quote(&h.name)
            ));
            for a in lost.iter().take(n as usize) {
                w.locations.push(Location::base(&base.file, a.line, a.line));
            }
        } else if !lost.is_empty() && lost.len() == gained.len() {
            // Same number of assertions, different expectations.
            let base_expected: Vec<&FactSite> = multiset_minus(&b.expected, &h.expected);
            let head_expected: Vec<&FactSite> = multiset_minus(&h.expected, &b.expected);
            if !base_expected.is_empty() && !head_expected.is_empty() {
                w.notes
                    .push(format!("expected values edited in {}", quote(&h.name)));
                for e in head_expected {
                    w.locations.push(Location::head(
                        head.map_or("", |n| n.file.as_str()).to_string(),
                        e.line,
                        e.line,
                    ));
                }
            }
        }
        for m in &h.markers {
            if !b.markers.contains(m) {
                let what = match m.as_str() {
                    "skip" => "is now skipped",
                    "only" => "now runs with `.only`, so other tests in the file are skipped",
                    _ => "is now marked todo",
                };
                w.notes.push(format!("{} {what}", quote(&h.name)));
                if let Some(head) = head {
                    w.locations.push(Location::head(&head.file, h.line, h.line));
                }
            }
        }
    }
    for b in removed {
        w.notes.push(format!("test {} removed", quote(&b.name)));
        w.locations
            .push(Location::base(&base.file, b.line, b.end_line));
    }
    unmatched_head.len() as u32
}

pub fn analyze(ctx: &Ctx, pairs: &Pairs) -> TestsResult {
    let mut result = TestsResult::default();
    let moves = ctx.moves.borrow();
    let base_by_file: BTreeMap<&str, &TestNode> = ctx
        .base
        .tests
        .iter()
        .map(|t| (t.file.as_str(), t))
        .collect();
    let mut seen_base = BTreeSet::new();
    let mut weak: BTreeMap<String, Weakening> = BTreeMap::new();
    for h in &ctx.head.tests {
        let base_path = moves.get(&h.file).map(String::as_str).unwrap_or(&h.file);
        let comp = h.component_id.clone().unwrap_or_else(|| "root".into());
        match base_by_file.get(base_path) {
            Some(b) => {
                seen_base.insert(b.file.as_str());
                // A test file that did not change cannot have been weakened.
                if b.file == h.file && ctx.text.get(&h.file).is_none() {
                    continue;
                }
                let w = weak.entry(comp.clone()).or_default();
                let added = compare_cases(b, Some(h), w);
                *result.added_cases.entry(comp).or_default() += added;
            }
            None => {
                *result.added_cases.entry(comp).or_default() += h.cases.len() as u32;
            }
        }
    }
    for b in &ctx.base.tests {
        if seen_base.contains(b.file.as_str()) {
            continue;
        }
        let comp = b.component_id.clone().unwrap_or_else(|| "root".into());
        let w = weak.entry(comp).or_default();
        if !b.cases.is_empty() {
            w.notes.push(format!(
                "test file `{}` deleted ({})",
                b.file,
                plural(b.cases.len() as u32, "case", "cases")
            ));
            w.locations.push(Location::base(&b.file, 1, 1));
        }
    }
    special_cases(ctx, pairs, &mut weak);

    for (comp, w) in weak {
        if w.notes.is_empty() {
            continue;
        }
        let source_changed = ctx.text.files.iter().any(|f| {
            ctx.component_of_path(&f.path) == comp
                && !ctx.is_test_file(&f.path)
                && ctx.head_files.contains_key(f.path.as_str())
        });
        let mut why = capitalize(&w.notes.join("; "));
        if source_changed {
            why.push_str(&format!(
                "; source in `{comp}` changed in the same pull request"
            ));
        }
        why.push_str("; needs a person");
        let mut row = change(
            ChangeKind::Test,
            "test-weakened",
            ChangeLevel::Behavior,
            &comp,
            Some(&comp),
            "Tests weakened",
            format!("Tests in `{comp}` were weakened"),
            why,
            w.locations,
        );
        row.hints.rules_of_the_game = true;
        row.hints.needs_person = true;
        result.rows.push(row);
    }
    result
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// New comparisons in source that test a string literal also used in the
/// component's tests, e.g. `if (orderId === "test-order-1")`.
fn special_cases(ctx: &Ctx, pairs: &Pairs, weak: &mut BTreeMap<String, Weakening>) {
    // Only literals the tests already used before this change, and only in
    // code that already existed: new code and its new tests naturally share
    // values, but existing code that starts matching an existing test input
    // is the pattern worth a look.
    let mut literals: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for t in &ctx.base.tests {
        let comp = t.component_id.clone().unwrap_or_else(|| "root".into());
        literals
            .entry(comp)
            .or_default()
            .extend(t.literals.iter().map(String::as_str));
    }
    let candidates = pairs.pairs.iter().map(|(b, h, _)| (Some(*b), *h));
    for (b, h) in candidates {
        let Some(hf) = &h.facts else {
            continue;
        };
        let comp = h.component_id.clone().unwrap_or_default();
        let Some(lits) = literals.get(&comp) else {
            continue;
        };
        let old: BTreeSet<(String, String, String)> = b
            .and_then(|b| b.facts.as_ref())
            .map(|f| {
                f.comparisons
                    .iter()
                    .map(|c| (c.op.clone(), c.left.clone(), c.right.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for c in &hf.comparisons {
            if old.contains(&(c.op.clone(), c.left.clone(), c.right.clone())) {
                continue;
            }
            let literal = [&c.left, &c.right].into_iter().find_map(|side| {
                let inner = side.strip_prefix('"')?.strip_suffix('"')?;
                lits.contains(inner).then(|| inner.to_string())
            });
            if let Some(lit) = literal {
                let line = h.loc.as_ref().map(|l| (l.file.as_str(), c.line));
                if is_domain_value(ctx, &comp, &lit, line) {
                    continue;
                }
                let w = weak.entry(comp.clone()).or_default();
                w.notes.push(format!(
                    "`{}` special-cases \"{lit}\", a value used in the tests",
                    h.name
                ));
                if let Some(l) = &h.loc {
                    w.locations.push(Location::head(&l.file, c.line, c.line));
                }
            }
        }
    }
}

/// Whether `literal` also appears in the component's source outside the
/// comparison itself (a type, a constant, a return value): then it is a
/// domain value that tests happen to use, not test data special-cased.
fn is_domain_value(ctx: &Ctx, comp: &str, literal: &str, at: Option<(&str, u32)>) -> bool {
    let quoted = [
        format!("\"{literal}\""),
        format!("'{literal}'"),
        format!("`{literal}`"),
    ];
    for f in ctx.head.files.iter() {
        if f.is_test || f.component_id.as_deref() != Some(comp) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(onus_core::paths::native(ctx.head_root, &f.path))
        else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if at == Some((f.path.as_str(), i as u32 + 1)) {
                continue;
            }
            if quoted.iter().any(|q| line.contains(q.as_str())) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str, line: u32, expected: &str) -> TestCase {
        TestCase {
            name: name.into(),
            line,
            end_line: line + 4,
            assertions: vec![FactSite {
                text: format!("expect(x).toThrow({expected})"),
                line: line + 1,
            }],
            markers: vec![],
            expected: vec![FactSite {
                text: format!("toThrow:{expected}"),
                line: line + 1,
            }],
            fingerprint: format!("fp-{expected}"),
        }
    }

    fn node(cases: Vec<TestCase>) -> TestNode {
        TestNode {
            id: "c:a.spec.ts".into(),
            component_id: Some("c".into()),
            file: "a.spec.ts".into(),
            cases,
            exercises: vec![],
            literals: vec![],
        }
    }

    #[test]
    fn cases_sharing_a_title_pair_in_order() {
        let cases = || {
            vec![
                case("rejects year %d", 10, "\"too low\""),
                case("rejects year %d", 20, "\"too high\""),
            ]
        };
        let mut w = Weakening::default();
        let added = compare_cases(&node(cases()), Some(&node(cases())), &mut w);
        assert_eq!(added, 0);
        assert!(w.notes.is_empty(), "{:?}", w.notes);

        // Dropping the second of the two still reads as a removed case.
        let mut w = Weakening::default();
        let fewer = node(vec![case("rejects year %d", 10, "\"too low\"")]);
        compare_cases(&node(cases()), Some(&fewer), &mut w);
        assert_eq!(w.notes, ["test \"rejects year %d\" removed"]);
    }
}
