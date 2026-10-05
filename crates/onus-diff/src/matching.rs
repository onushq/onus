//! Step 1 and 2 of the semantic diff: match symbols by id, then pair the
//! leftovers by body fingerprint to find renames and moves.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onus_core::ids;
use onus_core::{
    ChangeKind, ChangeLevel, EdgeKind, Location, SemanticChange, SymbolKind, SymbolNode, Visibility,
};

use crate::ctx::{Ctx, change, plural};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairKind {
    Same,
    Rename,
    Move,
    RenameAndMove,
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
                            || moves.get(file(a)).map(String::as_str) == Some(file(r)))
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
    removed.retain(|r| !paired_removed.contains(r.id.as_str()));
    added.retain(|a| !taken.contains(a.id.as_str()));
    out.removed = removed;
    out.added = added;
    out
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
