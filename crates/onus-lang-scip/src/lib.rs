//! Imports SCIP indexes (ADR 0006).
//!
//! SCIP indexers are written by or on top of each language's own compiler
//! (rust-analyzer, scip-typescript, scip-java, scip-python, scip-go) and
//! index a whole repository in one pass. Importing an index runs nothing:
//! Onus only reads the file.
//!
//! For files another provider already analyzed, the index adds references
//! that provider missed and confirms the ones it found (confidence
//! `compiler`). For files no provider handles, it adds the symbols too,
//! with their signatures as shapes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use onus_core::hash::short_hash;
use onus_core::ids::{self, module_id, symbol_id};
use onus_core::{
    Component, Confidence, ContractShape, Edge, EdgeKind, FactProvider, Loc, PartialMap,
    ProviderError, ShapeKind, Site, SourceFile, SymbolKind, SymbolNode, Visibility, Workspace,
};
use protobuf::Message;
use scip::types::descriptor::Suffix;
use scip::types::symbol_information::Kind;
use scip::types::{Index, Occurrence, SymbolInformation, SymbolRole};

#[derive(Debug)]
pub struct ScipImport {
    id: String,
    index: PathBuf,
    tool: Mutex<Option<String>>,
}

impl ScipImport {
    pub fn new(id: String, index: PathBuf) -> Self {
        ScipImport {
            id,
            index,
            tool: Mutex::new(None),
        }
    }
}

impl FactProvider for ScipImport {
    fn id(&self) -> &str {
        &self.id
    }

    fn version(&self) -> String {
        self.tool
            .lock()
            .ok()
            .and_then(|t| t.clone())
            .unwrap_or_else(|| "scip".into())
    }

    fn facts(&self, ws: &Workspace, so_far: &PartialMap) -> Result<PartialMap, ProviderError> {
        let bytes = std::fs::read(&self.index).map_err(|e| {
            ProviderError::Failed(format!("cannot read {}: {e}", self.index.display()))
        })?;
        let index = Index::parse_from_bytes(&bytes).map_err(|e| {
            ProviderError::Failed(format!("{} is not a SCIP index: {e}", self.index.display()))
        })?;
        if let Some(tool) = index.metadata.tool_info.as_ref()
            && let Ok(mut slot) = self.tool.lock()
        {
            *slot = Some(format!("{} {}", tool.name, tool.version).trim().to_string());
        }
        Ok(import(&index, ws, so_far))
    }
}

/// A SCIP symbol defined in the index.
#[derive(Debug, Clone)]
struct Def {
    file: String,
    line: u32,
    end: u32,
    names: Vec<String>,
    last: Suffix,
    info: Option<SymbolInformation>,
}

fn has_role(o: &Occurrence, role: SymbolRole) -> bool {
    o.symbol_roles & (role as i32) != 0
}

/// Start line, and end line of the enclosing range when there is one.
fn lines(o: &Occurrence) -> (u32, Option<u32>) {
    let start = o.range.first().copied().unwrap_or(0).max(0) as u32 + 1;
    let end = match o.enclosing_range.len() {
        4 => Some(o.enclosing_range[2].max(0) as u32 + 1),
        3 => Some(o.enclosing_range[0].max(0) as u32 + 1),
        _ => None,
    };
    (start, end)
}

/// Descriptor names that make up a qualified name (`Class.method`), and the
/// last suffix; `None` for locals, parameters and type parameters.
fn qualified(symbol: &str) -> Option<(Vec<String>, Suffix)> {
    if symbol.starts_with("local ") {
        return None;
    }
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let mut names = Vec::new();
    let mut last = Suffix::UnspecifiedSuffix;
    for d in &parsed.descriptors {
        let suffix = d.suffix.enum_value().unwrap_or(Suffix::UnspecifiedSuffix);
        match suffix {
            Suffix::Namespace | Suffix::Package | Suffix::Meta => {}
            Suffix::Parameter | Suffix::TypeParameter | Suffix::Local => return None,
            _ => {
                names.push(d.name.clone());
                last = suffix;
            }
        }
    }
    (!names.is_empty()).then_some((names, last))
}

fn package_of(symbol: &str) -> Option<(String, String)> {
    let parsed = scip::symbol::parse_symbol(symbol).ok()?;
    let p = parsed.package.as_ref()?;
    Some((p.manager.clone(), p.name.clone()))
}

