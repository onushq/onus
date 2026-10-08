//! Step 5: third-party packages. A new package is supply-chain novelty. When
//! the package is the SDK of an external service reported in the same
//! component, it is folded into that row: one meaning, one row.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::ids;
use onus_core::{ChangeKind, ChangeLevel, EdgeKind, Location, PackageDep, SemanticChange};

use crate::ctx::{Ctx, change};

fn component_label(c: &str) -> String {
    if c == "root" {
        "The repository root".to_string()
    } else {
        format!("`{c}`")
    }
}

pub fn rows(ctx: &Ctx, rows: &mut Vec<SemanticChange>) {
    let key = |p: &PackageDep| (p.component_id.clone(), p.name.clone());
    let base: BTreeMap<(String, String), &PackageDep> =
        ctx.base.packages.iter().map(|p| (key(p), p)).collect();
    let head: BTreeMap<(String, String), &PackageDep> =
        ctx.head.packages.iter().map(|p| (key(p), p)).collect();
    let base_names: BTreeSet<&str> = ctx.base.packages.iter().map(|p| p.name.as_str()).collect();
    let mut new_rows = Vec::new();
    let mut touched_manifests = BTreeSet::new();

    for ((comp, name), dep) in &head {
        let import_sites = import_sites(ctx, comp, name);
        match base.get(&(comp.clone(), name.clone())) {
            None => {
                touched_manifests.insert(dep.file.clone());
                let mut locations = vec![Location::head(&dep.file, dep.line, dep.line)];
                locations.extend(import_sites);
                // Fold into a new external-service row for the same SDK.
                if let Some(ext) = rows.iter_mut().find(|r| {
                    r.component.as_deref() == Some(comp.as_str())
                        && r.subkind.starts_with("new-external")
                        && ctx
                            .head
                            .externals
                            .iter()
                            .any(|e| e.id == r.subject && e.packages.iter().any(|p| p == name))
                }) {
                    ext.hints.novelty.push(format!("new-dependency:{name}"));
                    ext.locations.extend(locations);
                    ext.locations.sort();
                    ext.locations.dedup();
                    continue;
                }
                let repo_new = !base_names.contains(name.as_str());
                let why = if repo_new {
                    format!(
                        "New third-party code in the supply chain; `{name}` is not used anywhere else in this repository"
                    )
                } else {
                    let users: BTreeSet<String> = ctx
                        .base
                        .packages
                        .iter()
                        .filter(|p| p.name == *name)
                        .map(|p| format!("`{}`", p.component_id))
                        .collect();
                    format!(
                        "Already used by {}; now also a dependency of {}",
                        users.into_iter().collect::<Vec<_>>().join(", "),
                        component_label(comp)
                    )
                };
                let mut row = change(
                    ChangeKind::Dependency,
                    "new-dependency",
                    ChangeLevel::Relationship,
                    &ids::npm_id(name),
                    Some(comp),
                    "New dependency",
                    format!(
                        "{} adds the npm package `{name}` ({})",
                        component_label(comp),
                        dep.version
                    ),
                    why,
                    locations,
                );
                row.id = format!("new-dependency:npm:{name}@{comp}");
                if repo_new {
                    row.hints.novelty.push(format!("new-package:{name}"));
                }
                new_rows.push(row);
            }
            Some(old) if old.version != dep.version => {
                touched_manifests.insert(dep.file.clone());
                let mut row = change(
                    ChangeKind::Dependency,
                    "dependency-version-changed",
                    ChangeLevel::Relationship,
                    &ids::npm_id(name),
                    Some(comp),
                    "Dependency version",
                    format!(
                        "{} moves `{name}` from {} to {}",
                        component_label(comp),
                        old.version,
                        dep.version
                    ),
                    "Different third-party code runs after this change".into(),
                    vec![Location::head(&dep.file, dep.line, dep.line)],
                );
                row.id = format!("dependency-version-changed:npm:{name}@{comp}");
                new_rows.push(row);
            }
            Some(_) => {}
        }
    }
    for ((comp, name), dep) in &base {
        if head.contains_key(&(comp.clone(), name.clone())) {
            continue;
        }
        touched_manifests.insert(dep.file.clone());
        let mut row = change(
            ChangeKind::Dependency,
            "dependency-removed",
            ChangeLevel::Relationship,
            &ids::npm_id(name),
            Some(comp),
            "Dependency removed",
            format!("{} drops the npm package `{name}`", component_label(comp)),
            "Less third-party code in the supply chain".into(),
            vec![Location::base(&dep.file, dep.line, dep.line)],
        );
        row.id = format!("dependency-removed:npm:{name}@{comp}");
        new_rows.push(row);
    }
    let new_rows = merge_new_packages(new_rows);
    let any_dependency_change = !touched_manifests.is_empty();
    // Packages already in the repository (a migration moving dependencies
    // into each package) and bulk removals or upgrades read as one row per
    // component. A package new to the repository keeps its own row.
    let new_rows = crate::ctx::group_rows(
        new_rows,
        |r| r.hints.novelty.is_empty(),
        |r, n, place| match r.subkind.as_str() {
            "new-dependency" => {
                format!("{n} npm packages already used in this repository added {place}")
            }
            "dependency-removed" => format!("{n} npm packages dropped {place}"),
            _ => format!("{n} npm package versions changed {place}"),
        },
        |r| {
            let name = onus_core::ids::name_of(&r.subject);
            format!("`{name}`")
        },
        |r, items| match r.subkind.as_str() {
            "new-dependency" => format!("{items}; no new third-party code enters the repository"),
            "dependency-removed" => format!("{items}; less third-party code in the supply chain"),
            _ => format!("{items}; different third-party code runs after this change"),
        },
    );
    rows.extend(new_rows);
    // Lockfiles only record what the manifests already say.
    if any_dependency_change {
        for f in &ctx.text.files {
            let name = f.path.rsplit('/').next().unwrap_or(&f.path);
            if LOCKFILES.contains(&name) {
                ctx.explain(&f.path);
            }
        }
    }
}

