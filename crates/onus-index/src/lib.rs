//! An in-memory index over a [`CodebaseMap`], answering the questions
//! agents and people ask about a codebase (PLAN.md, Phase 2): where is
//! something, what depends on it, what does it depend on, which tests
//! exercise it, who owns it.
//!
//! Every answer is computed from the map, sorted, and bounded by a limit,
//! so the same map and question always give the same, reasonably sized
//! answer. Each item carries its evidence (file and line).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use onus_core::{CodebaseMap, Edge, EdgeKind, SymbolNode, ids};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The default number of items in an answer.
pub const DEFAULT_LIMIT: usize = 50;
/// The most items an answer may ask for.
pub const MAX_LIMIT: usize = 500;
/// The deepest a dependency walk may go.
pub const MAX_DEPTH: u32 = 8;

/// A map with lookup tables for queries.
#[derive(Debug)]
pub struct MapIndex {
    map: CodebaseMap,
    /// Node id → node number. Nodes are every symbol, module, external,
    /// event, table, config key and npm package an edge mentions.
    node: HashMap<String, u32>,
    names: Vec<String>,
    /// Per node: edges leaving it and edges arriving at it.
    out: Vec<Vec<u32>>,
    inc: Vec<Vec<u32>>,
    /// Symbol id → index in `map.symbols`.
    symbol: HashMap<String, usize>,
    /// Lowercase name tokens per symbol, for `find`.
    tokens: Vec<Vec<String>>,
    /// Per symbol: its fields and parameters, with their name tokens.
    fields: Vec<Vec<(String, Vec<String>)>>,
    /// Repository path → lowercase words of the path.
    path_tokens: HashMap<String, Vec<String>>,
    /// Module id → repository path.
    module_file: HashMap<String, String>,
    /// Repository path → module id.
    file_module: HashMap<String, String>,
    /// Repository path → index in `map.files`.
    file: HashMap<String, usize>,
    /// Repository path → symbols declared there.
    file_symbols: HashMap<String, Vec<usize>>,
    /// Symbol or module id → tests that exercise it.
    tested_by: HashMap<String, Vec<usize>>,
}

