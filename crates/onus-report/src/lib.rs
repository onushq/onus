//! Renders a [`SemanticReport`] as Markdown (for a pull request comment or a
//! job summary) or as JSON. Both are deterministic: the same report always
//! renders to the same bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use onus_core::{SemanticChange, SemanticReport, Side};

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
            let word = if lines.len() == 1 && lines[0][0] == lines[0][1] {
                "line"
            } else {
                "lines"
            };
            let side = match side {
                Side::Head => "",
                Side::Base => " (before the change)",
            };
            format!("`{file}` {word} {}{side}", parts.join(", "))
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
        "**Intent check:** {} · **Boundary rules:** {}",
        intent_line(report),
        rules_line(report, rules_declared)
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
