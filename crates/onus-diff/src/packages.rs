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
