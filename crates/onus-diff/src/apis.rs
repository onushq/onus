//! Third-party APIs used for the first time.
//!
//! A change that calls `Effect.fromEither` when no code in the repository
//! has ever used it may be calling something the installed version of the
//! package does not have: a common mistake when code is written from memory
//! of a different version. Onus cannot see inside the package, but it can
//! see that the use is new, and say which version the repository pins.
//!
//! Only packages the repository already uses are considered (a new package
//! is a dependency change, reported elsewhere), and only production code:
//! a test that calls an API that does not exist fails when the tests run.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::{ChangeKind, ChangeLevel, Confidence, Location, SemanticChange};

use crate::ctx::{Ctx, change, join_and};

pub fn rows(ctx: &Ctx) -> Vec<SemanticChange> {
    let mut base_apis: BTreeSet<&str> = BTreeSet::new();
    let mut base_packages: BTreeSet<&str> = BTreeSet::new();
    for f in &ctx.base.files {
        for api in &f.external_apis {
            base_apis.insert(api.as_str());
            if let Some((pkg, _)) = api.split_once(' ') {
                base_packages.insert(pkg);
            }
        }
    }
    let changed: BTreeSet<&str> = ctx.text.files.iter().map(|f| f.path.as_str()).collect();
    // package → api → files using it.
    let mut first: BTreeMap<&str, BTreeMap<&str, BTreeSet<&str>>> = BTreeMap::new();
    for f in &ctx.head.files {
        if !changed.contains(f.path.as_str()) || f.is_test {
            continue;
        }
        for api in &f.external_apis {
            if base_apis.contains(api.as_str()) {
                continue;
            }
            let Some((pkg, name)) = api.split_once(' ') else {
                continue;
            };
            if !base_packages.contains(pkg) {
                continue;
            }
            first
                .entry(pkg)
                .or_default()
                .entry(name)
                .or_default()
                .insert(f.path.as_str());
        }
    }
    let mut rows = Vec::new();
    for (pkg, apis) in first {
        let names: Vec<String> = apis.keys().map(|a| format!("`{a}`")).collect();
        let mut locations = Vec::new();
        for (api, files) in &apis {
            for file in files {
                let line = use_line(ctx, file, api);
                locations.push(Location::head(*file, line, line));
            }
        }
        let version = pinned_version(ctx, pkg, apis.values().flatten().copied());
        let pronoun = if names.len() == 1 {
            "it exists"
        } else {
            "they exist"
        };
        let check = match &version {
            Some(v) => {
                format!("check that {pronoun} in `{pkg}` {v}, the version this repository pins")
            }
            None => format!("check that {pronoun} in the installed version of `{pkg}`"),
        };
        // Ranked with internal changes: a hint to verify, not a finding.
        let mut row = change(
            ChangeKind::Internal,
            "external-api-first-use",
            ChangeLevel::Relationship,
            &format!("npm:{pkg}"),
            None,
            "Third-party API, first use",
            format!(
                "First use of {} from `{pkg}`",
                if names.len() == 1 {
                    names[0].clone()
                } else {
                    join_and(&names)
                }
            ),
            format!(
                "No other code in this repository uses {}; {check}",
                if names.len() == 1 { "it" } else { "them" }
            ),
            locations,
        );
        row.id = format!("external-api-first-use:{pkg}");
        row.hints
            .novelty
            .extend(apis.keys().map(|a| format!("new-api:{pkg} {a}")));
        row.hints.confidence = Confidence::Static;
        rows.push(row);
    }
    rows
}

/// The first added line of `file` that mentions the API's last part
/// (`fromEither` for `Effect.fromEither`), or 1.
fn use_line(ctx: &Ctx, file: &str, api: &str) -> u32 {
    let needle = api.rsplit('.').next().unwrap_or(api);
    ctx.text
        .files
        .iter()
        .find(|f| f.path == file)
        .and_then(|f| {
            f.added_lines
                .iter()
                .find(|(_, text)| mentions(text, needle))
                .map(|(l, _)| *l)
        })
        .unwrap_or(1)
}

/// Whether `text` contains `word` as a whole identifier.
fn mentions(text: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// The version of `pkg` declared by the component of the files using it,
/// else by any component; `None` when nothing declares it.
fn pinned_version<'a>(
    ctx: &Ctx,
    pkg: &str,
    files: impl Iterator<Item = &'a str>,
) -> Option<String> {
    let comps: BTreeSet<String> = files.map(|f| ctx.component_of_path(f)).collect();
    let deps: Vec<&onus_core::PackageDep> =
        ctx.head.packages.iter().filter(|p| p.name == pkg).collect();
    deps.iter()
        .find(|d| comps.contains(&d.component_id))
        .or_else(|| deps.first())
        .map(|d| d.version.clone())
        .filter(|v| !v.is_empty())
}
