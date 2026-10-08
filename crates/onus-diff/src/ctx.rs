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
    /// Framework packs listed in onus.yaml.
    pub packs: BTreeSet<String>,
    /// The `testData` globs of onus.yaml.
    test_data: globset::GlobSet,
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
            test_data: onus_map::build::test_data_globs(config),
            packs: config
                .map(|c| {
                    c.extractors
                        .packs
                        .iter()
                        .map(|p| p.trim_start_matches("./").to_string())
                        .collect()
                })
                .unwrap_or_default(),
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

    /// Whether onus.yaml declares the file test data.
    pub fn is_test_data(&self, path: &str) -> bool {
        self.test_data.is_match(path)
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
        self.dependents_in(self.head, subject)
    }

    /// The same, in the base map: for things the change removed.
    pub fn base_dependents(&self, subject: &str) -> (u32, BTreeSet<String>) {
        self.dependents_in(self.base, subject)
    }

    fn dependents_in(&self, map: &CodebaseMap, subject: &str) -> (u32, BTreeSet<String>) {
        let is_component = self.components.contains_key(subject) && !subject.contains(':');
        let own_file = map
            .symbols
            .iter()
            .find(|s| s.id == subject)
            .and_then(|s| s.loc.as_ref())
            .map(|l| l.file.clone());
        let class_prefix = format!("{subject}.");
        let mut files = BTreeSet::new();
        let mut comps = BTreeSet::new();
        for e in &map.edges {
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

    /// Files in the head map that use `symbol` (or a member of it), with the
    /// first line of use in each, sorted by path. The symbol's own file is
    /// left out.
    pub fn dependent_sites(&self, symbol: &str) -> Vec<(String, u32)> {
        sites_in(self.head, symbol)
    }

    /// The same, in the base map: for things the change removed.
    pub fn base_dependent_sites(&self, symbol: &str) -> Vec<(String, u32)> {
        sites_in(self.base, symbol)
    }

    /// Whether the component's package is published: its `package.json` is
    /// not private and says what to publish (`files` or `publishConfig`).
    pub fn is_published(&self, component: &str) -> bool {
        let Some(c) = self.components.get(component) else {
            return false;
        };
        let dir = c
            .roots
            .first()
            .map(|r| r.trim_end_matches("**").trim_end_matches('/'))
            .unwrap_or("");
        let path = if dir.is_empty() {
            "package.json".to_string()
        } else {
            format!("{dir}/package.json")
        };
        let Ok(text) = std::fs::read_to_string(onus_core::paths::native(self.head_root, &path))
        else {
            return false;
        };
        let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        pkg.get("private") != Some(&serde_json::Value::Bool(true))
            && (pkg.get("files").is_some() || pkg.get("publishConfig").is_some())
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

fn sites_in(map: &CodebaseMap, symbol: &str) -> Vec<(String, u32)> {
    let own_file = map
        .symbols
        .iter()
        .find(|s| s.id == symbol)
        .and_then(|s| s.loc.as_ref())
        .map(|l| l.file.as_str());
    let class_prefix = format!("{symbol}.");
    let mut first: BTreeMap<&str, u32> = BTreeMap::new();
    for e in &map.edges {
        if e.to != symbol && !e.to.starts_with(&class_prefix) {
            continue;
        }
        for s in &e.sites {
            if Some(s.file.as_str()) == own_file {
                continue;
            }
            let line = first.entry(s.file.as_str()).or_insert(s.line);
            *line = (*line).min(s.line);
        }
    }
    first.into_iter().map(|(f, l)| (f.to_string(), l)).collect()
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

/// The kind of edit a row's title names (`` `module` `` in "Build config
/// `a/tsconfig.json` changes `module`"), or "" when it names none.
pub fn edit_pattern(title: &str) -> String {
    let at = [" changes ", " change "]
        .iter()
        .find_map(|w| title.find(w).map(|i| i + w.len()));
    let Some(at) = at else {
        return String::new();
    };
    let mut rest = &title[at..];
    for tail in [" in `", " at the repository root"] {
        if let Some(i) = rest.rfind(tail) {
            rest = &rest[..i];
        }
    }
    rest.to_string()
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

/// `a, b, c and 27 more`: lists stay readable in one table cell.
pub fn join_some(items: &[String], max: usize) -> String {
    if items.len() <= max {
        return join_and(items);
    }
    format!("{} and {} more", items[..max].join(", "), items.len() - max)
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

/// Merges rows that share a component and subkind into one row when there
/// are at least two: `title` gets the count and the component, `items` the
/// per-row detail shown in the "why".
pub fn group_rows(
    rows: Vec<SemanticChange>,
    groupable: impl Fn(&SemanticChange) -> bool,
    title: impl Fn(&SemanticChange, usize, &str) -> String,
    item: impl Fn(&SemanticChange) -> String,
    why: impl Fn(&SemanticChange, &str) -> String,
) -> Vec<SemanticChange> {
    group_rows_by(rows, groupable, |_| String::new(), title, item, why)
}

/// [`group_rows`], keeping rows apart that differ in `key` (the kind of
/// edit, for config files).
pub fn group_rows_by(
    rows: Vec<SemanticChange>,
    groupable: impl Fn(&SemanticChange) -> bool,
    key: impl Fn(&SemanticChange) -> String,
    title: impl Fn(&SemanticChange, usize, &str) -> String,
    item: impl Fn(&SemanticChange) -> String,
    why: impl Fn(&SemanticChange, &str) -> String,
) -> Vec<SemanticChange> {
    let mut out = Vec::new();
    type Key = (Option<String>, String, String);
    let mut groups: BTreeMap<Key, Vec<SemanticChange>> = BTreeMap::new();
    for r in rows {
        if groupable(&r) {
            groups
                .entry((r.component.clone(), r.subkind.clone(), key(&r)))
                .or_default()
                .push(r);
        } else {
            out.push(r);
        }
    }
    let mut ids: BTreeMap<String, u32> = BTreeMap::new();
    for ((component, subkind, _), mut members) in groups {
        if members.len() == 1 {
            out.extend(members);
            continue;
        }
        members.sort_by(|a, b| a.id.cmp(&b.id));
        let place = match component.as_deref() {
            Some(c) if c != "root" => format!("in `{c}`"),
            _ => "at the repository root".to_string(),
        };
        let items: Vec<String> = members.iter().map(&item).collect();
        let first = &members[0];
        let mut row = first.clone();
        row.title = title(first, members.len(), &place);
        row.why_it_matters = why(first, &join_some(&items, 4));
        row.subject = component.clone().unwrap_or_else(|| "root".into());
        let n = ids.entry(format!("{subkind}:{}", row.subject)).or_default();
        *n += 1;
        row.id = if *n == 1 {
            format!("{subkind}:{}", row.subject)
        } else {
            format!("{subkind}:{}#{n}", row.subject)
        };
        row.locations = members.iter().flat_map(|m| m.locations.clone()).collect();
        row.locations.sort();
        row.locations.dedup();
        row.hints.novelty = members
            .iter()
            .flat_map(|m| m.hints.novelty.clone())
            .collect();
        row.hints.novelty.sort();
        row.hints.novelty.dedup();
        row.hints.rules_of_the_game = members.iter().any(|m| m.hints.rules_of_the_game);
        row.hints.needs_person = members.iter().any(|m| m.hints.needs_person);
        out.push(row);
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
