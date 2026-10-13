//! Renders a [`SemanticReport`] as Markdown (for a pull request comment or a
//! job summary) or as JSON. Both are deterministic: the same report always
//! renders to the same bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use onus_core::{ReportCoverage, SemanticChange, SemanticReport, Side, TestReach};

/// Hidden marker so a bot can find and update its own comment.
pub const COMMENT_MARKER: &str = "<!-- onus-report -->";

pub fn to_json(report: &SemanticReport) -> String {
    let mut s = serde_json::to_string_pretty(report).expect("reports serialize");
    s.push('\n');
    s
}

fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn plural(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", thousands(n))
    }
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// The one-line headline.
pub fn title_line(report: &SemanticReport) -> String {
    let lines = report.text_stats.lines_added + report.text_stats.lines_removed;
    let size = format!(
        "{} in {}",
        plural(lines, "line", "lines"),
        plural(report.text_stats.files, "file", "files")
    );
    let n = report.summary.meaning_changes;
    if n == 0 {
        return format!("Onus · no changes in meaning · {size}");
    }
    format!(
        "Onus · {} · {} · {size}",
        plural(n, "meaning-level change", "meaning-level changes"),
        if report.summary.needs_attention == 1 {
            "1 needs attention".to_string()
        } else {
            format!(
                "{} need attention",
                thousands(report.summary.needs_attention)
            )
        }
    )
}

fn intent_line(report: &SemanticReport) -> String {
    match &report.intent_check {
        None => "not stated (add an `onus-intent` block to the pull request)".into(),
        Some(i) if i.mismatches.is_empty() => "matches stated intent".into(),
        Some(i) => format!(
            "**{} outside stated intent**",
            plural(i.mismatches.len() as u32, "change", "changes")
        ),
    }
}

fn rules_line(report: &SemanticReport, rules_declared: bool) -> String {
    match report.rule_violations.len() {
        0 if !rules_declared => "none declared".into(),
        0 => "no new violations".into(),
        n => format!(
            "**{}**",
            plural(n as u32, "new violation", "new violations")
        ),
    }
}

/// ` · **Tests ran:** 34 of 40 changed lines (85%)`
fn coverage_line(cov: &ReportCoverage) -> String {
    let ran = if cov.changed > 0 {
        format!(
            "{} of {} ({}%)",
            thousands(cov.ran),
            plural(cov.changed, "changed line", "changed lines"),
            // Rounded down, so 100% means every line ran.
            u64::from(cov.ran) * 100 / u64::from(cov.changed)
        )
    } else if cov.not_measured.is_empty() {
        "no changed executable lines".into()
    } else {
        "no measured changed lines".into()
    };
    if cov.not_measured.is_empty() {
        format!(" · **Tests ran:** {ran}")
    } else {
        format!(
            " · **Tests ran:** {ran}; {} not in the coverage",
            plural(
                cov.not_measured.len() as u32,
                "changed file",
                "changed files"
            )
        )
    }
}

/// `3–5, 9`, and whether that is more than one line.
fn line_list(lines: &[[u32; 2]]) -> (String, bool) {
    let parts: Vec<String> = lines
        .iter()
        .map(|[a, b]| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}–{b}")
            }
        })
        .collect();
    let many = lines.len() > 1 || lines.first().is_some_and(|[a, b]| a != b);
    (parts.join(", "), many)
}

