//! Structure-only changes (moves, formatting) and step 10: everything not
//! explained by another row collapses into one internal row per component.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onus_core::rank::SUBKIND_INTERNAL_CHANGES;
use onus_core::{
    ChangeKind, ChangeLevel, Confidence, EdgeKind, Location, MovedFile, SemanticChange, Side,
    SourceFile, Stats, StructureNotes,
};

use crate::ctx::{Ctx, change, join_and, join_some, plural, ranges, thousands};
use crate::tests_diff::TestsResult;
use crate::text::Status;

fn imports_equal(base: &SourceFile, head: &SourceFile, moves: &BTreeMap<String, String>) -> bool {
    base.imports.len() == head.imports.len()
        && base.imports.iter().zip(&head.imports).all(|(b, h)| {
            let mapped = moves.get(h).map(String::as_str).unwrap_or(h);
            mapped == b
        })
}

/// Adds files that moved with only formatting or import-path edits to
/// `ctx.moves`.
pub fn detect_moves(ctx: &Ctx) {
    let deleted: Vec<&SourceFile> = ctx
        .text
        .files
        .iter()
        .filter(|f| f.status == Status::Deleted)
        .filter_map(|f| ctx.base_files.get(f.path.as_str()).copied())
        .collect();
    let added: Vec<&SourceFile> = ctx
        .text
        .files
        .iter()
        .filter(|f| f.status == Status::Added)
        .filter_map(|f| ctx.head_files.get(f.path.as_str()).copied())
        .collect();
    let mut taken = BTreeSet::new();
    let mut pairs = Vec::new();
    for a in &added {
        if let Some(d) = deleted
            .iter()
            .find(|d| !taken.contains(&d.path) && d.content_hash == a.content_hash && d.lines > 0)
        {
            taken.insert(d.path.clone());
            pairs.push((a.path.clone(), d.path.clone()));
        }
    }
    let mut moves = ctx.moves.borrow_mut();
    for (h, b) in pairs {
        moves.insert(h, b);
    }
}

/// Modified or moved files whose tokens and resolved imports are unchanged.
pub fn formatting_only(ctx: &Ctx) -> BTreeSet<String> {
    let moves = ctx.moves.borrow();
    let mut out = BTreeSet::new();
    for f in &ctx.text.files {
        if !matches!(f.status, Status::Modified | Status::Added) {
            continue;
        }
        let base_path = match f.status {
            Status::Added => match moves.get(&f.path) {
                Some(b) => b.as_str(),
                None => continue,
            },
            _ => f.path.as_str(),
        };
        let (Some(b), Some(h)) = (
            ctx.base_files.get(base_path),
            ctx.head_files.get(f.path.as_str()),
        ) else {
            continue;
        };
        if b.content_hash == h.content_hash && imports_equal(b, h, &moves) {
            out.insert(f.path.clone());
        }
    }
    out
}

#[derive(Default)]
struct Bucket {
    added: u32,
    removed: u32,
    files: BTreeSet<String>,
    head_lines: BTreeMap<String, BTreeSet<u32>>,
    base_lines: BTreeMap<String, BTreeSet<u32>>,
    tests: bool,
    code: bool,
    docs: bool,
}

/// Prose rather than code: Markdown, text and files under `docs/`.
pub fn is_docs(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".mdx", ".markdown", ".txt", ".rst", ".adoc"]
        .iter()
        .any(|e| lower.ends_with(e))
        || lower.starts_with("docs/")
        || lower.contains("/docs/")
}

/// The bucket of files onus.yaml declares test data (`#` cannot appear in a
/// component id).
const TEST_DATA: &str = "#test-data";

