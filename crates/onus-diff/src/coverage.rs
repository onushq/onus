//! Which changed lines tests ran, from coverage of the head tree (ADR 0017).
//!
//! A changed line is an added or edited head line. It counts when the
//! coverage says it is executable, and it ran when some test ran it. Each
//! row gets the count for the lines its head locations cover; the report
//! gets the total, the changed lines no test ran, and the changed files with
//! runtime code that the coverage does not include.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onus_core::{
    LineCoverage, Location, ReportCoverage, SemanticChange, Side, SymbolKind, line_ranges,
};

use crate::ctx::Ctx;
use crate::text::Status;

/// Symbols that hold code that runs, as opposed to types.
fn runs(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Class
            | SymbolKind::Const
            | SymbolKind::Variable
    )
}

/// Sets `hints.coverage` on the rows and gives the report's coverage, when
/// the head map has coverage.
pub fn attach(ctx: &Ctx, rows: &mut [SemanticChange]) -> Option<ReportCoverage> {
    let cov = ctx.head.coverage.as_ref()?;
    let measured: HashMap<&str, (BTreeSet<u32>, BTreeSet<u32>)> = cov
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.lines()))
        .collect();
    let runtime: BTreeSet<&str> = ctx
        .head
        .symbols
        .iter()
        .filter(|s| runs(s.kind))
        .filter_map(|s| s.loc.as_ref().map(|l| l.file.as_str()))
        .collect();
    // File → (changed executable lines, those that ran).
    let mut changed: BTreeMap<&str, (BTreeSet<u32>, BTreeSet<u32>)> = BTreeMap::new();
    let mut not_measured = Vec::new();
    let mut not_run = Vec::new();
    for f in &ctx.text.files {
        let path = f.path.as_str();
        if f.status == Status::Deleted
            || ctx.is_test_file(path)
            || ctx.is_test_data(path)
            || !ctx.head_files.contains_key(path)
            || f.added_lines.is_empty()
        {
            continue;
        }
        let Some((covered, executable)) = measured.get(path) else {
            if runtime.contains(path) {
                not_measured.push(path.to_string());
            }
            continue;
        };
        let lines: BTreeSet<u32> = f
            .added_lines
            .iter()
            .map(|(n, _)| *n)
            .filter(|n| executable.contains(n))
            .collect();
        let ran: BTreeSet<u32> = lines.intersection(covered).copied().collect();
        let missed: BTreeSet<u32> = lines.difference(&ran).copied().collect();
        not_run.extend(
            line_ranges(&missed)
                .into_iter()
                .map(|[a, b]| Location::head(path, a, b)),
        );
        changed.insert(path, (lines, ran));
    }
    for r in rows.iter_mut() {
        let mut lines: BTreeSet<(&str, u32)> = BTreeSet::new();
        for l in r.locations.iter().filter(|l| l.side == Side::Head) {
            if let Some((file, (exec, _))) = changed.get_key_value(l.file.as_str()) {
                lines.extend(exec.range(l.lines[0]..=l.lines[1]).map(|n| (*file, *n)));
            }
        }
        if !lines.is_empty() {
            let ran = lines
                .iter()
                .filter(|(f, n)| changed[f].1.contains(n))
                .count() as u32;
            r.hints.coverage = Some(LineCoverage {
                changed: lines.len() as u32,
                ran,
            });
        }
    }
    let touched: Vec<String> = ctx
        .text
        .files
        .iter()
        .map(|f| format!("`{}`", f.path))
        .collect();
    Some(ReportCoverage {
        sources: cov.sources.clone(),
        changed: changed.values().map(|(c, _)| c.len() as u32).sum(),
        ran: changed.values().map(|(_, r)| r.len() as u32).sum(),
        not_run,
        not_measured,
        // Notes about a file, when it changed, and notes about a whole
        // source, such as a recorded file that is not coverage.
        notes: cov
            .notes
            .iter()
            .filter(|n| {
                !n.starts_with("coverage of `") || touched.iter().any(|t| n.contains(t.as_str()))
            })
            .cloned()
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::change;
    use crate::testing::{component, ctx, file, function, map};
    use crate::text::{FileChange, TreeDiff};
    use onus_core::{ChangeKind, ChangeLevel, Coverage, FileCoverage};
    use serde_json::json;

    fn edited(path: &str, added: &[u32]) -> FileChange {
        FileChange {
            path: path.into(),
            old_path: None,
            status: Status::Modified,
            binary: false,
            hunks: vec![],
            added: added.len() as u32,
            removed: 0,
            added_lines: added.iter().map(|n| (*n, String::new())).collect(),
        }
    }

    fn row(id: &str, locations: Vec<Location>) -> SemanticChange {
        change(
            ChangeKind::Internal,
            id,
            ChangeLevel::Behavior,
            id,
            Some("billing"),
            "",
            id.into(),
            String::new(),
            locations,
        )
    }

    #[test]
    fn counts_changed_lines_that_ran() {
        let mut head = map(json!({
            "components": [component("billing")],
            "files": [
                file("billing", "src/a.ts"), file("billing", "src/types.ts"),
                file("billing", "src/b.ts"), file("billing", "src/a.test.ts"),
            ],
            "symbols": [
                function("billing:src/a.ts#a", 1, "x"),
                function("billing:src/b.ts#b", 1, "y"),
                { "id": "billing:src/types.ts#T", "componentId": "billing", "kind": "type",
                  "name": "T", "visibility": "public",
                  "loc": { "file": "packages/billing/src/types.ts", "start": 1, "end": 3 } },
            ],
        }));
        head.coverage = Some(Coverage {
            files: vec![FileCoverage {
                path: "packages/billing/src/a.ts".into(),
                covered: vec![[1, 3], [6, 6]],
                uncovered: vec![[4, 5], [8, 9]],
            }],
            notes: vec![
                "coverage of `packages/billing/src/a.ts` is older than the file; left out (re-run the tests)".into(),
                "coverage of `packages/other.ts` is older than the file; left out (re-run the tests)".into(),
            ],
            ..Coverage::default()
        });
        let base = map(json!({}));
        let text = TreeDiff {
            files: vec![
                // Line 7 is not executable.
                edited("packages/billing/src/a.ts", &[2, 3, 4, 5, 6, 7, 9]),
                edited("packages/billing/src/a.test.ts", &[1]),
                edited("packages/billing/src/b.ts", &[1]),
                edited("packages/billing/src/types.ts", &[1]),
            ],
        };
        let c = ctx(&base, &head, &text, None);
        let mut rows = vec![
            row(
                "one",
                vec![Location::head("packages/billing/src/a.ts", 1, 4)],
            ),
            row(
                "two",
                vec![
                    Location::head("packages/billing/src/a.ts", 4, 9),
                    Location::head("packages/billing/src/a.ts", 9, 9),
                    // The base side's lines are not head lines.
                    Location::base("packages/billing/src/a.ts", 1, 3),
                ],
            ),
            row(
                "three",
                vec![Location::base("packages/billing/src/a.ts", 1, 9)],
            ),
        ];
        let cov = attach(&c, &mut rows).unwrap();
        assert_eq!((cov.changed, cov.ran), (6, 3));
        assert_eq!(
            cov.not_run,
            [
                Location::head("packages/billing/src/a.ts", 4, 5),
                Location::head("packages/billing/src/a.ts", 9, 9),
            ]
        );
        // A runtime file without coverage; not the type-only file or the test.
        assert_eq!(cov.not_measured, ["packages/billing/src/b.ts"]);
        assert_eq!(cov.notes.len(), 1);
        let hints: Vec<Option<LineCoverage>> = rows.iter().map(|r| r.hints.coverage).collect();
        assert_eq!(
            hints,
            [
                Some(LineCoverage { changed: 3, ran: 2 }),
                Some(LineCoverage { changed: 4, ran: 1 }),
                None,
            ]
        );
    }

    #[test]
    fn nothing_without_coverage() {
        let head = map(json!({}));
        let text = TreeDiff::default();
        let c = ctx(&head, &head, &text, None);
        assert_eq!(attach(&c, &mut []), None);
    }
}
