//! The TypeScript and JavaScript adapter for Onus.
//!
//! Files are parsed with tree-sitter (TypeScript and TSX grammars) in
//! parallel. Each file yields syntax-level facts ([`extract`]); the linker
//! below then resolves imports, re-exports and references across files
//! ([`resolve`]) and produces map symbols, edges, tests and diagnostics.
//! Nothing from the analyzed tree is installed or executed.

pub mod extract;
pub mod jsonc;
pub mod lang;
pub mod norm;
pub mod packs;
pub mod resolve;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onus_core::ids::{self, module_id, symbol_id};
use onus_core::{
    Confidence, Edge, EdgeKind, LanguageAdapter, Loc, MapDiagnostic, PartialMap, Site, SourceFile,
    SymbolKind, SymbolNode, TestNode, Visibility, Workspace,
};
use rayon::prelude::*;

use crate::extract::{Binding, EventExpr, Export, FileFacts, Imported, Patterns, RefKind};
use crate::resolve::{Resolution, Resolver};

/// Stack size of the threads that walk syntax trees.
pub const PARSE_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Component id used for analyzed files outside every component.
pub const ROOT_COMPONENT: &str = "root";

#[derive(Debug, Default, Clone, Copy)]
pub struct TypeScriptAdapter;

impl LanguageAdapter for TypeScriptAdapter {
    fn id(&self) -> &str {
        "typescript"
    }

    fn version(&self) -> String {
        format!(
            "onus-lang-ts {} (tree-sitter-typescript 0.23)",
            env!("CARGO_PKG_VERSION")
        )
    }

    fn handles(&self, path: &str) -> bool {
        lang::is_source(path)
    }

    fn build(&self, ws: &Workspace) -> Result<PartialMap, onus_core::ProviderError> {
        let patterns = patterns(ws).map_err(onus_core::ProviderError::Failed)?;
        let sources: Vec<&onus_core::WorkspaceFile> =
            ws.files.iter().filter(|f| self.handles(&f.path)).collect();
        let extract_all = || -> Vec<FileFacts> {
            sources
                .par_iter()
                .map(|f| {
                    let text = std::fs::read(onus_core::paths::native(&ws.root, &f.path))
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    extract::extract(&f.path, &text, f.is_test, &patterns)
                })
                .collect()
        };
        // The syntax-tree walks are recursive, and real code nests deeply
        // (long method chains, generated expressions), so parse on threads
        // with large stacks. Only the pages actually used are committed.
        let facts = match rayon::ThreadPoolBuilder::new()
            .stack_size(PARSE_STACK_BYTES)
            .build()
        {
            Ok(pool) => pool.install(extract_all),
            Err(_) => extract_all(),
        };
        let resolver = Resolver::new(
            &ws.root,
            ws.files.iter().map(|f| f.path.clone()),
            ws.packages.clone(),
        );
        Ok(Linker::new(ws, &facts, &resolver).link())
    }
}

/// The compiled packs for a workspace.
pub fn patterns(ws: &Workspace) -> Result<Patterns, String> {
    Patterns::new(
        &ws.extractors.publish_patterns,
        &ws.extractors.subscribe_patterns,
        &ws.extractors.prisma_clients,
        &ws.extractors.packs,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Target {
    Symbol(String),
    Module(usize),
    /// A file Onus does not analyze, such as a `.svelte` component.
    Asset,
    Npm(String),
}

struct FileInfo {
    component: String,
    rel: String,
}

struct Linker<'a> {
    ws: &'a Workspace,
    facts: &'a [FileFacts],
    resolver: &'a Resolver,
    by_path: HashMap<&'a str, usize>,
    info: Vec<FileInfo>,
    /// Per file: top-level declaration names.
    decls: Vec<HashMap<&'a str, usize>>,
    /// Per file: local import binding name → (import index, binding).
    bindings: Vec<HashMap<&'a str, (usize, &'a Binding)>>,
    /// Per file and import index: resolution of the specifier.
    resolutions: Vec<Vec<Resolution>>,
    export_memo: std::cell::RefCell<HashMap<(usize, String), Option<Target>>>,
    /// Symbol id → (file, decl index), for literal lookups.
    symbols: HashMap<String, (usize, usize)>,
}