fn symbol_kind(info: Option<&SymbolInformation>, last: Suffix) -> SymbolKind {
    let kind = info.and_then(|i| i.kind.enum_value().ok());
    match kind {
        Some(Kind::Class | Kind::Struct | Kind::Object | Kind::SingletonClass) => SymbolKind::Class,
        Some(Kind::Interface | Kind::Trait | Kind::Protocol | Kind::TypeClass) => {
            SymbolKind::Interface
        }
        Some(Kind::Enum) => SymbolKind::Enum,
        Some(Kind::TypeAlias | Kind::Type | Kind::Union) => SymbolKind::Type,
        Some(Kind::Method | Kind::StaticMethod | Kind::AbstractMethod | Kind::TraitMethod) => {
            SymbolKind::Method
        }
        Some(Kind::Function | Kind::Macro) => SymbolKind::Function,
        Some(Kind::Constant) => SymbolKind::Const,
        Some(Kind::Variable | Kind::StaticVariable) => SymbolKind::Variable,
        _ => match last {
            Suffix::Type => SymbolKind::Type,
            Suffix::Method | Suffix::Macro => SymbolKind::Function,
            _ => SymbolKind::Const,
        },
    }
}

fn is_callable(info: Option<&SymbolInformation>, last: Suffix) -> bool {
    matches!(last, Suffix::Method | Suffix::Macro)
        || matches!(
            info.and_then(|i| i.kind.enum_value().ok()),
            Some(Kind::Function | Kind::Method | Kind::StaticMethod | Kind::Constructor)
        )
}

fn component_dir(c: &Component) -> String {
    let Some(first) = c.roots.first() else {
        return String::new();
    };
    first
        .split('/')
        .take_while(|s| !s.contains(['*', '?', '[', '{']))
        .collect::<Vec<_>>()
        .join("/")
}

struct Ids<'a> {
    component_of: HashMap<&'a str, &'a str>,
    dirs: HashMap<&'a str, String>,
}

impl Ids<'_> {
    fn component(&self, file: &str) -> String {
        self.component_of
            .get(file)
            .copied()
            .unwrap_or("root")
            .to_string()
    }

    fn rel<'f>(&self, file: &'f str) -> &'f str {
        let c = self.component_of.get(file).copied().unwrap_or("root");
        match self.dirs.get(c) {
            Some(d) if !d.is_empty() => file.strip_prefix(&format!("{d}/")).unwrap_or(file),
            _ => file,
        }
    }

    fn module(&self, file: &str) -> String {
        module_id(&self.component(file), self.rel(file))
    }

    fn symbol(&self, file: &str, name: &str) -> String {
        symbol_id(&self.component(file), self.rel(file), name)
    }
}

