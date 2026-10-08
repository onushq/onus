//! Step 1 and 2 of the semantic diff: match symbols by id, then pair the
//! leftovers by body fingerprint to find renames and moves.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onus_core::ids;
use onus_core::{
    ChangeKind, ChangeLevel, EdgeKind, Location, SemanticChange, SymbolKind, SymbolNode, Visibility,
};

use crate::ctx::{Ctx, change, join_some, plural};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairKind {
    Same,
    Rename,
    Move,
    RenameAndMove,
    /// Same name and kind in another file, with an edited definition whose
    /// shape mostly overlaps: moved and changed in one go.
    MoveWithChanges,
}

#[derive(Debug, Default)]
pub struct Pairs<'a> {
    /// Base and head versions of the same symbol (same id, renamed or moved).
    pub pairs: Vec<(&'a SymbolNode, &'a SymbolNode, PairKind)>,
    pub removed: Vec<&'a SymbolNode>,
    pub added: Vec<&'a SymbolNode>,
    /// Base id → head id for renamed or moved symbols.
    pub renamed: BTreeMap<String, String>,
}

fn is_code(s: &SymbolNode) -> bool {
    s.component_id.is_some()
}

fn file(s: &SymbolNode) -> &str {
    s.loc.as_ref().map(|l| l.file.as_str()).unwrap_or("")
}

pub fn match_symbols<'a>(ctx: &Ctx<'a>) -> Pairs<'a> {
    let mut out = Pairs::default();
    let mut removed: Vec<&SymbolNode> = Vec::new();
    for s in ctx.base.symbols.iter().filter(|s| is_code(s)) {
        match ctx.head_syms.get(s.id.as_str()) {
            Some(h) => out.pairs.push((s, h, PairKind::Same)),
            None => removed.push(s),
        }
    }
    let mut added: Vec<&SymbolNode> = ctx
        .head
        .symbols
        .iter()
        .filter(|s| is_code(s) && !ctx.base_syms.contains_key(s.id.as_str()))
        .collect();

    // Head path → base path, to recognize moved files.
    let moves = ctx.moves.borrow().clone();
    let mut taken: BTreeSet<&str> = BTreeSet::new();
    let mut class_map: HashMap<String, String> = HashMap::new();
    let mut paired_removed: BTreeSet<&str> = BTreeSet::new();
    // Top-level symbols first, then methods (which follow their class).
    for pass in [false, true] {
        for r in &removed {
            let is_method = r.kind == SymbolKind::Method;
            if is_method != pass || r.body_fingerprint.is_none() {
                continue;
            }
            let candidates: Vec<&&SymbolNode> = added
                .iter()
                .filter(|a| {
                    !taken.contains(a.id.as_str())
                        && a.kind == r.kind
                        && a.body_fingerprint == r.body_fingerprint
                        && (a.component_id == r.component_id
                            || moves.get(file(a)).map(String::as_str) == Some(file(r))
                            // The same definition under the same name in
                            // another component: moved between components.
                            || a.name == r.name)
                        // Private declarations with the same small body in
                        // other files (`let open = $state(false)` in two
                        // pages) are a coincidence, not a rename or a move.
                        && (a.visibility == Visibility::Public
                            || r.visibility == Visibility::Public
                            || file(a) == file(r)
                            || moves.get(file(a)).map(String::as_str) == Some(file(r))
                            || (a.name == r.name && a.component_id == r.component_id))
                })
                .collect();
            let best = if is_method {
                let (class, method) = r.name.split_once('.').unwrap_or(("", &r.name));
                let base_class = format!("{}#{}", ids::module_of(&r.id), class);
                let Some(head_class) = class_map.get(&base_class) else {
                    continue;
                };
                let want = format!("{head_class}.{method}");
                candidates.into_iter().find(|a| a.id == want)
            } else {
                candidates
                    .iter()
                    .find(|a| a.name == r.name)
                    .or_else(|| candidates.iter().find(|a| file(a) == file(r)))
                    .or_else(|| candidates.first())
                    .copied()
            };
            let Some(a) = best else {
                continue;
            };
            taken.insert(a.id.as_str());
            paired_removed.insert(r.id.as_str());
            let kind = match (a.name == r.name, file(a) == file(r)) {
                (true, _) => PairKind::Move,
                (false, true) => PairKind::Rename,
                (false, false) => PairKind::RenameAndMove,
            };
            if r.kind == SymbolKind::Class {
                class_map.insert(r.id.clone(), a.id.clone());
            }
            out.renamed.insert(r.id.clone(), a.id.clone());
            out.pairs.push((r, a, kind));
        }
    }
    // Moved and edited at once: same name and kind in another file, with
    // mostly the same members or parameters. Exact fingerprints would miss
    // it, and it would read as one removal plus one addition.
    let remaining: Vec<&SymbolNode> = removed
        .iter()
        .filter(|r| !paired_removed.contains(r.id.as_str()) && r.kind != SymbolKind::Method)
        .copied()
        .collect();
    for r in remaining {
        let candidates: Vec<&&SymbolNode> = added
            .iter()
            .filter(|a| {
                !taken.contains(a.id.as_str())
                    && a.kind == r.kind
                    && a.name == r.name
                    && file(a) != file(r)
                    // Two private declarations that share a name in different
                    // components (a page's local `tabs`) are a coincidence.
                    && (a.component_id == r.component_id
                        || (a.visibility == Visibility::Public
                            && r.visibility == Visibility::Public))
                    && shapes_overlap(r, a)
            })
            .collect();
        // Only an unambiguous match counts.
        if let [a] = candidates.as_slice() {
            taken.insert(a.id.as_str());
            paired_removed.insert(r.id.as_str());
            out.renamed.insert(r.id.clone(), a.id.clone());
            out.pairs.push((r, a, PairKind::MoveWithChanges));
        }
    }
    removed.retain(|r| !paired_removed.contains(r.id.as_str()));
    added.retain(|a| !taken.contains(a.id.as_str()));
    out.removed = removed;
    out.added = added;
    out
}