impl MapIndex {
    pub fn new(map: CodebaseMap) -> MapIndex {
        let mut node: HashMap<String, u32> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        let mut intern = |id: &str, node: &mut HashMap<String, u32>| -> u32 {
            if let Some(&n) = node.get(id) {
                return n;
            }
            let n = names.len() as u32;
            names.push(id.to_string());
            node.insert(id.to_string(), n);
            n
        };
        let mut ends = Vec::with_capacity(map.edges.len());
        for e in &map.edges {
            let a = intern(&e.from, &mut node);
            let b = intern(&e.to, &mut node);
            ends.push((a, b));
        }
        for s in &map.symbols {
            intern(&s.id, &mut node);
        }
        let mut out = vec![Vec::new(); names.len()];
        let mut inc = vec![Vec::new(); names.len()];
        for (i, (a, b)) in ends.into_iter().enumerate() {
            out[a as usize].push(i as u32);
            inc[b as usize].push(i as u32);
        }

        let symbol: HashMap<String, usize> = map
            .symbols
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i))
            .collect();
        let tokens = map.symbols.iter().map(|s| name_tokens(&s.name)).collect();
        let fields = map
            .symbols
            .iter()
            .map(|s| {
                let Some(shape) = &s.shape else {
                    return Vec::new();
                };
                shape
                    .members
                    .iter()
                    .map(|m| m.name.clone())
                    .chain(shape.params.iter().map(|p| p.name.clone()))
                    .map(|n| {
                        let t = name_tokens(&n);
                        (n, t)
                    })
                    .collect()
            })
            .collect();
        let path_tokens = map
            .files
            .iter()
            .map(|f| (f.path.clone(), name_tokens(&f.path)))
            .collect();

        let mut module_file: HashMap<String, String> = HashMap::new();
        let mut file_symbols: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, s) in map.symbols.iter().enumerate() {
            if let Some(loc) = &s.loc {
                module_file
                    .entry(ids::module_of(&s.id).to_string())
                    .or_insert_with(|| loc.file.clone());
                file_symbols.entry(loc.file.clone()).or_default().push(i);
            }
        }
        for t in &map.tests {
            module_file
                .entry(t.id.clone())
                .or_insert_with(|| t.file.clone());
        }
        for e in &map.edges {
            if !e.from.contains('#')
                && !ids::is_global(&e.from)
                && let Some(site) = e.sites.first()
            {
                module_file
                    .entry(e.from.clone())
                    .or_insert_with(|| site.file.clone());
            }
        }
        let file_module = module_file
            .iter()
            .map(|(m, f)| (f.clone(), m.clone()))
            .collect();
        let file = map
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.clone(), i))
            .collect();

        let mut tested_by: HashMap<String, Vec<usize>> = HashMap::new();
        for (ti, t) in map.tests.iter().enumerate() {
            for target in &t.exercises {
                tested_by.entry(target.clone()).or_default().push(ti);
                tested_by
                    .entry(ids::module_of(target).to_string())
                    .or_default()
                    .push(ti);
            }
        }
        for list in tested_by.values_mut() {
            list.sort_unstable();
            list.dedup();
        }

        MapIndex {
            map,
            node,
            names,
            out,
            inc,
            symbol,
            tokens,
            fields,
            path_tokens,
            module_file,
            file_module,
            file,
            file_symbols,
            tested_by,
        }
    }

    pub fn map(&self) -> &CodebaseMap {
        &self.map
    }

    /// What the index holds.
    pub fn status(&self) -> Status {
        Status {
            commit: self.map.commit.clone(),
            components: self.map.components.len(),
            files: self.map.files.len(),
            symbols: self.map.symbols.len(),
            edges: self.map.edges.len(),
            tests: self.map.tests.len(),
            diagnostics: self.map.diagnostics.len(),
        }
    }

    /// Resolves what someone means by `target`: a symbol, module or
    /// component id, a repository path, or a bare symbol name.
    pub fn resolve(&self, target: &str) -> Vec<Target> {
        let target = target.trim();
        if self.symbol.contains_key(target) {
            return vec![Target::Symbol(target.to_string())];
        }
        if self.module_file.contains_key(target) {
            return vec![Target::Module(target.to_string())];
        }
        if self.map.components.iter().any(|c| c.id == target) {
            return vec![Target::Component(target.to_string())];
        }
        let path = target.trim_start_matches("./");
        if let Some(m) = self.file_module.get(path) {
            return vec![Target::Module(m.clone())];
        }
        if self.file.contains_key(path) {
            return vec![Target::File(path.to_string())];
        }
        if self.node.contains_key(target) {
            return vec![Target::Node(target.to_string())];
        }
        // A bare name: every symbol with that name or qualified name.
        let mut found: Vec<Target> = self
            .map
            .symbols
            .iter()
            .filter(|s| s.name == target || ids::name_of(&s.id) == target)
            .map(|s| Target::Symbol(s.id.clone()))
            .collect();
        found.sort();
        found
    }

    /// Symbols that match the words of `query`, best first: by how many
    /// words match and where (the symbol's name, then its fields and
    /// parameters, then its file's path). Also the files whose paths match,
    /// so an answer always gives somewhere to start.
    pub fn find(&self, query: &str, limit: usize) -> Found {
        let limit = clamp(limit);
        let q = query.trim().to_lowercase();
        let mut words: Vec<String> = name_tokens(query)
            .into_iter()
            .filter(|w| !STOP_WORDS.contains(&w.as_str()))
            .collect();
        words.dedup();
        if q.is_empty() || words.is_empty() {
            return Found::default();
        }
        let joined = q.replace(' ', "");
        let mut hits: Vec<(u32, usize, Vec<String>, &SymbolNode)> = Vec::new();
        for (i, s) in self.map.symbols.iter().enumerate() {
            let exact = s.name.to_lowercase() == joined;
            let mut score = if exact { 1000 } else { 0 };
            let mut matched = 0;
            let mut why: Vec<String> = Vec::new();
            let mut in_name_or_field = exact;
            let mut path_words = 0;
            // A field or parameter named exactly like the query.
            if let Some((field, _)) = self.fields[i]
                .iter()
                .find(|(name, _)| name.to_lowercase() == joined)
            {
                score += 200;
                in_name_or_field = true;
                why.push(format!("field `{field}`"));
            }
            for w in &words {
                let name_hit = self.tokens[i].iter().find(|t| word_matches(w, t));
                let field_hit = self.fields[i]
                    .iter()
                    .find(|(_, tokens)| tokens.iter().any(|t| word_matches(w, t)));
                let path_hit = s
                    .loc
                    .as_ref()
                    .and_then(|l| self.path_tokens.get(&l.file))
                    .is_some_and(|tokens| tokens.iter().any(|t| word_matches(w, t)));
                let best = if let Some(t) = name_hit {
                    in_name_or_field = true;
                    if t == w { 35 } else { 30 }
                } else if let Some((field, _)) = field_hit {
                    in_name_or_field = true;
                    why.push(format!("field `{field}`"));
                    18
                } else if path_hit {
                    path_words += 1;
                    12
                } else {
                    0
                };
                if best > 0 {
                    matched += 1;
                }
                score += best;
            }
            // Symbols of a file whose path has every word count too: the
            // `publisherProfile.ts` of "publisher profile".
            let whole_path = words.len() > 1 && path_words == words.len();
            if !in_name_or_field && !whole_path {
                continue;
            }
            score += 15 * matched as u32;
            if s.visibility == onus_core::Visibility::Public {
                score += 5;
            }
            why.sort();
            why.dedup();
            hits.push((score, matched, why, s));
        }
        // Symbols matching more of the words first, then by where they match.
        hits.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.0.cmp(&a.0))
                .then_with(|| a.3.id.cmp(&b.3.id))
        });
        let total = hits.len();

        let mut files: Vec<(usize, &str)> = self
            .map
            .files
            .iter()
            .filter_map(|f| {
                let tokens = self.path_tokens.get(&f.path)?;
                let n = words
                    .iter()
                    .filter(|w| tokens.iter().any(|t| word_matches(w, t)))
                    .count();
                (n > 0).then_some((n, f.path.as_str()))
            })
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        let all_words = files.first().map_or(0, |f| f.0);
        let files: Vec<String> = files
            .into_iter()
            .take_while(|f| f.0 == all_words)
            .take(10)
            .map(|(_, p)| p.to_string())
            .collect();

        let hint = if total == 0 && files.is_empty() {
            Some(
                "Nothing matched by name, field or path. Try one distinctive word (a type, \
                 function or field name) or a different spelling."
                    .to_string(),
            )
        } else if total == 0 {
            Some(
                "No symbol or field matched; these files match by path. Try `onus_file` on one \
                 of them, or fewer words."
                    .to_string(),
            )
        } else {
            None
        };
        Found {
            query: query.to_string(),
            total,
            truncated: total > limit,
            symbols: hits
                .into_iter()
                .take(limit)
                .map(|(_, matched, why, s)| FoundSymbol {
                    symbol: symbol_ref(s),
                    matched_words: matched,
                    matched_fields: why,
                })
                .collect(),
            files,
            hint,
        }
    }

    /// Everything known about one symbol.
    pub fn symbol(&self, id: &str) -> Option<SymbolInfo> {
        let s = &self.map.symbols[*self.symbol.get(id)?];
        let n = self.node.get(id).map(|&n| n as usize);
        let count =
            |list: Option<&Vec<Vec<u32>>>| n.and_then(|n| list.map(|l| l[n].len())).unwrap_or(0);
        Some(SymbolInfo {
            symbol: symbol_ref(s),
            shape: s.shape.clone(),
            invariants: s.invariants.clone(),
            uses: count(Some(&self.out)),
            used_by: count(Some(&self.inc)),
            tests: self.tested_by.get(id).map_or(0, Vec::len),
        })
    }

    /// What depends on `target`, following edges backwards up to `depth`
    /// steps (code dependencies only: imports, calls, type references).
    pub fn dependents(&self, target: &str, depth: u32, limit: usize) -> Result<Walk, String> {
        self.walk(target, depth, limit, Direction::Backward)
    }

    /// What `target` depends on, following edges forwards up to `depth`
    /// steps: code, external services, events, data and config.
    pub fn dependencies(&self, target: &str, depth: u32, limit: usize) -> Result<Walk, String> {
        self.walk(target, depth, limit, Direction::Forward)
    }

    fn walk(&self, target: &str, depth: u32, limit: usize, dir: Direction) -> Result<Walk, String> {
        let limit = clamp(limit);
        let depth = depth.clamp(1, MAX_DEPTH);
        let starts = self.start_nodes(target)?;
        let mut seen: BTreeSet<u32> = starts.iter().copied().collect();
        let mut queue: VecDeque<(u32, u32)> = starts.iter().map(|&n| (n, 0)).collect();
        let mut links: Vec<Link> = Vec::new();
        while let Some((n, d)) = queue.pop_front() {
            if d >= depth {
                continue;
            }
            let edges = match dir {
                Direction::Backward => &self.inc[n as usize],
                Direction::Forward => &self.out[n as usize],
            };
            for &ei in edges {
                let e = &self.map.edges[ei as usize];
                if dir == Direction::Backward && !e.kind.is_code_dependency() {
                    continue;
                }
                let (next, via) = match dir {
                    Direction::Backward => (&e.from, &e.to),
                    Direction::Forward => (&e.to, &e.from),
                };
                let Some(&m) = self.node.get(next) else {
                    continue;
                };
                if !seen.insert(m) {
                    continue;
                }
                links.push(self.link(next, via, e, d + 1));
                // Externals, events, tables and packages end a walk.
                if !ids::is_global(next) {
                    queue.push_back((m, d + 1));
                }
            }
        }
        links.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.id.cmp(&b.id)));
        let total = links.len();
        links.truncate(limit);
        let mut components: BTreeMap<String, usize> = BTreeMap::new();
        for l in &links {
            if let Some(c) = &l.component {
                *components.entry(c.clone()).or_default() += 1;
            }
        }
        Ok(Walk {
            target: target.to_string(),
            resolved: starts
                .iter()
                .map(|&n| self.names[n as usize].clone())
                .collect(),
            depth,
            total,
            truncated: total > limit,
            components,
            links,
        })
    }

    /// The nodes a walk starts from: the symbol or module itself; for a
    /// file, its module and symbols; for a component, its public symbols.
    fn start_nodes(&self, target: &str) -> Result<Vec<u32>, String> {
        let resolved = self.resolve(target);
        if resolved.is_empty() {
            return Err(no_match(target));
        }
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for t in &resolved {
            match t {
                Target::Symbol(id) | Target::Node(id) => {
                    ids.insert(id.clone());
                }
                Target::Module(m) => {
                    ids.insert(m.clone());
                    if let Some(f) = self.module_file.get(m) {
                        for &si in self.file_symbols.get(f).into_iter().flatten() {
                            ids.insert(self.map.symbols[si].id.clone());
                        }
                    }
                }
                Target::File(path) => {
                    for &si in self.file_symbols.get(path).into_iter().flatten() {
                        ids.insert(self.map.symbols[si].id.clone());
                    }
                }
                Target::Component(c) => {
                    for s in &self.map.symbols {
                        if s.component_id.as_deref() == Some(c)
                            && s.visibility == onus_core::Visibility::Public
                        {
                            ids.insert(s.id.clone());
                        }
                    }
                }
            }
        }
        Ok(ids
            .iter()
            .filter_map(|id| self.node.get(id).copied())
            .collect())
    }

    fn link(&self, id: &str, via: &str, e: &Edge, depth: u32) -> Link {
        let site = e.sites.first();
        Link {
            id: id.to_string(),
            name: ids::display_name(id).to_string(),
            component: ids::component_of(id).map(str::to_string),
            kind: e.kind.as_str().to_string(),
            via: via.to_string(),
            depth,
            file: site.map(|s| s.file.clone()),
            line: site.map(|s| s.line),
            confidence: format!("{:?}", e.confidence).to_lowercase(),
        }
    }

    /// The tests that exercise `target` (a symbol, a file or a module).
    pub fn tests_for(&self, target: &str, limit: usize) -> Result<Tests, String> {
        let limit = clamp(limit);
        let resolved = self.resolve(target);
        if resolved.is_empty() {
            return Err(no_match(target));
        }
        let mut keys: BTreeSet<String> = BTreeSet::new();
        for t in &resolved {
            match t {
                Target::Symbol(id) | Target::Node(id) => {
                    keys.insert(id.clone());
                }
                Target::Module(m) => {
                    keys.insert(m.clone());
                }
                Target::File(p) => {
                    if let Some(m) = self.file_module.get(p) {
                        keys.insert(m.clone());
                    }
                }
                Target::Component(c) => {
                    keys.extend(
                        self.map
                            .symbols
                            .iter()
                            .filter(|s| s.component_id.as_deref() == Some(c))
                            .map(|s| s.id.clone()),
                    );
                }
            }
        }
        let mut tests: BTreeSet<usize> = BTreeSet::new();
        for k in &keys {
            tests.extend(self.tested_by.get(k).into_iter().flatten().copied());
        }
        let total = tests.len();
        let list: Vec<TestRef> = tests
            .into_iter()
            .take(limit)
            .map(|ti| {
                let t = &self.map.tests[ti];
                TestRef {
                    file: t.file.clone(),
                    component: t.component_id.clone(),
                    cases: t.cases.len(),
                    skipped: t
                        .cases
                        .iter()
                        .filter(|c| c.markers.iter().any(|m| m == "skip" || m == "todo"))
                        .count(),
                }
            })
            .collect();
        Ok(Tests {
            target: target.to_string(),
            total,
            truncated: total > limit,
            tests: list,
        })
    }

    /// Who owns `target` (a component, file or symbol), from onus.yaml and
    /// CODEOWNERS.
    pub fn owners(&self, target: &str) -> Result<Owners, String> {
        let component = self.component_of(target).ok_or_else(|| {
            format!("`{target}` is not in a component (it belongs to the repository root)")
        })?;
        let c = self
            .map
            .components
            .iter()
            .find(|c| c.id == component)
            .ok_or_else(|| format!("no component `{component}`"))?;
        Ok(Owners {
            target: target.to_string(),
            component: c.id.clone(),
            owners: c.owners.clone(),
            labels: c.labels.clone(),
        })
    }

    fn component_of(&self, target: &str) -> Option<String> {
        for t in self.resolve(target) {
            let found = match t {
                Target::Component(c) => Some(c),
                Target::Symbol(id) | Target::Module(id) | Target::Node(id) => {
                    ids::component_of(&id).map(str::to_string)
                }
                Target::File(p) => self
                    .file
                    .get(&p)
                    .and_then(|&i| self.map.files[i].component_id.clone()),
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// A component: what it is, what it exposes, what it uses and what
    /// uses it.
    pub fn component(&self, id: &str) -> Option<ComponentInfo> {
        let c = self.map.components.iter().find(|c| c.id == id)?;
        let mut uses: BTreeMap<String, usize> = BTreeMap::new();
        let mut used_by: BTreeMap<String, usize> = BTreeMap::new();
        let mut externals: BTreeSet<String> = BTreeSet::new();
        let mut events_published: BTreeSet<String> = BTreeSet::new();
        let mut events_consumed: BTreeSet<String> = BTreeSet::new();
        for e in &self.map.edges {
            let from = ids::component_of(&e.from);
            let to = ids::component_of(&e.to);
            if from == Some(id) {
                match e.kind {
                    EdgeKind::CallsExternal => {
                        externals.insert(e.to.clone());
                    }
                    EdgeKind::Publishes => {
                        events_published.insert(ids::name_of(&e.to).to_string());
                    }
                    EdgeKind::Consumes => {
                        events_consumed.insert(ids::name_of(&e.to).to_string());
                    }
                    k if k.is_code_dependency() => {
                        if let Some(t) = to.filter(|t| *t != id) {
                            *uses.entry(t.to_string()).or_default() += 1;
                        }
                    }
                    _ => {}
                }
            } else if to == Some(id)
                && e.kind.is_code_dependency()
                && let Some(f) = from
            {
                *used_by.entry(f.to_string()).or_default() += 1;
            }
        }
        let public: Vec<SymbolRef> = self
            .map
            .symbols
            .iter()
            .filter(|s| {
                s.component_id.as_deref() == Some(id)
                    && s.visibility == onus_core::Visibility::Public
            })
            .map(symbol_ref)
            .collect();
        let public_total = public.len();
        Some(ComponentInfo {
            id: c.id.clone(),
            kind: format!("{:?}", c.kind).to_lowercase(),
            roots: c.roots.clone(),
            owners: c.owners.clone(),
            labels: c.labels.clone(),
            package_name: c.package_name.clone(),
            files: self
                .map
                .files
                .iter()
                .filter(|f| f.component_id.as_deref() == Some(id))
                .count(),
            public_symbols: public_total,
            public: public.into_iter().take(DEFAULT_LIMIT).collect(),
            uses,
            used_by,
            externals: externals.into_iter().collect(),
            events_published: events_published.into_iter().collect(),
            events_consumed: events_consumed.into_iter().collect(),
        })
    }

    /// A file: its component, symbols, imports and importers.
    pub fn file(&self, path: &str) -> Option<FileInfo> {
        let path = path.trim_start_matches("./");
        let f = &self.map.files[*self.file.get(path)?];
        let module = self.file_module.get(path).cloned();
        // Imports point at the module or at the symbols they bind.
        let mut targets: Vec<u32> = module
            .iter()
            .filter_map(|m| self.node.get(m).copied())
            .collect();
        targets.extend(
            self.file_symbols
                .get(path)
                .into_iter()
                .flatten()
                .filter_map(|&i| self.node.get(&self.map.symbols[i].id).copied()),
        );
        let imported_by: Vec<String> = targets
            .iter()
            .flat_map(|&n| &self.inc[n as usize])
            .map(|&ei| &self.map.edges[ei as usize])
            .filter(|e| e.kind == EdgeKind::Imports)
            .filter_map(|e| e.sites.first().map(|s| s.file.clone()))
            .filter(|f| f != path)
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect();
        let mut symbols: Vec<SymbolRef> = self
            .file_symbols
            .get(path)
            .into_iter()
            .flatten()
            .map(|&i| symbol_ref(&self.map.symbols[i]))
            .collect();
        symbols.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.id.cmp(&b.id)));
        Some(FileInfo {
            path: f.path.clone(),
            component: f.component_id.clone(),
            module,
            is_test: f.is_test,
            lines: f.lines,
            symbols,
            imports: f.imports.clone(),
            imported_by,
            diagnostics: self
                .map
                .diagnostics
                .iter()
                .filter(|d| d.file == f.path)
                .map(|d| format!("{}:{} {}: {}", d.file, d.line, d.kind, d.message))
                .collect(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Backward,
}

fn no_match(target: &str) -> String {
    format!(
        "nothing in the map matches `{target}`: use a symbol id, an exact symbol name, a file \
         path or a component id (`find` searches names by words)"
    )
}

fn clamp(limit: usize) -> usize {
    if limit == 0 {
        DEFAULT_LIMIT
    } else {
        limit.min(MAX_LIMIT)
    }
}

/// Whether a query word matches an identifier word: as a prefix
/// (`pub` matches `publisher`), or as the plural of it (`publishers`).
fn word_matches(word: &str, token: &str) -> bool {
    if word.len() < 3 {
        return token == word;
    }
    token.starts_with(word)
        || (word.len() > 3 && word.ends_with('s') && token == &word[..word.len() - 1])
}

/// Words that carry no meaning in a search for code.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "of", "on",
    "or", "the", "to", "with",
];

/// Lowercase words of an identifier: `formatPhoneNumber` → format, phone,
/// number; `MAX_RETRIES` → max, retries.
pub fn name_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if !ch.is_alphanumeric() {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
        }
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
        cur.extend(ch.to_lowercase());
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn symbol_ref(s: &SymbolNode) -> SymbolRef {
    SymbolRef {
        id: s.id.clone(),
        name: s.name.clone(),
        kind: format!("{:?}", s.kind).to_lowercase(),
        component: s.component_id.clone(),
        public: s.visibility == onus_core::Visibility::Public,
        file: s.loc.as_ref().map(|l| l.file.clone()),
        line: s.loc.as_ref().map(|l| l.start),
    }
}

/// What a query's `target` turned out to be.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum Target {
    Symbol(String),
    Module(String),
    Component(String),
    File(String),
    /// An external service, event, table, config key or npm package.
    Node(String),
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub commit: String,
    pub components: usize,
    pub files: usize,
    pub symbols: usize,
    pub edges: usize,
    pub tests: usize,
    pub diagnostics: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SymbolRef {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub component: Option<String>,
    pub public: bool,
    pub file: Option<String>,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub query: String,
    pub total: usize,
    pub truncated: bool,
    pub symbols: Vec<FoundSymbol>,
    /// Files whose paths match the most words (at most 10).
    pub files: Vec<String>,
    /// What to try when nothing, or only paths, matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FoundSymbol {
    #[serde(flatten)]
    pub symbol: SymbolRef,
    /// How many of the query's words matched.
    pub matched_words: usize,
    /// Fields or parameters that matched, when the name did not.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matched_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInfo {
    pub symbol: SymbolRef,
    pub shape: Option<onus_core::ContractShape>,
    pub invariants: Vec<String>,
    /// Edges leaving the symbol.
    pub uses: usize,
    /// Edges arriving at the symbol.
    pub used_by: usize,
    /// Test files that exercise it.
    pub tests: usize,
}

/// One step of a dependency walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub id: String,
    pub name: String,
    pub component: Option<String>,
    /// The edge kind: imports, calls, references-type, calls-external, …
    pub kind: String,
    /// The node on the other end of the edge, closer to the target.
    pub via: String,
    pub depth: u32,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Walk {
    pub target: String,
    /// The ids the walk started from.
    pub resolved: Vec<String>,
    pub depth: u32,
    pub total: usize,
    pub truncated: bool,
    /// Links per component, among those returned.
    pub components: BTreeMap<String, usize>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestRef {
    pub file: String,
    pub component: Option<String>,
    pub cases: usize,
    /// Cases marked skip or todo.
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tests {
    pub target: String,
    pub total: usize,
    pub truncated: bool,
    pub tests: Vec<TestRef>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Owners {
    pub target: String,
    pub component: String,
    pub owners: Vec<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComponentInfo {
    pub id: String,
    pub kind: String,
    pub roots: Vec<String>,
    pub owners: Vec<String>,
    pub labels: Vec<String>,
    pub package_name: Option<String>,
    pub files: usize,
    pub public_symbols: usize,
    /// The first public symbols, by id.
    pub public: Vec<SymbolRef>,
    /// Components this one depends on, with the number of edges.
    pub uses: BTreeMap<String, usize>,
    /// Components that depend on this one, with the number of edges.
    pub used_by: BTreeMap<String, usize>,
    pub externals: Vec<String>,
    pub events_published: Vec<String>,
    pub events_consumed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub path: String,
    pub component: Option<String>,
    pub module: Option<String>,
    pub is_test: bool,
    pub lines: u32,
    pub symbols: Vec<SymbolRef>,
    /// Module ids this file imports.
    pub imports: Vec<String>,
    /// Files that import this one.
    pub imported_by: Vec<String>,
    pub diagnostics: Vec<String>,
}

/// The questions the index answers, as one type for the CLI and MCP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "query", rename_all = "kebab-case")]
pub enum Query {
    /// What the map holds.
    Status,
    /// Symbols whose names match a few words, best first.
    Find {
        text: String,
        #[serde(default)]
        limit: usize,
    },
    /// One symbol: kind, shape, invariants, counts.
    Symbol { id: String },
    /// What depends on a symbol, file, module or component.
    Dependents {
        target: String,
        #[serde(default = "one")]
        depth: u32,
        #[serde(default)]
        limit: usize,
    },
    /// What a symbol, file, module or component depends on.
    Dependencies {
        target: String,
        #[serde(default = "one")]
        depth: u32,
        #[serde(default)]
        limit: usize,
    },
    /// The tests that exercise a symbol, file, module or component.
    TestsFor {
        target: String,
        #[serde(default)]
        limit: usize,
    },
    /// Who owns a component, file or symbol.
    Owners { target: String },
    /// A component: owners, labels, public surface, what it uses and what uses it.
    Component { id: String },
    /// A file: component, symbols, imports and importers.
    File { path: String },
}

fn one() -> u32 {
    1
}

impl MapIndex {
    /// Answers `q` as JSON.
    pub fn answer(&self, q: &Query) -> Result<serde_json::Value, String> {
        let json = |v: Result<serde_json::Value, serde_json::Error>| v.map_err(|e| e.to_string());
        match q {
            Query::Status => json(serde_json::to_value(self.status())),
            Query::Find { text, limit } => json(serde_json::to_value(self.find(text, *limit))),
            Query::Symbol { id } => match self.symbol(id) {
                Some(s) => json(serde_json::to_value(s)),
                None => {
                    let candidates = self.resolve(id);
                    if candidates.is_empty() {
                        Err(format!("no symbol `{id}`; try `find`"))
                    } else {
                        json(serde_json::to_value(
                            serde_json::json!({ "candidates": candidates }),
                        ))
                    }
                }
            },
            Query::Dependents {
                target,
                depth,
                limit,
            } => json(serde_json::to_value(
                self.dependents(target, *depth, *limit)?,
            )),
            Query::Dependencies {
                target,
                depth,
                limit,
            } => json(serde_json::to_value(
                self.dependencies(target, *depth, *limit)?,
            )),
            Query::TestsFor { target, limit } => {
                json(serde_json::to_value(self.tests_for(target, *limit)?))
            }
            Query::Owners { target } => json(serde_json::to_value(self.owners(target)?)),
            Query::Component { id } => match self.component(id) {
                Some(c) => json(serde_json::to_value(c)),
                None => Err(format!("no component `{id}`")),
            },
            Query::File { path } => match self.file(path) {
                Some(f) => json(serde_json::to_value(f)),
                None => Err(format!("no file `{path}` in the map")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_split_into_words() {
        assert_eq!(
            name_tokens("formatPhoneNumber"),
            ["format", "phone", "number"]
        );
        assert_eq!(name_tokens("MAX_RETRIES"), ["max", "retries"]);
        assert_eq!(
            name_tokens("UserPreferences.phone"),
            ["user", "preferences", "phone"]
        );
        assert_eq!(name_tokens("toE164"), ["to", "e164"]);
    }
}
