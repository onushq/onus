//! Combining what providers return, and checking what plugins return.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::ids;
use onus_core::{Confidence, Edge, EdgeKind, MapDiagnostic, PartialMap, Site};

/// Merges `other` into `into`. Files, symbols and tests already present are
/// kept (the earlier provider wins, filling in a missing shape); edges are
/// united, and an edge confirmed by both keeps the more trustworthy
/// confidence, so a compiler-confirmed reference upgrades a syntactic one.
pub fn merge(into: &mut PartialMap, other: PartialMap) {
    let files: BTreeSet<String> = into.files.iter().map(|f| f.path.clone()).collect();
    into.files
        .extend(other.files.into_iter().filter(|f| !files.contains(&f.path)));

    let mut by_id: BTreeMap<String, usize> = into
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect();
    for s in other.symbols {
        match by_id.get(&s.id) {
            Some(&i) => {
                let existing = &mut into.symbols[i];
                if existing.shape.is_none() {
                    existing.shape = s.shape;
                }
                if existing.loc.is_none() {
                    existing.loc = s.loc;
                }
            }
            None => {
                by_id.insert(s.id.clone(), into.symbols.len());
                into.symbols.push(s);
            }
        }
    }

    let mut edges: BTreeMap<(String, String, EdgeKind), (Confidence, BTreeSet<Site>)> =
        BTreeMap::new();
    for e in into.edges.drain(..) {
        edges.insert(
            (e.from, e.to, e.kind),
            (e.confidence, e.sites.into_iter().collect()),
        );
    }
    for e in other.edges {
        let entry = edges
            .entry((e.from, e.to, e.kind))
            .or_insert((e.confidence, BTreeSet::new()));
        entry.0 = entry.0.min(e.confidence);
        entry.1.extend(e.sites);
    }
    into.edges = edges
        .into_iter()
        .map(|((from, to, kind), (confidence, sites))| Edge {
            from,
            to,
            kind,
            confidence,
            sites: sites.into_iter().collect(),
        })
        .collect();

    let tests: BTreeSet<String> = into.tests.iter().map(|t| t.id.clone()).collect();
    into.tests
        .extend(other.tests.into_iter().filter(|t| !tests.contains(&t.id)));
    into.diagnostics.extend(other.diagnostics);
    sort(into);
}

pub fn sort(m: &mut PartialMap) {
    m.files.sort_by(|a, b| a.path.cmp(&b.path));
    m.symbols.sort_by(|a, b| a.id.cmp(&b.id));
    m.symbols.dedup_by(|a, b| a.id == b.id);
    m.edges
        .sort_by(|a, b| (&a.from, &a.to, a.kind).cmp(&(&b.from, &b.to, b.kind)));
    for e in &mut m.edges {
        e.sites.sort();
        e.sites.dedup();
    }
    m.tests.sort_by(|a, b| a.id.cmp(&b.id));
    m.diagnostics.sort();
    m.diagnostics.dedup();
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Drops anything in a plugin's output that points outside the tree or at
/// unknown files or components, and notes it. Plugin output is untrusted
/// input.
pub fn validate(
    plugin: &str,
    mut m: PartialMap,
    files: &BTreeSet<String>,
    components: &BTreeSet<String>,
) -> PartialMap {
    let mut notes: Vec<String> = Vec::new();
    let file_ok = |p: &str| safe_relative(p) && files.contains(p);
    let id_ok = |id: &str| {
        if ids::is_global(id) {
            return true;
        }
        match ids::component_of(id) {
            Some(c) => components.contains(c) || c == "root",
            None => false,
        }
    };
    let before = m.files.len();
    m.files.retain(|f| file_ok(&f.path));
    if m.files.len() < before {
        notes.push(format!(
            "{} file entries outside the tree",
            before - m.files.len()
        ));
    }
    let before = m.symbols.len();
    m.symbols
        .retain(|s| id_ok(&s.id) && s.loc.as_ref().is_none_or(|l| file_ok(&l.file)));
    if m.symbols.len() < before {
        notes.push(format!(
            "{} symbols with unknown ids or files",
            before - m.symbols.len()
        ));
    }
    let before = m.edges.len();
    for e in &mut m.edges {
        e.sites.retain(|s| file_ok(&s.file));
    }
    m.edges.retain(|e| {
        id_ok(&e.from) && !e.sites.is_empty() && (ids::is_global(&e.to) || id_ok(&e.to))
    });
    if m.edges.len() < before {
        notes.push(format!(
            "{} edges with unknown ids or files",
            before - m.edges.len()
        ));
    }
    let before = m.tests.len();
    m.tests.retain(|t| file_ok(&t.file));
    if m.tests.len() < before {
        notes.push(format!("{} tests outside the tree", before - m.tests.len()));
    }
    m.diagnostics
        .retain(|d| file_ok(&d.file) || d.file.is_empty());
    for n in notes {
        m.diagnostics.push(MapDiagnostic {
            kind: "plugin-output-dropped".into(),
            file: String::new(),
            line: 0,
            message: format!("plugin `{plugin}`: dropped {n}"),
            confidence: Confidence::Low,
        });
    }
    sort(&mut m);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, c: Confidence, line: u32) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            kind: EdgeKind::Calls,
            confidence: c,
            sites: vec![Site {
                file: "a/x.ts".into(),
                line,
            }],
        }
    }

    #[test]
    fn a_confirmed_edge_takes_the_better_confidence() {
        let mut a = PartialMap {
            edges: vec![edge("a:x.ts#f", "a:y.ts#g", Confidence::Static, 3)],
            ..PartialMap::default()
        };
        let b = PartialMap {
            edges: vec![
                edge("a:x.ts#f", "a:y.ts#g", Confidence::Compiler, 3),
                edge("a:x.ts#f", "a:y.ts#h", Confidence::Compiler, 4),
            ],
            ..PartialMap::default()
        };
        merge(&mut a, b);
        let got: Vec<(&str, Confidence)> = a
            .edges
            .iter()
            .map(|e| (e.to.as_str(), e.confidence))
            .collect();
        assert_eq!(
            got,
            [
                ("a:y.ts#g", Confidence::Compiler),
                ("a:y.ts#h", Confidence::Compiler)
            ]
        );
    }

    #[test]
    fn plugin_output_cannot_escape_the_tree() {
        let files: BTreeSet<String> = ["a/x.ts".to_string()].into();
        let comps: BTreeSet<String> = ["a".to_string()].into();
        let mut bad = edge("a:x.ts#f", "a:y.ts#g", Confidence::Static, 1);
        bad.sites[0].file = "../etc/passwd".into();
        let m = PartialMap {
            edges: vec![
                bad,
                edge("a:x.ts#f", "npm:zod", Confidence::Static, 2),
                edge("evil:x#f", "a:y.ts#g", Confidence::Static, 2),
            ],
            ..PartialMap::default()
        };
        let m = validate("p", m, &files, &comps);
        assert_eq!(m.edges.len(), 1);
        assert_eq!(m.edges[0].to, "npm:zod");
        assert!(m.diagnostics[0].message.contains("dropped 2 edges"));
    }
}