fn import(index: &Index, ws: &Workspace, so_far: &PartialMap) -> PartialMap {
    let ids = Ids {
        component_of: ws
            .files
            .iter()
            .filter_map(|f| f.component.as_deref().map(|c| (f.path.as_str(), c)))
            .collect(),
        dirs: ws
            .components
            .iter()
            .map(|c| (c.id.as_str(), component_dir(c)))
            .collect(),
    };
    let known_files: BTreeSet<&str> = ws.files.iter().map(|f| f.path.as_str()).collect();
    let covered: BTreeSet<&str> = so_far.files.iter().map(|f| f.path.as_str()).collect();
    let existing: BTreeSet<&str> = so_far.symbols.iter().map(|s| s.id.as_str()).collect();
    // Import and re-export lines another provider found. Some indexers do
    // not mark occurrences in `import`/`export ... from` with the Import
    // role; there they are imports, not calls.
    let import_lines: BTreeSet<(&str, u32)> = so_far
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .flat_map(|e| e.sites.iter().map(|s| (s.file.as_str(), s.line)))
        .collect();
    // Symbols already in the map, by file, to attribute references.
    let mut enclosing: BTreeMap<&str, Vec<(u32, u32, &str)>> = BTreeMap::new();
    for s in &so_far.symbols {
        if let Some(l) = &s.loc {
            enclosing
                .entry(l.file.as_str())
                .or_default()
                .push((l.start, l.end, s.id.as_str()));
        }
    }

    // Definitions.
    let infos: HashMap<&str, &SymbolInformation> = index
        .documents
        .iter()
        .flat_map(|d| d.symbols.iter())
        .chain(index.external_symbols.iter())
        .map(|i| (i.symbol.as_str(), i))
        .collect();
    let mut defs: BTreeMap<String, Def> = BTreeMap::new();
    for doc in &index.documents {
        let file = doc.relative_path.as_str();
        if !known_files.contains(file) {
            continue;
        }
        for o in &doc.occurrences {
            if !has_role(o, SymbolRole::Definition) {
                continue;
            }
            let Some((names, last)) = qualified(&o.symbol) else {
                continue;
            };
            let (line, end) = lines(o);
            defs.entry(o.symbol.clone()).or_insert(Def {
                file: file.to_string(),
                line,
                end: end.unwrap_or(line),
                names,
                last,
                info: infos.get(o.symbol.as_str()).map(|i| (*i).clone()),
            });
        }
    }

    // The map id of a defined SCIP symbol: the full qualified name if the
    // map has it, else its top-level declaration, else the module.
    let target_id = |d: &Def| -> String {
        let full = ids.symbol(&d.file, &d.names.join("."));
        if !covered.contains(d.file.as_str()) || existing.contains(full.as_str()) {
            return full;
        }
        let top = ids.symbol(&d.file, &d.names[0]);
        if existing.contains(top.as_str()) {
            top
        } else {
            ids.module(&d.file)
        }
    };

    let mut out = PartialMap::default();
    let mut edges: BTreeMap<(String, String, EdgeKind), BTreeSet<Site>> = BTreeMap::new();
    let mut referenced_from: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for doc in &index.documents {
        let file = doc.relative_path.as_str();
        if !known_files.contains(file) {
            continue;
        }
        let module = ids.module(file);
        // Enclosing symbols in this file: the map's, or this index's own
        // definitions with enclosing ranges.
        let own: Vec<(u32, u32, String)> = if covered.contains(file) {
            enclosing
                .get(file)
                .map(|v| {
                    v.iter()
                        .map(|(a, b, id)| (*a, *b, id.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            defs.values()
                .filter(|d| d.file == file && d.end > d.line)
                .map(|d| (d.line, d.end, target_id(d)))
                .collect()
        };
        for o in &doc.occurrences {
            if has_role(o, SymbolRole::Definition) {
                continue;
            }
            let (line, _) = lines(o);
            let from = own
                .iter()
                .filter(|(a, b, _)| *a <= line && line <= *b)
                .min_by_key(|(a, b, _)| b - a)
                .map_or(module.clone(), |(_, _, id)| id.clone());
            let site = Site {
                file: file.to_string(),
                line,
            };
            match defs.get(&o.symbol) {
                Some(def) => {
                    let to = target_id(def);
                    if to == from
                        || to == ids.module(&def.file)
                        || from.starts_with(&format!("{to}."))
                    {
                        // A symbol the map has no node for, inside a
                        // declaration it does not model either.
                        continue;
                    }
                    // A member the map does not model (an interface method,
                    // a nested function) stands for its declaration: that is
                    // a use of the declaration, not a call to it.
                    let fell_back = to != ids.symbol(&def.file, &def.names.join("."));
                    let kind = if has_role(o, SymbolRole::Import)
                        || import_lines.contains(&(file, line))
                    {
                        Some(EdgeKind::Imports)
                    } else if fell_back {
                        Some(EdgeKind::ReferencesType)
                    } else if is_callable(def.info.as_ref(), def.last) {
                        Some(EdgeKind::Calls)
                    } else if def.last == Suffix::Type {
                        Some(EdgeKind::ReferencesType)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        let from = if kind == EdgeKind::Imports {
                            module.clone()
                        } else {
                            from
                        };
                        referenced_from
                            .entry(o.symbol.clone())
                            .or_default()
                            .insert(ids.component(file));
                        edges.entry((from, to, kind)).or_default().insert(site);
                    }
                }
                None if has_role(o, SymbolRole::Import) => {
                    if let Some((manager, name)) = package_of(&o.symbol)
                        && manager == "npm"
                        && !name.is_empty()
                    {
                        edges
                            .entry((module.clone(), ids::npm_id(&name), EdgeKind::Imports))
                            .or_default()
                            .insert(site);
                    }
                }
                None => {}
            }
        }
    }
    out.edges = edges
        .into_iter()
        .map(|((from, to, kind), sites)| Edge {
            from,
            to,
            kind,
            confidence: Confidence::Compiler,
            sites: sites.into_iter().collect(),
        })
        .collect();

    // Symbols and files for languages no other provider handled.
    let is_test: HashMap<&str, bool> = ws
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.is_test))
        .collect();
    for doc in &index.documents {
        let file = doc.relative_path.as_str();
        if !known_files.contains(file) || covered.contains(file) {
            continue;
        }
        let bytes = std::fs::read(onus_core::paths::native(&ws.root, file)).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        out.files.push(SourceFile {
            path: file.to_string(),
            component_id: Some(ids.component(file)),
            language: if doc.language.is_empty() {
                "unknown".into()
            } else {
                doc.language.to_ascii_lowercase()
            },
            is_test: is_test.get(file).copied().unwrap_or(false),
            lines: text.lines().count() as u32,
            content_hash: short_hash(&bytes),
            imports: vec![],
        });
        if is_test.get(file).copied().unwrap_or(false) {
            continue;
        }
        for (symbol, def) in defs.iter().filter(|(_, d)| d.file == file) {
            let own = ids.component(file);
            let public = referenced_from
                .get(symbol)
                .is_some_and(|comps| comps.iter().any(|c| *c != own));
            let signature = def
                .info
                .as_ref()
                .and_then(|i| i.signature_documentation.as_ref())
                .map(|s| s.text.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|s| !s.is_empty());
            out.symbols.push(SymbolNode {
                id: target_id(def),
                component_id: Some(own),
                kind: symbol_kind(def.info.as_ref(), def.last),
                name: def.names.join("."),
                visibility: if public {
                    Visibility::Public
                } else {
                    Visibility::Internal
                },
                shape: signature.map(|text| ContractShape {
                    kind: ShapeKind::Alias,
                    type_params: None,
                    params: vec![],
                    returns: None,
                    members: vec![],
                    type_text: Some(text),
                    unverified: false,
                }),
                body_fingerprint: None,
                invariants: vec![],
                loc: Some(Loc {
                    file: file.to_string(),
                    start: def.line,
                    end: def.end,
                    signature_end: None,
                }),
                facts: None,
                literal: None,
            });
        }
    }
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    out.symbols.sort_by(|a, b| a.id.cmp(&b.id));
    out.symbols.dedup_by(|a, b| a.id == b.id);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_core::{ComponentKind, WorkspaceFile};
    use scip::types::{Document, Metadata, ToolInfo};

    fn occ(symbol: &str, line: i32, roles: i32, enclosing: Option<(i32, i32)>) -> Occurrence {
        let mut o = Occurrence::new();
        o.symbol = symbol.into();
        o.range = vec![line, 0, 3];
        o.symbol_roles = roles;
        if let Some((a, b)) = enclosing {
            o.enclosing_range = vec![a, 0, b, 1];
        }
        o
    }

    fn component(id: &str, dir: &str) -> Component {
        Component {
            id: id.into(),
            kind: ComponentKind::Package,
            roots: vec![format!("{dir}/**")],
            public_entrypoints: vec![],
            owners: vec![],
            labels: vec![],
            package_name: None,
            confidence: Confidence::Inferred,
        }
    }

    const ADD: &str = "scip-python python lib 0.1 `lib.math`/add().";
    const SEND: &str = "scip-typescript npm web 1.0 src/`client.ts`/Client#send().";

    fn fixture() -> (tempfile::TempDir, Workspace, PartialMap, Index) {
        let dir = tempfile::tempdir().unwrap();
        for (p, t) in [
            ("lib/math.py", "def add(a, b):\n    return a + b\n"),
            (
                "app/main.py",
                "from lib.math import add\n\n\nprint(add(1, 2))\n",
            ),
            ("web/src/a.ts", "x\n"),
            ("web/src/client.ts", "x\n"),
        ] {
            let f = dir.path().join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        }
        let wf = |p: &str, c: &str| WorkspaceFile {
            path: p.into(),
            component: Some(c.into()),
            is_test: false,
        };
        let ws = Workspace {
            root: dir.path().to_path_buf(),
            files: vec![
                wf("app/main.py", "app"),
                wf("lib/math.py", "lib"),
                wf("web/src/a.ts", "web"),
                wf("web/src/client.ts", "web"),
            ],
            components: vec![
                component("app", "app"),
                component("lib", "lib"),
                component("web", "web"),
            ],
            packages: Default::default(),
            extractors: Default::default(),
        };
        // What the TypeScript adapter already found in `web`.
        let sym = |id: &str, file: &str, start: u32, end: u32| SymbolNode {
            id: id.into(),
            component_id: Some("web".into()),
            kind: SymbolKind::Function,
            name: id.rsplit('#').next().unwrap().into(),
            visibility: Visibility::Internal,
            shape: None,
            body_fingerprint: None,
            invariants: vec![],
            loc: Some(Loc {
                file: file.into(),
                start,
                end,
                signature_end: None,
            }),
            facts: None,
            literal: None,
        };
        let src = |p: &str| SourceFile {
            path: p.into(),
            component_id: Some("web".into()),
            language: "typescript".into(),
            is_test: false,
            lines: 1,
            content_hash: String::new(),
            imports: vec![],
        };
        let so_far = PartialMap {
            // The TypeScript adapter saw `import { Client } from "./client"`
            // on line 1 of a.ts.
            edges: vec![Edge {
                from: "web:src/a.ts".into(),
                to: "web:src/client.ts#Client".into(),
                kind: EdgeKind::Imports,
                confidence: Confidence::Static,
                sites: vec![Site {
                    file: "web/src/a.ts".into(),
                    line: 1,
                }],
            }],
            files: vec![src("web/src/a.ts"), src("web/src/client.ts")],
            symbols: vec![
                sym("web:src/a.ts#run", "web/src/a.ts", 1, 10),
                sym("web:src/client.ts#Client.send", "web/src/client.ts", 3, 5),
            ],
            ..PartialMap::default()
        };
        let mut index = Index::new();
        let mut meta = Metadata::new();
        let mut tool = ToolInfo::new();
        tool.name = "test-indexer".into();
        tool.version = "1.0".into();
        meta.tool_info = Some(tool).into();
        index.metadata = Some(meta).into();
        let def = SymbolRole::Definition as i32;
        let import = SymbolRole::Import as i32;
        let read = SymbolRole::ReadAccess as i32;
        let doc = |path: &str, occurrences: Vec<Occurrence>| {
            let mut d = Document::new();
            d.relative_path = path.into();
            d.language = "Python".into();
            d.occurrences = occurrences;
            d
        };
        let mut info = SymbolInformation::new();
        info.symbol = ADD.into();
        info.kind = Kind::Function.into();
        let mut sig = scip::types::Signature::new();
        sig.text = "def add(a,   b)".into();
        info.signature_documentation = Some(sig).into();
        let mut math = doc("lib/math.py", vec![occ(ADD, 0, def, Some((0, 1)))]);
        math.symbols = vec![info];
        index.documents = vec![
            math,
            doc(
                "app/main.py",
                vec![occ(ADD, 0, import, None), occ(ADD, 3, read, None)],
            ),
            doc("web/src/client.ts", vec![occ(SEND, 2, def, None)]),
            // A method call through an instance, which syntax alone cannot
            // resolve.
            doc(
                "web/src/a.ts",
                vec![occ(SEND, 0, read, None), occ(SEND, 4, read, None)],
            ),
        ];
        (dir, ws, so_far, index)
    }

    #[test]
    fn adds_symbols_and_compiler_confirmed_references() {
        let (_dir, ws, so_far, index) = fixture();
        let m = import(&index, &ws, &so_far);
        let edges: Vec<(&str, &str, EdgeKind, Confidence)> = m
            .edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.kind, e.confidence))
            .collect();
        assert_eq!(
            edges,
            [
                (
                    "app:main.py",
                    "lib:math.py#add",
                    EdgeKind::Imports,
                    Confidence::Compiler
                ),
                (
                    "app:main.py",
                    "lib:math.py#add",
                    EdgeKind::Calls,
                    Confidence::Compiler
                ),
                // Line 1 is the import, not a call from the module.
                (
                    "web:src/a.ts",
                    "web:src/client.ts#Client.send",
                    EdgeKind::Imports,
                    Confidence::Compiler
                ),
                (
                    "web:src/a.ts#run",
                    "web:src/client.ts#Client.send",
                    EdgeKind::Calls,
                    Confidence::Compiler
                ),
            ]
        );
        // Python has no other provider: the index supplies the symbol, public
        // because another component uses it, with its signature as shape.
        assert_eq!(m.symbols.len(), 1);
        let add = &m.symbols[0];
        assert_eq!(add.id, "lib:math.py#add");
        assert_eq!(add.kind, SymbolKind::Function);
        assert_eq!(add.visibility, Visibility::Public);
        assert_eq!(
            add.shape.as_ref().unwrap().type_text.as_deref(),
            Some("def add(a, b)")
        );
        assert_eq!(
            m.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["app/main.py", "lib/math.py"]
        );
    }

    #[test]
    fn reads_an_index_file_and_reports_the_tool() {
        let (dir, ws, so_far, index) = fixture();
        let path = dir.path().join("index.scip");
        std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();
        let p = ScipImport::new("scip:index.scip".into(), path);
        let m = p.facts(&ws, &so_far).unwrap();
        assert_eq!(m.edges.len(), 4);
        assert_eq!(p.version(), "test-indexer 1.0");
        let bad = ScipImport::new("x".into(), dir.path().join("lib/math.py"));
        assert!(matches!(
            bad.facts(&ws, &so_far),
            Err(ProviderError::Failed(_))
        ));
    }
}
