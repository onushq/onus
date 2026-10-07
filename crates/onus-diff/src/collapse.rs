//! Changes that repeat across many components (a migration touching every
//! project's tsconfig) read as one row, and the internal rows of a very
//! large change keep the biggest components and summarize the rest.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::rank::SUBKIND_INTERNAL_CHANGES;
use onus_core::{ChangeKind, Confidence, SemanticChange, Stats};

use crate::ctx::{join_some, plural, thousands};

/// A kind of change in more components than this becomes one row.
pub const WIDESPREAD_COMPONENTS: usize = 5;
/// Internal rows shown one by one; the rest are summarized.
pub const MAX_INTERNAL_ROWS: usize = 10;

fn component_name(r: &SemanticChange) -> String {
    r.component.clone().unwrap_or_else(|| "root".into())
}

fn widespread_candidate(r: &SemanticChange) -> bool {
    match r.kind {
        ChangeKind::Config => true,
        ChangeKind::Dependency => r.hints.novelty.is_empty(),
        _ => false,
    }
}

pub fn widespread(rows: Vec<SemanticChange>) -> Vec<SemanticChange> {
    // Grouped by subkind and, for config, by the kind of edit: six configs
    // dropping `module` and two dropping `exclude` are two patterns.
    let mut by_subkind: BTreeMap<(String, String), Vec<SemanticChange>> = BTreeMap::new();
    let mut internal = Vec::new();
    let mut out = Vec::new();
    for r in rows {
        if r.subkind == SUBKIND_INTERNAL_CHANGES {
            internal.push(r);
        } else if widespread_candidate(&r) {
            let pattern = if r.kind == ChangeKind::Config {
                crate::ctx::edit_pattern(&r.title)
            } else {
                String::new()
            };
            by_subkind
                .entry((r.subkind.clone(), pattern))
                .or_default()
                .push(r);
        } else {
            out.push(r);
        }
    }
    let mut ids: BTreeMap<String, u32> = BTreeMap::new();
    for ((subkind, pattern), members) in by_subkind {
        let components: BTreeSet<String> = members.iter().map(component_name).collect();
        if components.len() <= WIDESPREAD_COMPONENTS {
            out.extend(members);
            continue;
        }
        let files: BTreeSet<&str> = members
            .iter()
            .flat_map(|m| m.locations.iter().map(|l| l.file.as_str()))
            .collect();
        let names: Vec<String> = components.iter().map(|c| format!("`{c}`")).collect();
        let first = &members[0];
        let mut row = first.clone();
        let n = ids.entry(subkind.clone()).or_default();
        *n += 1;
        row.id = if *n == 1 {
            format!("{subkind}:widespread")
        } else {
            format!("{subkind}:widespread#{n}")
        };
        row.subject = "repository".into();
        row.component = None;
        let counts = format!(
            "{} ({})",
            plural(components.len() as u32, "component", "components"),
            plural(files.len() as u32, "file", "files")
        );
        row.title = if pattern.is_empty() {
            format!("{}: changes in {counts}", first.kind_label)
        } else {
            format!("{} changes {pattern} in {counts}", first.kind_label)
        };
        row.why_it_matters = format!(
            "The same kind of change in {}; review the pattern once rather than file by file",
            join_some(&names, 4)
        );
        row.locations = members.iter().flat_map(|m| m.locations.clone()).collect();
        row.locations.sort();
        row.locations.dedup();
        row.hints.labels = members
            .iter()
            .flat_map(|m| m.hints.labels.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        row.hints.novelty = vec![];
        row.hints.rules_of_the_game = members.iter().any(|m| m.hints.rules_of_the_game);
        row.hints.needs_person = members.iter().any(|m| m.hints.needs_person);
        row.hints.blast_radius = members
            .iter()
            .map(|m| m.hints.blast_radius)
            .max()
            .unwrap_or(0);
        out.push(row);
    }
    if internal.len() > MAX_INTERNAL_ROWS {
        let size = |r: &SemanticChange| r.stats.map_or(0, |s| s.lines_added + s.lines_removed);
        internal.sort_by(|a, b| size(b).cmp(&size(a)).then_with(|| a.id.cmp(&b.id)));
        let rest = internal.split_off(MAX_INTERNAL_ROWS);
        let stats = rest.iter().fold(Stats::default(), |acc, r| {
            let s = r.stats.unwrap_or_default();
            Stats {
                lines_added: acc.lines_added + s.lines_added,
                lines_removed: acc.lines_removed + s.lines_removed,
                files: acc.files + s.files,
            }
        });
        let mut names: Vec<String> = rest
            .iter()
            .map(|r| format!("`{}`", component_name(r)))
            .collect();
        names.sort();
        let mut row = rest[0].clone();
        row.id = format!("{SUBKIND_INTERNAL_CHANGES}:others");
        row.subject = "repository".into();
        row.component = None;
        row.title = format!(
            "{} changed lines inside {} other components",
            thousands(stats.lines_added + stats.lines_removed),
            rest.len()
        );
        row.why_it_matters = format!("Smaller internal changes in {}", join_some(&names, 4));
        row.locations = rest.iter().flat_map(|r| r.locations.clone()).collect();
        row.locations.sort();
        row.locations.dedup();
        row.stats = Some(stats);
        row.hints.labels = vec![];
        row.hints.blast_radius = 0;
        row.hints.confidence = rest
            .iter()
            .map(|r| r.hints.confidence)
            .max()
            .unwrap_or(Confidence::Static);
        internal.push(row);
    }
    out.extend(internal);
    out
}