impl<'a> Linker<'a> {
    fn new(ws: &'a Workspace, facts: &'a [FileFacts], resolver: &'a Resolver) -> Self {
        let by_path: HashMap<&str, usize> = facts
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.as_str(), i))
            .collect();
        let component_of: HashMap<&str, Option<&str>> = ws
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.component.as_deref()))
            .collect();
        let dirs: HashMap<&str, String> = ws
            .components
            .iter()
            .map(|c| (c.id.as_str(), component_dir(&c.roots)))
            .collect();
        let info: Vec<FileInfo> = facts
            .iter()
            .map(|f| {
                let component = component_of
                    .get(f.path.as_str())
                    .copied()
                    .flatten()
                    .unwrap_or(ROOT_COMPONENT)
                    .to_string();
                let dir = dirs.get(component.as_str()).cloned().unwrap_or_default();
                let rel = if dir.is_empty() {
                    f.path.clone()
                } else {
                    f.path
                        .strip_prefix(&format!("{dir}/"))
                        .unwrap_or(&f.path)
                        .to_string()
                };
                FileInfo { component, rel }
            })
            .collect();
        let decls = facts
            .iter()
            .map(|f| {
                f.decls
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| !d.name.contains('.'))
                    .map(|(i, d)| (d.name.as_str(), i))
                    .collect()
            })
            .collect();
        let bindings = facts
            .iter()
            .map(|f| {
                let mut m = HashMap::new();
                for (i, imp) in f.imports.iter().enumerate() {
                    for b in &imp.bindings {
                        m.insert(b.local.as_str(), (i, b));
                    }
                }
                m
            })
            .collect();
        let resolutions = facts
            .iter()
            .map(|f| {
                f.imports
                    .iter()
                    .map(|imp| resolver.resolve(&f.path, &imp.spec))
                    .collect()
            })
            .collect();
        let mut linker = Linker {
            ws,
            facts,
            resolver,
            by_path,
            info,
            decls,
            bindings,
            resolutions,
            export_memo: Default::default(),
            symbols: HashMap::new(),
        };
        for (fi, f) in facts.iter().enumerate() {
            if f.is_test {
                continue;
            }
            for (di, d) in f.decls.iter().enumerate() {
                linker
                    .symbols
                    .insert(linker.symbol_id(fi, &d.name), (fi, di));
            }
        }
        linker
    }

    fn symbol_id(&self, file: usize, name: &str) -> String {
        symbol_id(&self.info[file].component, &self.info[file].rel, name)
    }

    fn module_id(&self, file: usize) -> String {
        module_id(&self.info[file].component, &self.info[file].rel)
    }

    fn file_of(&self, resolution: &Resolution) -> Option<usize> {
        match resolution {
            Resolution::File(p) => self.by_path.get(p.as_str()).copied(),
            _ => None,
        }
    }

    fn resolve_spec(&self, file: usize, spec: &str) -> Option<Target> {
        match self.resolver.resolve(&self.facts[file].path, spec) {
            Resolution::File(p) => self.by_path.get(p.as_str()).map(|&i| Target::Module(i)),
            Resolution::Asset(_) => Some(Target::Asset),
            Resolution::Npm(p) => Some(Target::Npm(p)),
            Resolution::Unresolved => None,
        }
    }

    /// What `name` exported from `file` refers to, following re-exports.
    fn resolve_export(&self, file: usize, name: &str, depth: u32) -> Option<Target> {
        if depth > 32 {
            return None;
        }
        let key = (file, name.to_string());
        if let Some(hit) = self.export_memo.borrow().get(&key) {
            return hit.clone();
        }
        // Guard against cycles while computing.
        self.export_memo.borrow_mut().insert(key.clone(), None);
        let result = self.compute_export(file, name, depth);
        self.export_memo.borrow_mut().insert(key, result.clone());
        result
    }

    fn compute_export(&self, file: usize, name: &str, depth: u32) -> Option<Target> {
        let f = &self.facts[file];
        for e in &f.exports {
            match e {
                Export::Local {
                    exported, local, ..
                } if exported == name => {
                    if self.decls[file].contains_key(local.as_str()) {
                        return Some(Target::Symbol(self.symbol_id(file, local)));
                    }
                    if let Some((i, b)) = self.bindings[file].get(local.as_str()) {
                        return self.resolve_binding(file, *i, b, depth + 1);
                    }
                    return None;
                }
                Export::From {
                    spec,
                    exported,
                    imported,
                    ..
                } if exported == name => {
                    return match self.resolve_spec(file, spec)? {
                        Target::Module(t) => match imported {
                            Imported::Named(n) => self.resolve_export(t, n, depth + 1),
                            Imported::Default => self.resolve_export(t, "default", depth + 1),
                            Imported::Namespace => Some(Target::Module(t)),
                        },
                        other => Some(other),
                    };
                }
                _ => {}
            }
        }
        if name == "default" {
            return None;
        }
        for e in &f.exports {
            if let Export::Star { spec, .. } = e
                && let Some(Target::Module(t)) = self.resolve_spec(file, spec)
                && let Some(found) = self.resolve_export(t, name, depth + 1)
            {
                return Some(found);
            }
        }
        None
    }

    fn resolve_binding(
        &self,
        file: usize,
        import: usize,
        b: &Binding,
        depth: u32,
    ) -> Option<Target> {
        match &self.resolutions[file][import] {
            Resolution::File(_) => {
                let t = self.file_of(&self.resolutions[file][import])?;
                match &b.imported {
                    Imported::Named(n) => self.resolve_export(t, n, depth),
                    Imported::Default => self.resolve_export(t, "default", depth),
                    Imported::Namespace => Some(Target::Module(t)),
                }
            }
            Resolution::Asset(_) => Some(Target::Asset),
            Resolution::Npm(p) => Some(Target::Npm(p.clone())),
            Resolution::Unresolved => None,
        }
    }

    /// Every name a file exports, including through `export *`.
    fn export_names(&self, file: usize, seen: &mut BTreeSet<usize>, out: &mut BTreeSet<String>) {
        if !seen.insert(file) {
            return;
        }
        for e in &self.facts[file].exports {
            match e {
                Export::Local { exported, .. } | Export::From { exported, .. } => {
                    out.insert(exported.clone());
                }
                Export::Star { spec, .. } => {
                    if let Some(Target::Module(t)) = self.resolve_spec(file, spec) {
                        let mut inner = BTreeSet::new();
                        self.export_names(t, seen, &mut inner);
                        out.extend(inner.into_iter().filter(|n| n != "default"));
                    }
                }
            }
        }
    }

    fn public_symbols(&self) -> BTreeSet<String> {
        let mut public = BTreeSet::new();
        let mut modules = Vec::new();
        for c in &self.ws.components {
            for e in &c.public_entrypoints {
                if let Some(&f) = self.by_path.get(e.as_str()) {
                    modules.push(f);
                }
            }
        }
        let mut done = BTreeSet::new();
        while let Some(m) = modules.pop() {
            if !done.insert(m) {
                continue;
            }
            let mut names = BTreeSet::new();
            self.export_names(m, &mut BTreeSet::new(), &mut names);
            for n in names {
                match self.resolve_export(m, &n, 0) {
                    Some(Target::Symbol(id)) => {
                        public.insert(id);
                    }
                    Some(Target::Module(t)) => modules.push(t),
                    _ => {}
                }
            }
        }
        // Methods of public classes are public too.
        let classes: Vec<String> = public.iter().map(|id| format!("{id}.")).collect();
        for id in self.symbols.keys() {
            if classes.iter().any(|c| id.starts_with(c.as_str())) {
                public.insert(id.clone());
            }
        }
        public
    }

    fn external_slug(&self, package: &str) -> Option<&str> {
        self.ws
            .extractors
            .externals
            .get(package)
            .map(|e| e.slug.as_str())
    }

    fn host_slug(&self, host: &str) -> String {
        for e in self.ws.extractors.externals.values() {
            if e.spec
                .hosts
                .iter()
                .any(|h| host == h || host.ends_with(&format!(".{h}")))
            {
                return e.slug.clone();
            }
        }
        format!("host:{host}")
    }

    fn link(self) -> PartialMap {
        let mut edges: BTreeMap<(String, String, EdgeKind), (Confidence, BTreeSet<Site>)> =
            BTreeMap::new();
        let mut add =
            |from: &str, to: &str, kind: EdgeKind, conf: Confidence, file: &str, line: u32| {
                if from == to {
                    return;
                }
                let e = edges
                    .entry((from.to_string(), to.to_string(), kind))
                    .or_insert((conf, BTreeSet::new()));
                e.0 = e.0.max(conf);
                e.1.insert(Site {
                    file: file.to_string(),
                    line,
                });
            };
        let mut diagnostics: Vec<MapDiagnostic> = Vec::new();
        let mut files_out = Vec::new();
        let mut tests_out = Vec::new();
        let mut events = BTreeSet::new();
        let mut tables = BTreeSet::new();
        let mut config_keys = BTreeSet::new();
        let mut routes: Vec<(usize, String, u32)> = Vec::new();

        for (fi, f) in self.facts.iter().enumerate() {
            let module = self.module_id(fi);
            let from_id = |from: &Option<String>| match from {
                Some(n) if !f.is_test => self.symbol_id(fi, n),
                _ => module.clone(),
            };
            diagnostics.extend(f.diagnostics.iter().cloned());
            let mut exercised = BTreeSet::new();

            // Imports.
            let mut import_targets = Vec::new();
            let mut called_packages = BTreeSet::new();
            for (ii, imp) in f.imports.iter().enumerate() {
                let res = &self.resolutions[fi][ii];
                import_targets.push((
                    imp.line,
                    match res {
                        Resolution::File(p) | Resolution::Asset(p) => p.clone(),
                        Resolution::Npm(p) => ids::npm_id(p),
                        Resolution::Unresolved => format!("?{}", imp.spec),
                    },
                ));
                match res {
                    // Not analyzed, but present: nothing to link, nothing missing.
                    Resolution::Asset(_) => {}
                    Resolution::Unresolved => diagnostics.push(MapDiagnostic {
                        kind: "unresolved-import".into(),
                        file: f.path.clone(),
                        line: imp.line,
                        message: format!("cannot resolve `{}`", imp.spec),
                        confidence: Confidence::Low,
                    }),
                    Resolution::Npm(p) => {
                        add(
                            &module,
                            &ids::npm_id(p),
                            EdgeKind::Imports,
                            Confidence::Static,
                            &f.path,
                            imp.line,
                        );
                        if imp.bindings.is_empty()
                            && !f.is_test
                            && let Some(slug) = self.external_slug(p)
                        {
                            add(
                                &module,
                                &ids::external_id(slug),
                                EdgeKind::CallsExternal,
                                Confidence::Inferred,
                                &f.path,
                                imp.line,
                            );
                        }
                    }
                    Resolution::File(_) => {
                        if imp.bindings.is_empty()
                            && !f.is_test
                            && let Some(t) = self.file_of(res)
                        {
                            add(
                                &module,
                                &self.module_id(t),
                                EdgeKind::Imports,
                                Confidence::Static,
                                &f.path,
                                imp.line,
                            );
                        }
                        for b in &imp.bindings {
                            match self.resolve_binding(fi, ii, b, 0) {
                                Some(Target::Asset) => {}
                                Some(Target::Symbol(id)) => {
                                    exercised.insert(id.clone());
                                    add(
                                        &module,
                                        &id,
                                        EdgeKind::Imports,
                                        Confidence::Static,
                                        &f.path,
                                        b.line,
                                    );
                                }
                                Some(Target::Module(t)) => {
                                    add(
                                        &module,
                                        &self.module_id(t),
                                        EdgeKind::Imports,
                                        Confidence::Static,
                                        &f.path,
                                        b.line,
                                    );
                                }
                                Some(Target::Npm(p)) => {
                                    add(
                                        &module,
                                        &ids::npm_id(&p),
                                        EdgeKind::Imports,
                                        Confidence::Static,
                                        &f.path,
                                        b.line,
                                    );
                                }
                                None => diagnostics.push(MapDiagnostic {
                                    kind: "unresolved-import".into(),
                                    file: f.path.clone(),
                                    line: b.line,
                                    message: format!(
                                        "`{}` is not exported by `{}`",
                                        match &b.imported {
                                            Imported::Named(n) => n.as_str(),
                                            Imported::Default => "default",
                                            Imported::Namespace => "*",
                                        },
                                        imp.spec
                                    ),
                                    confidence: Confidence::Low,
                                }),
                            }
                        }
                    }
                }
            }
            // Re-exports.
            for e in &f.exports {
                if let Export::From { spec, line, .. } | Export::Star { spec, line } = e {
                    import_targets.push((
                        *line,
                        match self.resolver.resolve(&f.path, spec) {
                            Resolution::File(p) | Resolution::Asset(p) => p,
                            Resolution::Npm(p) => ids::npm_id(&p),
                            Resolution::Unresolved => format!("?{spec}"),
                        },
                    ));
                }
                match e {
                    Export::From {
                        spec,
                        exported,
                        line,
                        ..
                    } => match self.resolve_export(fi, exported, 0) {
                        Some(Target::Asset) => {}
                        Some(Target::Symbol(id)) => add(
                            &module,
                            &id,
                            EdgeKind::Imports,
                            Confidence::Static,
                            &f.path,
                            *line,
                        ),
                        Some(Target::Module(t)) => add(
                            &module,
                            &self.module_id(t),
                            EdgeKind::Imports,
                            Confidence::Static,
                            &f.path,
                            *line,
                        ),
                        Some(Target::Npm(p)) => add(
                            &module,
                            &ids::npm_id(&p),
                            EdgeKind::Imports,
                            Confidence::Static,
                            &f.path,
                            *line,
                        ),
                        None => diagnostics.push(MapDiagnostic {
                            kind: "unresolved-import".into(),
                            file: f.path.clone(),
                            line: *line,
                            message: format!("cannot resolve re-export `{exported}` from `{spec}`"),
                            confidence: Confidence::Low,
                        }),
                    },
                    Export::Star { spec, line } => match self.resolve_spec(fi, spec) {
                        Some(Target::Module(t)) => add(
                            &module,
                            &self.module_id(t),
                            EdgeKind::Imports,
                            Confidence::Static,
                            &f.path,
                            *line,
                        ),
                        Some(Target::Npm(p)) => add(
                            &module,
                            &ids::npm_id(&p),
                            EdgeKind::Imports,
                            Confidence::Static,
                            &f.path,
                            *line,
                        ),
                        _ => diagnostics.push(MapDiagnostic {
                            kind: "unresolved-import".into(),
                            file: f.path.clone(),
                            line: *line,
                            message: format!("cannot resolve `export * from \"{spec}\"`"),
                            confidence: Confidence::Low,
                        }),
                    },
                    Export::Local { .. } => {}
                }
            }

            // References.
            for r in &f.refs {
                let target = if self.decls[fi].contains_key(r.name.as_str()) && !f.is_test {
                    Some(Target::Symbol(self.symbol_id(fi, &r.name)))
                } else if let Some((ii, b)) = self.bindings[fi].get(r.name.as_str()) {
                    match self.resolve_binding(fi, *ii, b, 0) {
                        Some(Target::Module(t)) => match &r.member {
                            Some(m) => self.resolve_export(t, m, 0),
                            None => Some(Target::Module(t)),
                        },
                        other => other,
                    }
                } else {
                    None
                };
                let from = from_id(&r.from);
                match target {
                    Some(Target::Symbol(id)) => {
                        exercised.insert(id.clone());
                        let kind = match r.kind {
                            RefKind::Call => Some(EdgeKind::Calls),
                            RefKind::Type => Some(EdgeKind::ReferencesType),
                            RefKind::Value => match self.symbols.get(&id) {
                                Some((tf, td)) => matches!(
                                    self.facts[*tf].decls[*td].kind,
                                    SymbolKind::Function | SymbolKind::Method
                                )
                                .then_some(EdgeKind::Calls),
                                None => None,
                            },
                        };
                        if let Some(kind) = kind {
                            add(&from, &id, kind, Confidence::Static, &f.path, r.line);
                        }
                    }
                    Some(Target::Npm(p)) => {
                        if !f.is_test
                            && (r.kind == RefKind::Call
                                || (r.kind == RefKind::Value && r.member.is_some()))
                            && let Some(slug) = self.external_slug(&p)
                        {
                            called_packages.insert(p.clone());
                            add(
                                &from,
                                &ids::external_id(slug),
                                EdgeKind::CallsExternal,
                                Confidence::Static,
                                &f.path,
                                r.line,
                            );
                        }
                    }
                    _ => {}
                }
            }
            // Registered SDKs imported but never called in this file.
            for (ii, imp) in f.imports.iter().enumerate() {
                if let Resolution::Npm(p) = &self.resolutions[fi][ii]
                    && !f.is_test
                    && !imp.bindings.is_empty()
                    && !called_packages.contains(p)
                    && let Some(slug) = self.external_slug(p)
                {
                    add(
                        &module,
                        &ids::external_id(slug),
                        EdgeKind::CallsExternal,
                        Confidence::Inferred,
                        &f.path,
                        imp.line,
                    );
                }
            }

            // Events, data, config and outbound calls describe what the
            // product does; a test that publishes an event or calls a host
            // does not add a relationship.
            let production = !f.is_test;
            for ev in f.events.iter().filter(|_| production) {
                let (name, conf) = self.event_name(fi, &ev.name);
                if conf == Confidence::Low {
                    diagnostics.push(MapDiagnostic {
                        kind: "dynamic-event-name".into(),
                        file: f.path.clone(),
                        line: ev.line,
                        message: format!("event name `{name}` is computed at runtime"),
                        confidence: Confidence::Low,
                    });
                }
                let id = ids::event_id(&name);
                events.insert(name);
                let kind = if ev.publish {
                    EdgeKind::Publishes
                } else {
                    EdgeKind::Consumes
                };
                add(&from_id(&ev.from), &id, kind, conf, &f.path, ev.line);
            }
            for d in f.data.iter().filter(|_| production) {
                tables.insert(d.model.clone());
                let kind = if d.write {
                    EdgeKind::Writes
                } else {
                    EdgeKind::Reads
                };
                add(
                    &from_id(&d.from),
                    &ids::table_id(&d.model),
                    kind,
                    Confidence::Static,
                    &f.path,
                    d.line,
                );
            }
            for e in f.env.iter().filter(|_| production) {
                config_keys.insert(e.value.clone());
                add(
                    &from_id(&e.from),
                    &ids::config_key_id(&e.value),
                    EdgeKind::ReadsConfig,
                    Confidence::Static,
                    &f.path,
                    e.line,
                );
            }
            for r in f.routes.iter().filter(|_| production) {
                routes.push((fi, r.value.clone(), r.line));
            }
            for (from, kind, to, line) in f.edges.iter().filter(|_| production) {
                add(
                    &from_id(from),
                    to,
                    *kind,
                    Confidence::Static,
                    &f.path,
                    *line,
                );
            }
            for h in f.hosts.iter().filter(|_| production) {
                let slug = self.host_slug(&h.value);
                add(
                    &from_id(&h.from),
                    &ids::external_id(&slug),
                    EdgeKind::CallsExternal,
                    Confidence::Static,
                    &f.path,
                    h.line,
                );
            }

            files_out.push(SourceFile {
                path: f.path.clone(),
                component_id: Some(self.info[fi].component.clone()),
                language: language_name(&f.path).to_string(),
                is_test: f.is_test,
                lines: f.lines,
                content_hash: f.content_hash.clone(),
                imports: {
                    import_targets.sort();
                    let mut targets: Vec<String> =
                        import_targets.into_iter().map(|(_, t)| t).collect();
                    // One entry per statement, not per re-exported name.
                    targets.dedup();
                    targets
                },
            });
            if f.is_test {
                tests_out.push(TestNode {
                    id: module.clone(),
                    component_id: Some(self.info[fi].component.clone()),
                    file: f.path.clone(),
                    cases: f.test_cases.clone(),
                    exercises: exercised.into_iter().collect(),
                    literals: f.literals.clone(),
                });
            }
        }

        // Symbols.
        let public = self.public_symbols();
        let mut symbols = Vec::new();
        for (fi, f) in self.facts.iter().enumerate() {
            if f.is_test {
                continue;
            }
            for d in &f.decls {
                let id = self.symbol_id(fi, &d.name);
                symbols.push(SymbolNode {
                    visibility: if public.contains(&id) {
                        Visibility::Public
                    } else {
                        Visibility::Internal
                    },
                    id,
                    component_id: Some(self.info[fi].component.clone()),
                    kind: d.kind,
                    name: d.name.clone(),
                    shape: d.shape.clone(),
                    body_fingerprint: Some(d.fingerprint.clone()),
                    invariants: vec![],
                    loc: Some(Loc {
                        file: f.path.clone(),
                        start: d.start,
                        end: d.end,
                        signature_end: d.signature_end,
                    }),
                    facts: d.facts.clone(),
                    literal: d.literal.clone(),
                });
            }
        }
        // HTTP routes from packs: public contracts of their component.
        for (fi, name, line) in &routes {
            symbols.push(SymbolNode {
                id: self.symbol_id(*fi, name),
                component_id: Some(self.info[*fi].component.clone()),
                kind: SymbolKind::HttpRoute,
                name: name.clone(),
                visibility: Visibility::Public,
                shape: Some(onus_core::ContractShape {
                    kind: onus_core::ShapeKind::Alias,
                    type_params: None,
                    params: vec![],
                    returns: None,
                    members: vec![],
                    type_text: Some(name.clone()),
                    unverified: false,
                }),
                body_fingerprint: None,
                invariants: vec![],
                loc: Some(Loc {
                    file: self.facts[*fi].path.clone(),
                    start: *line,
                    end: *line,
                    signature_end: None,
                }),
                facts: None,
                literal: None,
            });
        }
        for (prefix, kind, names) in [
            ("event", SymbolKind::Event, &events),
            ("table", SymbolKind::DbTable, &tables),
            ("config", SymbolKind::ConfigKey, &config_keys),
        ] {
            for n in names {
                let id = match prefix {
                    "event" => ids::event_id(n),
                    "table" => ids::table_id(n),
                    _ => ids::config_key_id(n),
                };
                symbols.push(SymbolNode {
                    id,
                    component_id: None,
                    kind,
                    name: n.clone(),
                    visibility: Visibility::Public,
                    shape: None,
                    body_fingerprint: None,
                    invariants: vec![],
                    loc: None,
                    facts: None,
                    literal: None,
                });
            }
        }
        symbols.sort_by(|a, b| a.id.cmp(&b.id));
        symbols.dedup_by(|a, b| a.id == b.id);

        let edges = edges
            .into_iter()
            .map(|((from, to, kind), (confidence, sites))| Edge {
                from,
                to,
                kind,
                confidence,
                sites: sites.into_iter().collect(),
            })
            .collect();
        diagnostics.sort();
        diagnostics.dedup();
        PartialMap {
            files: files_out,
            symbols,
            edges,
            tests: tests_out,
            diagnostics,
        }
    }

    /// The event name an expression refers to, and how sure Onus is.
    fn event_name(&self, file: usize, expr: &EventExpr) -> (String, Confidence) {
        match expr {
            EventExpr::Literal(s) => (s.clone(), Confidence::Static),
            EventExpr::Ident(n) => match self.literal_of(file, n, None) {
                Some(v) => (v, Confidence::Static),
                None => (n.clone(), Confidence::Low),
            },
            EventExpr::Member(o, p) => match self.literal_of(file, o, Some(p)) {
                Some(v) => (v, Confidence::Static),
                None => (format!("{o}.{p}"), Confidence::Low),
            },
            EventExpr::Dynamic(t) => (
                if t.is_empty() {
                    "?".to_string()
                } else {
                    t.clone()
                },
                Confidence::Low,
            ),
        }
    }

    fn literal_of(&self, file: usize, name: &str, member: Option<&str>) -> Option<String> {
        let id = if self.decls[file].contains_key(name) {
            self.symbol_id(file, name)
        } else {
            let (ii, b) = self.bindings[file].get(name)?;
            match self.resolve_binding(file, *ii, b, 0)? {
                Target::Symbol(id) => id,
                Target::Module(t) => {
                    let m = member?;
                    return match self.resolve_export(t, m, 0)? {
                        Target::Symbol(id) => {
                            let (tf, td) = self.symbols.get(&id)?;
                            self.facts[*tf].decls[*td].literal.clone()
                        }
                        _ => None,
                    };
                }
                Target::Npm(_) | Target::Asset => return None,
            }
        };
        let (tf, td) = self.symbols.get(&id)?;
        let d = &self.facts[*tf].decls[*td];
        match member {
            None => d.literal.clone(),
            Some(m) => d
                .enum_values
                .iter()
                .find(|(n, _)| n == m)
                .map(|(_, v)| v.clone()),
        }
    }
}

fn language_name(path: &str) -> &'static str {
    match lang::extension(path) {
        Some("ts" | "tsx" | "mts" | "cts") => "typescript",
        _ => "javascript",
    }
}

/// The directory a component's first root glob points at.
pub fn component_dir(roots: &[String]) -> String {
    let Some(first) = roots.first() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for seg in first.split('/') {
        if seg.contains(['*', '?', '[', '{']) {
            break;
        }
        parts.push(seg);
    }
    parts.join("/")
}
