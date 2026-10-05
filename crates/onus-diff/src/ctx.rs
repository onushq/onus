//! Shared state and helpers for the diff passes.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use onus_core::ids;
use onus_core::{
    ChangeKind, ChangeLevel, CodebaseMap, Component, Confidence, Hints, Location, OnusConfig,
    SemanticChange, Sensitivity, SourceFile, SymbolNode,
};
use onus_map::discover::ComponentMatcher;

use crate::text::TreeDiff;

/// Labels treated as high sensitivity when `onus.yaml` does not say.
pub const DEFAULT_SENSITIVE_LABELS: &[&str] = &["auth", "payments", "pii"];

pub struct Ctx<'a> {
    pub base: &'a CodebaseMap,
    pub head: &'a CodebaseMap,
    pub base_root: &'a Path,
    pub head_root: &'a Path,
    pub text: &'a TreeDiff,
    pub base_syms: HashMap<&'a str, &'a SymbolNode>,
    pub head_syms: HashMap<&'a str, &'a SymbolNode>,
    pub base_files: HashMap<&'a str, &'a SourceFile>,
    pub head_files: HashMap<&'a str, &'a SourceFile>,
    pub components: BTreeMap<String, &'a Component>,
    sensitive: BTreeSet<String>,
    matcher: ComponentMatcher,
    /// Head path → base path for files that moved (identical bytes or
    /// identical tokens).
    pub moves: RefCell<BTreeMap<String, String>>,
    /// Files whose changes are fully explained without a line-level row.
    pub explained: RefCell<BTreeSet<String>>,
}

impl std::fmt::Debug for Ctx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx").finish_non_exhaustive()
    }
}

impl<'a> Ctx<'a> {
    pub fn new(
        base: &'a CodebaseMap,
        head: &'a CodebaseMap,
        base_root: &'a Path,
        head_root: &'a Path,
        text: &'a TreeDiff,
        config: Option<&OnusConfig>,
    ) -> Self {
        let mut components = BTreeMap::new();
        for c in &base.components {
            components.insert(c.id.clone(), c);
        }
        for c in &head.components {
            components.insert(c.id.clone(), c);
        }
        let mut sensitive: BTreeSet<String> = DEFAULT_SENSITIVE_LABELS
            .iter()
            .map(|s| s.to_string())
            .collect();
        if let Some(cfg) = config {
            for (label, l) in &cfg.labels {
                if l.sensitivity >= Sensitivity::Medium {
                    sensitive.insert(label.clone());
                } else {
                    sensitive.remove(label);
                }
            }
        }
        let all: Vec<Component> = components.values().map(|c| (*c).clone()).collect();
        Ctx {
            base,
            head,
            base_root,
            head_root,
            text,
            base_syms: base.symbols.iter().map(|s| (s.id.as_str(), s)).collect(),
            head_syms: head.symbols.iter().map(|s| (s.id.as_str(), s)).collect(),
            base_files: base.files.iter().map(|f| (f.path.as_str(), f)).collect(),
            head_files: head.files.iter().map(|f| (f.path.as_str(), f)).collect(),
            components,
            sensitive,
            matcher: ComponentMatcher::new(&all),
            moves: RefCell::new(text.renames()),
            explained: RefCell::new(BTreeSet::new()),
        }
    }