/// Names of members and parameters, the parts of a shape that identify it.
fn shape_names(s: &SymbolNode) -> BTreeSet<&str> {
    s.shape
        .as_ref()
        .map(|sh| {
            sh.members
                .iter()
                .map(|m| m.name.as_str())
                .chain(sh.params.iter().map(|p| p.name.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// At least half of the member and parameter names in common.
fn shapes_overlap(a: &SymbolNode, b: &SymbolNode) -> bool {
    let (x, y) = (shape_names(a), shape_names(b));
    if x.is_empty() && y.is_empty() {
        return true;
    }
    let common = x.intersection(&y).count();
    let all = x.union(&y).count();
    common * 2 >= all
}

/// One row per renamed symbol (moves alone are structure, not meaning).
pub fn rename_rows(ctx: &Ctx, pairs: &Pairs) -> Vec<SemanticChange> {
    let mut rows = Vec::new();
    for (b, h, kind) in &pairs.pairs {
        if !matches!(kind, PairKind::Rename | PairKind::RenameAndMove)
            || h.kind == SymbolKind::Method
        {
            continue;
        }
        let mut locations = Vec::new();
        if let Some(l) = &h.loc {
            locations.push(Location::head(
                &l.file,
                l.start,
                l.signature_end.unwrap_or(l.start),
            ));
        }
        let mut call_sites = 0u32;
        for e in &ctx.head.edges {
            if e.to != h.id {
                continue;
            }
            for s in &e.sites {
                locations.push(Location::head(&s.file, s.line, s.line));
            }
            if e.kind == EdgeKind::Calls {
                call_sites += e.sites.len() as u32;
            }
        }
        let call_files: BTreeSet<&str> = ctx
            .head
            .edges
            .iter()
            .filter(|e| e.to == h.id && e.kind == EdgeKind::Calls)
            .flat_map(|e| e.sites.iter().map(|s| s.file.as_str()))
            .collect();
        let stale: Vec<&onus_core::MapDiagnostic> = ctx
            .head
            .diagnostics
            .iter()
            .filter(|d| {
                d.kind == "unresolved-import" && d.message.contains(&format!("`{}`", b.name))
            })
            .collect();
        for d in &stale {
            locations.push(Location::head(&d.file, d.line, d.line));
        }
        let component = h.component_id.as_deref();
        let mut title = format!("`{}` renamed to `{}`", b.name, h.name);
        if *kind == PairKind::RenameAndMove {
            title.push_str(&format!(" and moved to `{}`", file(h)));
        }
        let mut why = format!(
            "No behavior change (identical body); {} in {} updated",
            plural(call_sites, "call site", "call sites"),
            plural(call_files.len() as u32, "file", "files"),
        );
        if h.visibility == Visibility::Public {
            why.push_str(&format!(
                "; exported name of `{}` changed and every caller in this repository was updated",
                component.unwrap_or("")
            ));
        }
        let (kind_out, subkind) = if stale.is_empty() {
            (ChangeKind::Internal, "rename")
        } else {
            why = format!(
                "{} still use the old name `{}` and no longer resolve",
                plural(stale.len() as u32, "reference", "references"),
                b.name
            );
            (ChangeKind::Breaking, "rename-incomplete")
        };
        let mut row = change(
            kind_out,
            subkind,
            ChangeLevel::Structure,
            &h.id,
            component,
            "Rename",
            title,
            why,
            locations,
        );
        row.hints.blast_radius = ctx.dependents(&h.id).0;
        rows.push(row);
    }
    rows
}

/// One row per pair of components that symbols moved between: a refactor
/// that moves 17 types into a new library reads as one change, not as 17
/// removals and 17 additions.
pub fn move_rows(ctx: &Ctx, pairs: &Pairs) -> Vec<SemanticChange> {
    let mut groups: BTreeMap<(String, String), Vec<(&SymbolNode, &SymbolNode)>> = BTreeMap::new();
    for (b, h, kind) in &pairs.pairs {
        if !matches!(kind, PairKind::Move | PairKind::MoveWithChanges)
            || h.kind == SymbolKind::Method
        {
            continue;
        }
        let (Some(from), Some(to)) = (&b.component_id, &h.component_id) else {
            continue;
        };
        if from != to {
            groups
                .entry((from.clone(), to.clone()))
                .or_default()
                .push((b, h));
        }
    }
    let mut rows = Vec::new();
    for ((from, to), moved) in groups {
        let mut names: Vec<String> = moved.iter().map(|(_, h)| format!("`{}`", h.name)).collect();
        names.sort();
        let mut locations = Vec::new();
        let mut dependents = BTreeSet::new();
        for (b, h) in &moved {
            if let Some(l) = &h.loc {
                locations.push(Location::head(&l.file, l.start, l.end));
            }
            if let Some(l) = &b.loc {
                locations.push(Location::base(&l.file, l.start, l.end));
            }
            for e in ctx.head.edges.iter().filter(|e| e.to == h.id) {
                for s in &e.sites {
                    dependents.insert(s.file.clone());
                    locations.push(Location::head(&s.file, s.line, s.line));
                }
            }
        }
        let stale: Vec<&onus_core::MapDiagnostic> = ctx
            .head
            .diagnostics
            .iter()
            .filter(|d| {
                d.kind == "unresolved-import"
                    && moved
                        .iter()
                        .any(|(b, _)| d.message.contains(&format!("`{}`", b.name)))
            })
            .collect();
        for d in &stale {
            locations.push(Location::head(&d.file, d.line, d.line));
        }

        // Contract changes made while moving, folded into this row.
        let mut changed: Vec<String> = Vec::new();
        let mut breaking = false;
        for (b, h) in &moved {
            if b.body_fingerprint == h.body_fingerprint {
                continue;
            }
            let deltas = match (&b.shape, &h.shape) {
                (Some(bs), Some(hs)) => crate::contracts::shape_changes(h.kind, bs, hs),
                _ => vec![],
            };
            breaking |= deltas.iter().any(|(br, _)| *br);
            let detail = deltas
                .first()
                .map(|(_, p)| {
                    let p = if p.chars().count() > 60 {
                        format!("{}…", p.chars().take(60).collect::<String>())
                    } else {
                        p.clone()
                    };
                    format!(" ({p}{})", if deltas.len() > 1 { ", …" } else { "" })
                })
                .unwrap_or_default();
            changed.push(format!("`{}`{detail}", h.name));
        }
        changed.sort();
        let mut why = if changed.is_empty() {
            format!("Identical definitions ({})", join_some(&names, 4))
        } else if changed.len() == moved.len() {
            format!(
                "All of them changed while moving: {}",
                join_some(&changed, 3)
            )
        } else {
            format!(
                "{} identical, {} changed while moving: {}",
                moved.len() - changed.len(),
                changed.len(),
                join_some(&changed, 3)
            )
        };
        let (kind, subkind) = if stale.is_empty() {
            why.push_str(&format!(
                "; every reference in this repository now points at `{to}`"
            ));
            if breaking {
                (ChangeKind::Breaking, "moved-between-components")
            } else {
                (ChangeKind::Internal, "moved-between-components")
            }
        } else {
            why.push_str(&format!(
                "; {} still use the old place and no longer resolve",
                plural(stale.len() as u32, "reference", "references")
            ));
            (ChangeKind::Breaking, "moved-incomplete")
        };
        let was_public = moved
            .iter()
            .any(|(b, _)| b.visibility == Visibility::Public);
        let still_public = moved
            .iter()
            .all(|(_, h)| h.visibility == Visibility::Public);
        if was_public {
            why.push_str(&format!("; `{from}` no longer exports them"));
            if !still_public {
                why.push_str(&format!(" and `{to}` does not export all of them"));
            }
        }
        let title = if moved.len() == 1 {
            format!("`{}` moved from `{from}` to `{to}`", moved[0].1.name)
        } else {
            format!("{} symbols moved from `{from}` to `{to}`", moved.len())
        };
        let mut row = change(
            kind,
            subkind,
            ChangeLevel::Structure,
            &to,
            Some(&to),
            "Moved",
            title,
            why,
            locations,
        );
        row.id = format!("{subkind}:{from}->{to}");
        row.hints.blast_radius = dependents.len() as u32;
        rows.push(row);
    }
    rows
}
