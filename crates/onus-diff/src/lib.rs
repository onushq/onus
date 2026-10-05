//! The semantic diff engine (PLAN.md section 5.3).
//!
//! Given the base and head trees and their maps, it produces a ranked
//! [`SemanticReport`]: a few changes in meaning instead of thousands of
//! changed lines. Every row comes from deterministic analysis and fixed
//! templates; nothing is generated.

pub mod config_files;
mod contracts;
mod ctx;
pub mod intent;
mod internal;
mod matching;
mod notable;
mod packages;
mod relations;
mod rules;
mod secrets_scan;
mod tests_diff;
pub mod text;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use onus_core::rank;
use onus_core::{
    ChangeKind, CodebaseMap, Confidence, OnusConfig, ReportSummary, SCHEMA_VERSION, SemanticReport,
    Side, TextStats,
};
use onus_map::walk;

pub use intent::Intent;

/// Everything the diff needs.
#[derive(Debug, Clone, Copy)]
pub struct DiffInput<'a> {
    pub base_root: &'a Path,
    pub head_root: &'a Path,
    pub base_map: &'a CodebaseMap,
    pub head_map: &'a CodebaseMap,
    /// The config both maps were built with (for label sensitivity).
    pub config: Option<&'a OnusConfig>,
    pub intent: Option<&'a Intent>,
    pub base_label: &'a str,
    pub head_label: &'a str,
}

pub fn diff(input: &DiffInput) -> SemanticReport {
    let base_files = walk::list_files(input.base_root);
    let head_files = walk::list_files(input.head_root);
    let text = text::diff_trees(input.base_root, &base_files, input.head_root, &head_files);
    let ctx = ctx::Ctx::new(
        input.base_map,
        input.head_map,
        input.base_root,
        input.head_root,
        &text,
        input.config,
    );
    internal::detect_moves(&ctx);
    let pairs = matching::match_symbols(&ctx);

    let mut rows = Vec::new();
    rows.extend(matching::rename_rows(&ctx, &pairs));
    rows.extend(contracts::rows(&ctx, &pairs));
    let violations = rules::rows(&ctx, &pairs);
    rows.extend(relations::rows(&ctx, &violations));
    packages::rows(&ctx, &mut rows);
    rows.extend(config_files::rows(&ctx));
    let tests = tests_diff::analyze(&ctx, &pairs);
    rows.extend(tests.rows.iter().cloned());
    rows.extend(notable::rows(&ctx, &pairs));
    rows.extend(secrets_scan::rows(&ctx));
    rows.extend(violations);
    let (internal_rows, structure) = internal::rows(&ctx, &rows, &tests);
    rows.extend(internal_rows);

    // Hints that depend on the whole picture.
    let changed: BTreeSet<&str> = text.files.iter().map(|f| f.path.as_str()).collect();
    let low_files: BTreeSet<&str> = input
        .head_map
        .diagnostics
        .iter()
        .filter(|d| d.confidence == Confidence::Low && changed.contains(d.file.as_str()))
        .map(|d| d.file.as_str())
        .collect();
    for r in &mut rows {
        if r.hints.labels.is_empty() {
            if let Some(c) = &r.component {
                r.hints.labels = ctx.labels(c);
            }
        }
        if r.locations
            .iter()
            .any(|l| l.side == Side::Head && low_files.contains(l.file.as_str()))
        {
            r.hints.confidence = Confidence::Low;
        }
        r.hints.novelty.sort();
        r.hints.novelty.dedup();
        r.locations.sort();
        r.locations.dedup();
        if matches!(r.kind, ChangeKind::SecuritySensitive | ChangeKind::Breaking)
            || r.hints.rules_of_the_game
        {
            r.hints.needs_person = true;
        }
    }
    // Unique ids.
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for r in &mut rows {
        let n = seen.entry(r.id.clone()).or_default();
        *n += 1;
        if *n > 1 {
            r.id = format!("{}#{}", r.id, n);
        }
    }

    let intent_check = input
        .intent
        .map(|i| intent::check(i, &mut rows, &input.head_map.externals));
    rank::rank(&mut rows);

    let rule_violations: Vec<_> = rows
        .iter()
        .filter(|r| r.subkind == "rule-violation")
        .cloned()
        .collect();
    let summary = ReportSummary {
        meaning_changes: rows.len() as u32,
        needs_attention: rows.iter().filter(|r| r.hints.needs_person).count() as u32,
        secrets: rows
            .iter()
            .filter(|r| r.subkind == "secret-committed")
            .count() as u32,
        new_rule_violations: rule_violations.len() as u32,
        intent_mismatches: rows.iter().filter(|r| r.hints.intent_mismatch).count() as u32,
    };
    let mut map_diagnostics: Vec<_> = input
        .head_map
        .diagnostics
        .iter()
        .filter(|d| changed.contains(d.file.as_str()))
        .cloned()
        .collect();
    map_diagnostics.sort();
    map_diagnostics.dedup();

    SemanticReport {
        schema_version: SCHEMA_VERSION,
        base: input.base_label.to_string(),
        head: input.head_label.to_string(),
        summary,
        changes: rows,
        intent_check,
        rule_violations,
        structure,
        text_stats: TextStats {
            files: text.files.len() as u32,
            lines_added: text.lines_added(),
            lines_removed: text.lines_removed(),
        },
        map_diagnostics,
    }
}