/// The changed lines no test ran, the changed files the coverage leaves
/// out, and where the coverage came from.
fn coverage_details(out: &mut String, cov: &ReportCoverage) {
    if cov.not_run.is_empty() && cov.not_measured.is_empty() && cov.notes.is_empty() {
        return;
    }
    let mut by_file: BTreeMap<&str, Vec<[u32; 2]>> = BTreeMap::new();
    for l in &cov.not_run {
        by_file.entry(l.file.as_str()).or_default().push(l.lines);
    }
    let lines: u32 = cov
        .not_run
        .iter()
        .map(|l| l.lines[1] - l.lines[0] + 1)
        .sum();
    let _ = writeln!(
        out,
        "<details><summary>Changed lines no test ran: {}</summary>",
        match (lines, cov.not_measured.len() as u32) {
            (0, 0) => "none".to_string(),
            (0, files) => format!("{} not in the coverage", plural(files, "file", "files")),
            (lines, 0) => format!(
                "{} in {}",
                plural(lines, "line", "lines"),
                plural(by_file.len() as u32, "file", "files")
            ),
            (lines, files) => format!(
                "{} in {}; {} not in the coverage",
                plural(lines, "line", "lines"),
                plural(by_file.len() as u32, "file", "files"),
                plural(files, "file", "files")
            ),
        }
    );
    let _ = writeln!(out);
    for (file, ranges) in &by_file {
        let (list, many) = line_list(ranges);
        let _ = writeln!(
            out,
            "- `{file}` {} {list}",
            if many { "lines" } else { "line" }
        );
    }
    for f in &cov.not_measured {
        let _ = writeln!(out, "- not in the coverage (no test loaded it): `{f}`");
    }
    for n in &cov.notes {
        let _ = writeln!(out, "- {n}");
    }
    for s in &cov.sources {
        let _ = writeln!(
            out,
            "- coverage: {} `{}` ({}, {})",
            s.format.as_str(),
            s.path,
            s.origin.as_str(),
            plural(s.files, "file", "files")
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "</details>");
    let _ = writeln!(out);
}

fn kind_cell(c: &SemanticChange) -> String {
    if c.hints.intent_mismatch {
        format!("Outside stated intent · {}", c.kind_label)
    } else {
        c.kind_label.clone()
    }
}

/// Locations grouped by file: `path` lines 3–5, 9.
fn evidence(c: &SemanticChange) -> Vec<String> {
    let mut by_file: BTreeMap<(&str, Side), Vec<[u32; 2]>> = BTreeMap::new();
    for l in &c.locations {
        by_file
            .entry((l.file.as_str(), l.side))
            .or_default()
            .push(l.lines);
    }
    by_file
        .into_iter()
        .map(|((file, side), lines)| {
            let (list, many) = line_list(&lines);
            let word = if many { "lines" } else { "line" };
            let side = match side {
                Side::Head => "",
                Side::Base => " (before the change)",
            };
            format!("`{file}` {word} {list}{side}")
        })
        .collect()
}

/// Renders the Markdown report. `rules_declared` says whether the map had
/// boundary rules, so "no new violations" is not claimed without rules.
pub fn to_markdown(report: &SemanticReport, rules_declared: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{COMMENT_MARKER}");
    let _ = writeln!(out, "### {}", title_line(report));
    let _ = writeln!(out);
    if report.changes.is_empty() {
        let _ = writeln!(
            out,
            "No changes in meaning: only formatting, comments, moved files or other structure changed."
        );
    } else {
        let _ = writeln!(out, "|   | Change | Kind | Why it matters |");
        let _ = writeln!(out, "|---|---|---|---|");
        for c in &report.changes {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                if c.hints.needs_person { "●" } else { "○" },
                cell(&c.title),
                cell(&kind_cell(c)),
                cell(&c.why_it_matters)
            );
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "**Intent check:** {} · **Boundary rules:** {}{}",
        intent_line(report),
        rules_line(report, rules_declared),
        report
            .coverage
            .as_ref()
            .map(coverage_line)
            .unwrap_or_default()
    );
    if let Some(i) = &report.intent_check
        && !i.mismatches.is_empty()
    {
        let _ = writeln!(out);
        if !i.stated.is_empty() {
            let _ = writeln!(out, "Stated intent: \"{}\"", i.stated);
            let _ = writeln!(out);
        }
        let titles: BTreeMap<&str, &str> = report
            .changes
            .iter()
            .map(|c| (c.id.as_str(), c.title.as_str()))
            .collect();
        for m in &i.mismatches {
            let _ = writeln!(
                out,
                "- {}: {}",
                titles
                    .get(m.change_id.as_str())
                    .copied()
                    .unwrap_or(&m.change_id),
                m.reason
            );
        }
    }
    let _ = writeln!(out);
    if let Some(cov) = &report.coverage {
        coverage_details(&mut out, cov);
    }

    if !report.changes.is_empty() {
        let _ = writeln!(
            out,
            "<details><summary>Evidence per row (files and lines)</summary>"
        );
        let _ = writeln!(out);
        for (i, c) in report.changes.iter().enumerate() {
            let _ = writeln!(out, "{}. **{}**", i + 1, c.title);
            let mut facts = vec![
                format!("kind `{}` / `{}`", c.kind.as_str(), c.subkind),
                format!("confidence `{}`", c.hints.confidence.as_str()),
            ];
            if !c.hints.labels.is_empty() {
                facts.push(format!("labels {}", c.hints.labels.join(", ")));
            }
            if c.hints.blast_radius > 0 {
                facts.push(
                    plural(c.hints.blast_radius, "dependent file", "dependent files").to_string(),
                );
            }
            if c.hints.dependent_components > 0 {
                facts.push(format!(
                    "used by {}",
                    plural(
                        c.hints.dependent_components,
                        "other component",
                        "other components"
                    )
                ));
            }
            if let Some(t) = c.hints.tested {
                facts.push(
                    match t {
                        TestReach::Tested => "tested",
                        TestReach::Traced => "traced, not tested",
                        TestReach::Untested => "untested",
                    }
                    .into(),
                );
            }
            if let Some(cov) = c.hints.coverage {
                facts.push(format!(
                    "tests ran {} of {}",
                    thousands(cov.ran),
                    plural(cov.changed, "changed line", "changed lines")
                ));
            }
            if !c.hints.novelty.is_empty() {
                facts.push(format!("novelty {}", c.hints.novelty.join(", ")));
            }
            if c.hints.rules_of_the_game {
                facts.push("rules of the game".into());
            }
            if let Some(s) = &c.stats {
                facts.push(format!(
                    "+{} −{} in {}",
                    thousands(s.lines_added),
                    thousands(s.lines_removed),
                    plural(s.files, "file", "files")
                ));
            }
            let _ = writeln!(out, "   - {}", facts.join(" · "));
            for e in evidence(c) {
                let _ = writeln!(out, "   - {e}");
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "</details>");
        let _ = writeln!(out);
    }

    let s = &report.structure;
    if !s.moved_files.is_empty() || !s.formatting_only.is_empty() {
        let _ = writeln!(
            out,
            "<details><summary>Structure only: {} moved, {} formatting only</summary>",
            plural(s.moved_files.len() as u32, "file", "files"),
            plural(s.formatting_only.len() as u32, "file", "files")
        );
        let _ = writeln!(out);
        for m in &s.moved_files {
            let _ = writeln!(out, "- moved `{}` → `{}`", m.from, m.to);
        }
        for f in &s.formatting_only {
            let _ = writeln!(out, "- formatting, comments or import paths only: `{f}`");
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "</details>");
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "<details><summary>Map confidence notes</summary>");
    let _ = writeln!(out);
    if report.map_diagnostics.is_empty() {
        let _ = writeln!(
            out,
            "Every import, call and event in the changed files was resolved statically."
        );
    } else {
        for d in &report.map_diagnostics {
            if d.file.is_empty() {
                let _ = writeln!(out, "- {}: {}", d.kind, d.message);
                continue;
            }
            let _ = writeln!(
                out,
                "- `{}:{}` {}: {} (confidence `{}`)",
                d.file,
                d.line,
                d.kind,
                d.message,
                d.confidence.as_str()
            );
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "</details>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_core::{LineCoverage, Location};
    use serde_json::json;

    fn report() -> SemanticReport {
        serde_json::from_value(json!({
            "schemaVersion": 1, "base": "base", "head": "head",
            "summary": { "meaningChanges": 1, "needsAttention": 0, "secrets": 0,
                         "newRuleViolations": 0, "intentMismatches": 0 },
            "changes": [{
                "id": "internal-changes:billing", "kind": "internal",
                "subkind": "internal-changes", "level": "behavior", "subject": "billing",
                "component": "billing", "kindLabel": "Internal",
                "title": "5 changed lines of code inside `billing`", "whyItMatters": "Stays inside",
                "hints": { "labels": [], "blastRadius": 0, "novelty": [], "confidence": "static",
                           "rulesOfTheGame": false, "intentMismatch": false, "needsPerson": false },
                "locations": [{ "file": "src/discount.ts", "lines": [21, 28], "side": "head" }],
            }],
            "ruleViolations": [],
            "structure": { "movedFiles": [], "formattingOnly": [] },
            "textStats": { "files": 1, "linesAdded": 5, "linesRemoved": 0 },
            "mapDiagnostics": [],
        }))
        .unwrap()
    }

    fn covered(r: &mut SemanticReport) {
        r.changes[0].hints.coverage = Some(LineCoverage { changed: 5, ran: 3 });
        r.coverage = Some(
            serde_json::from_value(json!({
                "sources": [{ "format": "lcov", "path": "coverage/lcov.info", "sha256": "ab",
                              "origin": "supplied", "files": 41, "unmatched": 3 }],
                "changed": 40, "ran": 34,
                "notRun": [
                    Location::head("src/eta.ts", 13, 13),
                    Location::head("src/discount.ts", 21, 23),
                    Location::head("src/discount.ts", 27, 28),
                ],
                "notMeasured": ["src/refunds.ts"],
            }))
            .unwrap(),
        );
    }

    #[test]
    fn coverage_shows_the_lines_no_test_ran() {
        let mut r = report();
        let before = to_markdown(&r, true);
        covered(&mut r);
        let md = to_markdown(&r, true);
        assert!(md.contains(
            "**Boundary rules:** no new violations · **Tests ran:** 34 of 40 changed lines (85%); 1 changed file not in the coverage\n"
        ));
        assert!(md.contains(
            "<details><summary>Changed lines no test ran: 6 lines in 2 files; 1 file not in the coverage</summary>\n\n\
             - `src/discount.ts` lines 21–23, 27–28\n\
             - `src/eta.ts` line 13\n\
             - not in the coverage (no test loaded it): `src/refunds.ts`\n\
             - coverage: lcov `coverage/lcov.info` (supplied, 41 files)\n\n</details>\n\n\
             <details><summary>Evidence per row"
        ));
        assert!(md.contains("· tests ran 3 of 5 changed lines\n"));
        // The headline stays as it was.
        assert_eq!(md.lines().nth(1), before.lines().nth(1));
        // Without coverage, the report is as before.
        r.coverage = None;
        r.changes[0].hints.coverage = None;
        assert_eq!(to_markdown(&r, true), before);
        assert!(!before.contains("Tests ran") && !before.contains("tests ran"));
    }

    #[test]
    fn no_changed_executable_lines() {
        let mut r = report();
        covered(&mut r);
        let cov = r.coverage.as_mut().unwrap();
        (cov.changed, cov.ran) = (0, 0);
        cov.not_run.clear();
        cov.not_measured.clear();
        let md = to_markdown(&r, false);
        assert!(md.contains("· **Tests ran:** no changed executable lines\n"));
        assert!(!md.contains("Changed lines no test ran"));
        // A changed file nothing measured is not "no changed lines".
        let cov = r.coverage.as_mut().unwrap();
        cov.not_measured = vec!["src/eta.ts".into()];
        let md = to_markdown(&r, false);
        assert!(md.contains(
            "· **Tests ran:** no measured changed lines; 1 changed file not in the coverage\n"
        ));
        assert!(md.contains(
            "<details><summary>Changed lines no test ran: 1 file not in the coverage</summary>"
        ));
    }
}