pub fn rows(
    ctx: &Ctx,
    existing: &[SemanticChange],
    tests: &TestsResult,
) -> (Vec<SemanticChange>, StructureNotes) {
    let formatting = formatting_only(ctx);
    let moves = ctx.moves.borrow().clone();
    let mut cov_head: HashMap<&str, BTreeSet<u32>> = HashMap::new();
    let mut cov_base: HashMap<&str, BTreeSet<u32>> = HashMap::new();
    for r in existing {
        for l in &r.locations {
            let set = match l.side {
                Side::Head => cov_head.entry(l.file.as_str()).or_default(),
                Side::Base => cov_base.entry(l.file.as_str()).or_default(),
            };
            set.extend(l.lines[0]..=l.lines[1]);
        }
    }
    let explained = ctx.explained.borrow().clone();
    let deleted_moved: BTreeSet<&str> = moves.values().map(String::as_str).collect();
    let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
    for f in &ctx.text.files {
        if f.status == Status::Renamed
            || explained.contains(&f.path)
            || formatting.contains(&f.path)
            || (f.status == Status::Deleted && deleted_moved.contains(f.path.as_str()))
        {
            continue;
        }
        let comp = if ctx.is_test_data(&f.path) {
            TEST_DATA.to_string()
        } else {
            ctx.component_of_path(&f.path)
        };
        let mut uncovered = false;
        let mut bucket = Bucket::default();
        if f.binary {
            uncovered = true;
        }
        for h in &f.hunks {
            if !h.head_lines.is_empty() {
                let cov = cov_head.get(f.path.as_str());
                let unc: Vec<u32> = h
                    .head_lines
                    .iter()
                    .copied()
                    .filter(|l| !cov.is_some_and(|c| c.contains(l)))
                    .collect();
                if unc.is_empty() {
                    continue;
                }
                uncovered = true;
                bucket.added += unc.len() as u32;
                bucket.removed += h.base_lines.len() as u32;
                bucket
                    .head_lines
                    .entry(f.path.clone())
                    .or_default()
                    .extend(unc);
                if !h.base_lines.is_empty() {
                    bucket
                        .base_lines
                        .entry(f.base_path().to_string())
                        .or_default()
                        .extend(h.base_lines.iter().copied());
                }
            } else {
                let cov = cov_base.get(f.base_path());
                let unc: Vec<u32> = h
                    .base_lines
                    .iter()
                    .copied()
                    .filter(|l| !cov.is_some_and(|c| c.contains(l)))
                    .collect();
                if unc.is_empty() {
                    continue;
                }
                uncovered = true;
                bucket.removed += unc.len() as u32;
                bucket
                    .base_lines
                    .entry(f.base_path().to_string())
                    .or_default()
                    .extend(unc);
            }
        }
        if !uncovered {
            continue;
        }
        let b = buckets.entry(comp).or_default();
        b.added += bucket.added;
        b.removed += bucket.removed;
        b.files.insert(f.path.clone());
        for (k, v) in bucket.head_lines {
            b.head_lines.entry(k).or_default().extend(v);
        }
        for (k, v) in bucket.base_lines {
            b.base_lines.entry(k).or_default().extend(v);
        }
        if ctx.is_test_file(&f.path) {
            b.tests = true;
        } else if is_docs(&f.path) {
            b.docs = true;
        } else {
            b.code = true;
        }
    }

    // New config keys per component.
    let keys = |map: &onus_core::CodebaseMap| -> BTreeSet<(String, String)> {
        map.edges
            .iter()
            .filter(|e| e.kind == EdgeKind::ReadsConfig)
            .filter_map(|e| {
                Some((
                    onus_core::ids::component_of(&e.from)?.to_string(),
                    onus_core::ids::name_of(&e.to).to_string(),
                ))
            })
            .collect()
    };
    let base_keys = keys(ctx.base);
    let new_keys: BTreeSet<(String, String)> =
        keys(ctx.head).difference(&base_keys).cloned().collect();

    let mut rows = Vec::new();
    for (comp, b) in buckets {
        let total = b.added + b.removed;
        let kinds: Vec<String> = [(b.code, "code"), (b.tests, "tests"), (b.docs, "docs")]
            .iter()
            .filter(|(present, _)| *present)
            .map(|(_, name)| name.to_string())
            .collect();
        let what = if kinds.is_empty() {
            "code".to_string()
        } else {
            join_and(&kinds)
        };
        let test_data = comp == TEST_DATA;
        let place = if test_data {
            "in test data".to_string()
        } else if comp == "root" {
            "at the repository root".to_string()
        } else {
            format!("inside `{comp}`")
        };
        let title = if b.removed == 0 {
            format!("{} lines of new {what} {place}", thousands(total))
        } else if b.added == 0 {
            format!("{} removed lines of {what} {place}", thousands(total))
        } else {
            format!("{} changed lines of {what} {place}", thousands(total))
        };
        let users: Vec<String> = if comp == "root" || test_data {
            vec![]
        } else {
            let (_, comps) = ctx.dependents(&comp);
            comps
                .into_iter()
                .filter(|c| c != &comp)
                .map(|c| format!("`{c}`"))
                .collect()
        };
        let mut notes = vec![if test_data {
            "Files onus.yaml declares as test data; they are not part of the map".to_string()
        } else if comp == "root" {
            "Outside every component Onus found; declare them in onus.yaml to track them"
                .to_string()
        } else if users.is_empty() {
            format!("Stays inside `{comp}`; no other component depends on it")
        } else {
            format!(
                "Stays inside `{comp}`, which {} {} on",
                join_some(&users, 3),
                if users.len() == 1 {
                    "depends"
                } else {
                    "depend"
                }
            )
        }];
        if let Some(n) = tests.added_cases.get(&comp).filter(|n| **n > 0) {
            notes.push(format!("adds {}", plural(*n, "test case", "test cases")));
        }
        let comp_keys: Vec<String> = new_keys
            .iter()
            .filter(|(c, _)| c == &comp)
            .map(|(_, k)| format!("`{k}`"))
            .collect();
        if !comp_keys.is_empty() {
            notes.push(format!("reads new config {}", join_and(&comp_keys)));
        }
        let mut confidence = Confidence::Static;
        let diag_files: Vec<String> = b
            .files
            .iter()
            .filter(|f| !ctx.head_diagnostics(f).is_empty())
            .cloned()
            .collect();
        if !diag_files.is_empty() {
            confidence = Confidence::Low;
            notes.push(format!(
                "includes code Onus cannot fully resolve in {}",
                join_and(
                    &diag_files
                        .iter()
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                )
            ));
        }
        let mut locations = Vec::new();
        for (file, lines) in &b.head_lines {
            for (a, z) in ranges(lines) {
                locations.push(Location::head(file, a, z));
            }
        }
        for (file, lines) in &b.base_lines {
            for (a, z) in ranges(lines) {
                locations.push(Location::base(file, a, z));
            }
        }
        if locations.is_empty() {
            for f in &b.files {
                locations.push(Location::head(f, 1, 1));
            }
        }
        let mut row = change(
            ChangeKind::Internal,
            SUBKIND_INTERNAL_CHANGES,
            ChangeLevel::Structure,
            if test_data { "test-data" } else { &comp },
            (comp != "root" && !test_data).then_some(comp.as_str()),
            "Internal",
            title,
            notes.join("; "),
            locations,
        );
        row.stats = Some(Stats {
            lines_added: b.added,
            lines_removed: b.removed,
            files: b.files.len() as u32,
        });
        row.hints.confidence = confidence;
        if comp != "root" && !test_data {
            row.hints.blast_radius = ctx.dependents(&comp).0;
        }
        rows.push(row);
    }

    let mut moved_files: Vec<MovedFile> = moves
        .iter()
        .map(|(to, from)| MovedFile {
            from: from.clone(),
            to: to.clone(),
        })
        .collect();
    moved_files.sort();
    let formatting_only: Vec<String> = formatting
        .into_iter()
        .filter(|f| !moves.contains_key(f))
        .collect();
    (
        rows,
        StructureNotes {
            moved_files,
            formatting_only,
        },
    )
}