/// Packages new to the repository read once however many components add
/// them, and several added to the same components read as one row.
fn merge_new_packages(rows: Vec<SemanticChange>) -> Vec<SemanticChange> {
    let is_new = |r: &SemanticChange| {
        r.subkind == "new-dependency"
            && r.hints
                .novelty
                .iter()
                .any(|n| n.starts_with("new-package:"))
    };
    let (new, mut out): (Vec<SemanticChange>, Vec<SemanticChange>) =
        rows.into_iter().partition(is_new);
    // Package → its rows, one per component.
    let mut by_package: BTreeMap<String, Vec<SemanticChange>> = BTreeMap::new();
    for r in new {
        by_package.entry(r.subject.clone()).or_default().push(r);
    }
    // Components → packages added to exactly those components.
    let mut by_components: BTreeMap<Vec<String>, Vec<Vec<SemanticChange>>> = BTreeMap::new();
    for (_, rows) in by_package {
        let mut comps: Vec<String> = rows
            .iter()
            .map(|r| r.component.clone().unwrap_or_else(|| "root".into()))
            .collect();
        comps.sort();
        comps.dedup();
        by_components.entry(comps).or_default().push(rows);
    }
    for (comps, packages) in by_components {
        if comps.len() == 1 && packages.len() == 1 {
            out.extend(packages.into_iter().flatten());
            continue;
        }
        let place =
            crate::ctx::join_and(&comps.iter().map(|c| component_label(c)).collect::<Vec<_>>());
        let names: Vec<String> = packages
            .iter()
            .map(|rows| {
                let name = onus_core::ids::name_of(&rows[0].subject);
                let version = rows[0]
                    .title
                    .rsplit_once('(')
                    .map(|(_, v)| v.trim_end_matches(')').to_string())
                    .unwrap_or_default();
                format!("`{name}` ({version})")
            })
            .collect();
        let verb = if comps.len() == 1 { "adds" } else { "add" };
        let mut row = packages[0][0].clone();
        row.component = (comps.len() == 1).then(|| comps[0].clone());
        if packages.len() == 1 {
            row.title = format!("{place} {verb} the npm package {}", names[0]);
            row.why_it_matters = format!(
                "New third-party code in the supply chain; {} is not used anywhere else in this repository",
                names[0].split(' ').next().unwrap_or("")
            );
            row.id = format!("new-dependency:{}", row.subject);
        } else {
            row.subject = "npm-packages".into();
            row.title = format!(
                "{place} {verb} {} npm packages new to the repository",
                packages.len()
            );
            row.why_it_matters = format!(
                "New third-party code in the supply chain: {}",
                crate::ctx::join_some(&names, 5)
            );
            row.id = format!("new-dependency:{}", comps.join("+"));
        }
        let all = packages.iter().flatten();
        row.locations = all.clone().flat_map(|r| r.locations.clone()).collect();
        row.locations.sort();
        row.locations.dedup();
        row.hints.novelty = all.flat_map(|r| r.hints.novelty.clone()).collect();
        row.hints.novelty.sort();
        row.hints.novelty.dedup();
        out.push(row);
    }
    out
}

pub const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "bun.lock",
];

fn import_sites(ctx: &Ctx, comp: &str, name: &str) -> Vec<Location> {
    let target = ids::npm_id(name);
    ctx.head
        .edges
        .iter()
        .filter(|e| {
            e.kind == EdgeKind::Imports
                && e.to == target
                && ids::component_of(&e.from) == Some(comp)
        })
        .flat_map(|e| {
            e.sites
                .iter()
                .map(|s| Location::head(&s.file, s.line, s.line))
        })
        .collect()
}