    /// Sensitivity labels of a component, sorted.
    pub fn labels(&self, component: &str) -> Vec<String> {
        self.components
            .get(component)
            .map(|c| {
                c.labels
                    .iter()
                    .filter(|l| self.sensitive.contains(*l))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn is_sensitive(&self, component: &str) -> bool {
        !self.labels(component).is_empty()
    }

    /// The component a file belongs to, in either tree.
    pub fn component_of_path(&self, path: &str) -> String {
        if let Some(c) = self
            .head_files
            .get(path)
            .or_else(|| self.base_files.get(path))
            .and_then(|f| f.component_id.clone())
        {
            return c;
        }
        self.matcher
            .component_of(path)
            .unwrap_or_else(|| onus_lang_ts_root().to_string())
    }

    pub fn is_test_file(&self, path: &str) -> bool {
        self.head_files
            .get(path)
            .or_else(|| self.base_files.get(path))
            .is_some_and(|f| f.is_test)
    }

    /// Files that changed in the given component.
    pub fn component_changed(&self, component: &str) -> bool {
        self.text
            .files
            .iter()
            .any(|f| self.component_of_path(&f.path) == component)
    }

    /// Files in the head map (tests included) that depend on `subject`, and
    /// their components. For a component id, files in other components that
    /// depend on anything inside it.
    pub fn dependents(&self, subject: &str) -> (u32, BTreeSet<String>) {
        let is_component = self.components.contains_key(subject) && !subject.contains(':');
        let own_file = self
            .head_syms
            .get(subject)
            .and_then(|s| s.loc.as_ref())
            .map(|l| l.file.clone());
        let class_prefix = format!("{subject}.");
        let mut files = BTreeSet::new();
        let mut comps = BTreeSet::new();
        for e in &self.head.edges {
            let hit = if is_component {
                ids::component_of(&e.to) == Some(subject)
                    && ids::component_of(&e.from) != Some(subject)
            } else {
                e.to == subject || e.to.starts_with(&class_prefix)
            };
            if !hit {
                continue;
            }
            for s in &e.sites {
                if Some(&s.file) == own_file.as_ref() {
                    continue;
                }
                files.insert(s.file.clone());
                comps.insert(self.component_of_path(&s.file));
            }
        }
        (files.len() as u32, comps)
    }

    /// Head diagnostics in a file.
    pub fn head_diagnostics(&self, file: &str) -> Vec<&'a onus_core::MapDiagnostic> {
        self.head
            .diagnostics
            .iter()
            .filter(|d| d.file == file)
            .collect()
    }

    pub fn explain(&self, file: &str) {
        self.explained.borrow_mut().insert(file.to_string());
    }
}

fn onus_lang_ts_root() -> &'static str {
    "root"
}

/// A change with default hints.
#[allow(clippy::too_many_arguments)]
pub fn change(
    kind: ChangeKind,
    subkind: &str,
    level: ChangeLevel,
    subject: &str,
    component: Option<&str>,
    kind_label: &str,
    title: String,
    why: String,
    locations: Vec<Location>,
) -> SemanticChange {
    let mut locations = locations;
    locations.sort();
    locations.dedup();
    SemanticChange {
        id: format!("{subkind}:{subject}"),
        kind,
        subkind: subkind.to_string(),
        level,
        subject: subject.to_string(),
        component: component.map(str::to_string),
        kind_label: kind_label.to_string(),
        title,
        why_it_matters: why,
        hints: Hints {
            labels: vec![],
            blast_radius: 0,
            novelty: vec![],
            confidence: Confidence::Static,
            rules_of_the_game: false,
            intent_mismatch: false,
            needs_person: false,
        },
        locations,
        stats: None,
    }
}

/// `1,404`
pub fn thousands(n: u32) -> String {
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

pub fn plural(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", thousands(n))
    }
}

/// `a`, `a and b`, `a, b and c`
pub fn join_and(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => format!("{} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

/// Collapses sorted line numbers into inclusive ranges.
pub fn ranges(lines: &BTreeSet<u32>) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &l in lines {
        match out.last_mut() {
            Some((_, end)) if *end + 1 == l => *end = l,
            _ => out.push((l, l)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_and_lists() {
        assert_eq!(thousands(1404), "1,404");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000_000), "1,000,000");
        assert_eq!(
            join_and(&["a".into(), "b".into(), "c".into()]),
            "a, b and c"
        );
        let set: BTreeSet<u32> = [1, 2, 3, 7, 9, 10].into_iter().collect();
        assert_eq!(ranges(&set), [(1, 3), (7, 7), (9, 10)]);
    }
}
