//! Per-file extraction: everything Onus reads from one file's syntax tree,
//! before names are resolved across files.

use std::collections::{BTreeSet, HashMap, HashSet};

use onus_core::hash::short_hash;
use onus_core::secrets;
use onus_core::{
    BodyFacts, Comparison, Confidence, ContractShape, FactSite, MapDiagnostic, Member, MemberKind,
    Param, ShapeKind, SymbolKind, TestCase,
};
use tree_sitter::Node;

use crate::lang;
use crate::norm::{end_line, line, norm, norm_type, string_value, text};

/// What to look for, from `onus.yaml` and defaults.
#[derive(Debug, Clone, Default)]
pub struct Patterns {
    pub publish: Vec<CallPattern>,
    pub subscribe: Vec<CallPattern>,
    pub prisma_clients: Vec<String>,
}

/// A parsed call pattern such as `bus.publish($EVENT, ...)` or
/// `@OnEvent($EVENT)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPattern {
    pub callee: String,
    pub decorator: bool,
    pub event_arg: usize,
}

impl CallPattern {
    pub fn parse(pattern: &str) -> Option<CallPattern> {
        let pattern = pattern.trim();
        let (decorator, rest) = match pattern.strip_prefix('@') {
            Some(r) => (true, r),
            None => (false, pattern),
        };
        let (callee, args) = rest.split_once('(')?;
        let args = args.trim_end().strip_suffix(')')?;
        let event_arg = args.split(',').position(|a| a.trim() == "$EVENT")?;
        Some(CallPattern {
            callee: callee.trim().to_string(),
            decorator,
            event_arg,
        })
    }

    fn matches(&self, callee: &str) -> bool {
        let callee = callee.strip_prefix("this.").unwrap_or(callee);
        callee == self.callee || callee.ends_with(&format!(".{}", self.callee))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Imported {
    Named(String),
    Default,
    Namespace,
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub local: String,
    pub imported: Imported,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub spec: String,
    pub line: u32,
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Clone)]
pub enum Export {
    Local {
        exported: String,
        local: String,
        line: u32,
    },
    From {
        spec: String,
        exported: String,
        imported: Imported,
        line: u32,
    },
    Star {
        spec: String,
        line: u32,
    },
}

#[derive(Debug, Clone)]
pub struct Decl {
    /// Qualified name: `Name` or `Class.method`.
    pub name: String,
    pub kind: SymbolKind,
    pub shape: Option<ContractShape>,
    pub fingerprint: String,
    pub start: u32,
    pub end: u32,
    /// Last line of the signature for functions and methods.
    pub signature_end: Option<u32>,
    pub facts: Option<BodyFacts>,
    pub literal: Option<String>,
    /// Enum members and their literal values, for event-name lookup.
    pub enum_values: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefKind {
    Call,
    Type,
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ref {
    /// Enclosing declaration (qualified), or `None` at the top level.
    pub from: Option<String>,
    pub name: String,
    pub member: Option<String>,
    pub kind: RefKind,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventExpr {
    Literal(String),
    Ident(String),
    Member(String, String),
    Dynamic(String),
}

#[derive(Debug, Clone)]
pub struct EventUse {
    pub from: Option<String>,
    pub publish: bool,
    pub name: EventExpr,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct DataUse {
    pub from: Option<String>,
    pub model: String,
    pub write: bool,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct SiteUse {
    pub from: Option<String>,
    pub value: String,
    pub line: u32,
}

#[derive(Debug, Clone, Default)]
pub struct FileFacts {
    pub path: String,
    pub is_test: bool,
    pub lines: u32,
    pub content_hash: String,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    pub decls: Vec<Decl>,
    pub refs: Vec<Ref>,
    pub events: Vec<EventUse>,
    pub data: Vec<DataUse>,
    pub env: Vec<SiteUse>,
    pub hosts: Vec<SiteUse>,
    pub diagnostics: Vec<MapDiagnostic>,
    pub test_cases: Vec<TestCase>,
    pub literals: Vec<String>,
}

const PRISMA_READS: &[&str] = &[
    "findUnique",
    "findUniqueOrThrow",
    "findFirst",
    "findFirstOrThrow",
    "findMany",
    "count",
    "aggregate",
    "groupBy",
];
const PRISMA_WRITES: &[&str] = &[
    "create",
    "createMany",
    "createManyAndReturn",
    "update",
    "updateMany",
    "upsert",
    "delete",
    "deleteMany",
];
const COMPARISON_OPS: &[&str] = &["<", "<=", ">", ">=", "==", "===", "!=", "!=="];
const HTTP_CLIENTS: &[&str] = &["fetch", "got", "ky", "axios"];
const AXIOS_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "request",
];

pub fn extract(path: &str, src: &str, is_test: bool, patterns: &Patterns) -> FileFacts {
    let mut facts = FileFacts {
        path: path.to_string(),
        is_test,
        lines: count_lines(src),
        ..FileFacts::default()
    };
    let Some(tree) = lang::parse(path, src) else {
        facts
            .diagnostics
            .push(diag("parse-error", path, 1, "file could not be parsed"));
        return facts;
    };
    let root = tree.root_node();
    let bytes = src.as_bytes();
    if root.has_error() {
        let at = first_error(root).map(line).unwrap_or(1);
        facts.diagnostics.push(diag(
            "parse-error",
            path,
            at,
            "syntax error; facts from this file may be incomplete",
        ));
    }
    facts.content_hash = content_hash(root, bytes);

    let mut x = Extractor {
        path,
        src: bytes,
        patterns,
        facts: &mut facts,
    };
    if is_test {
        x.extract_test_file(root);
    } else {
        x.extract_module(root);
    }
    facts.refs.sort();
    facts.refs.dedup();
    facts.diagnostics.sort();
    facts.diagnostics.dedup();
    facts
}

fn count_lines(src: &str) -> u32 {
    if src.is_empty() {
        0
    } else {
        src.lines().count() as u32
    }
}

fn diag(kind: &str, file: &str, line: u32, message: &str) -> MapDiagnostic {
    MapDiagnostic {
        kind: kind.to_string(),
        file: file.to_string(),
        line,
        message: message.to_string(),
        confidence: Confidence::Low,
    }
}

fn first_error(node: Node) -> Option<Node> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.has_error() {
            if let Some(n) = first_error(child) {
                return Some(n);
            }
        }
    }
    None
}

fn children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn has_child_token(node: Node, token: &str) -> bool {
    children(node)
        .iter()
        .any(|c| !c.is_named() && c.kind() == token)
}

/// Hash of the file's tokens, ignoring formatting, comments and the text of
/// module specifiers (imports are compared separately, after resolution).
fn content_hash(root: Node, src: &[u8]) -> String {
    let mut out = String::new();
    hash_tokens(root, src, &mut out);
    short_hash(out.as_bytes())
}

fn hash_tokens(node: Node, src: &[u8], out: &mut String) {
    match node.kind() {
        "comment" => {}
        "string" if node.is_named() => {
            let is_source = node.parent().is_some_and(|p| {
                matches!(p.kind(), "import_statement" | "export_statement")
                    && p.child_by_field_name("source") == Some(node)
            });
            if is_source {
                out.push_str("<module>");
            } else {
                out.push('"');
                out.push_str(&string_value(node, src).unwrap_or_default());
                out.push('"');
            }
            out.push('\u{1f}');
        }
        _ if node.child_count() == 0 => {
            let t = text(node, src);
            if !matches!(t, ";" | ",") {
                out.push_str(t);
                out.push('\u{1f}');
            }
        }
        _ => {
            for child in children(node) {
                hash_tokens(child, src, out);
            }
        }
    }
}

/// Names bound inside `node`: parameters, local variables, nested functions
/// and classes, catch parameters and type parameters.
fn collect_bindings(node: Node, src: &[u8], out: &mut HashSet<String>) {
    match node.kind() {
        "variable_declarator" => {
            if let Some(name) = node.child_by_field_name("name") {
                collect_pattern(name, src, out);
            }
        }
        "required_parameter" | "optional_parameter" => {
            if let Some(p) = node.child_by_field_name("pattern") {
                collect_pattern(p, src, out);
            }
        }
        "arrow_function" => {
            if let Some(p) = node.child_by_field_name("parameter") {
                collect_pattern(p, src, out);
            }
        }
        "function_declaration"
        | "generator_function_declaration"
        | "class_declaration"
        | "function_expression" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.insert(text(n, src).to_string());
            }
        }
        "catch_clause" => {
            if let Some(p) = node.child_by_field_name("parameter") {
                collect_pattern(p, src, out);
            }
        }
        "for_in_statement" => {
            if let Some(l) = node.child_by_field_name("left") {
                collect_pattern(l, src, out);
            }
        }
        "type_parameter" => {
            if let Some(n) = node.child_by_field_name("name") {
                out.insert(text(n, src).to_string());
            }
        }
        _ => {}
    }
    for child in named_children(node) {
        collect_bindings(child, src, out);
    }
}

fn collect_pattern(node: Node, src: &[u8], out: &mut HashSet<String>) {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            out.insert(text(node, src).to_string());
        }
        "pair_pattern" => {
            if let Some(v) = node.child_by_field_name("value") {
                collect_pattern(v, src, out);
            }
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            if let Some(l) = node.child_by_field_name("left") {
                collect_pattern(l, src, out);
            }
        }
        _ => {
            for child in named_children(node) {
                collect_pattern(child, src, out);
            }
        }
    }
}

/// Fingerprint of a declaration: its tokens with comments, formatting, its
/// own name and the names of its locals normalized away.
fn fingerprint(node: Node, src: &[u8], own_name: Option<Node>, locals: &HashSet<String>) -> String {
    let mut out = String::new();
    let mut numbering: HashMap<String, usize> = HashMap::new();
    fp_tokens(node, src, own_name, locals, &mut numbering, &mut out);
    short_hash(out.as_bytes())
}

fn fp_tokens(
    node: Node,
    src: &[u8],
    own_name: Option<Node>,
    locals: &HashSet<String>,
    numbering: &mut HashMap<String, usize>,
    out: &mut String,
) {
    if Some(node) == own_name {
        out.push_str("$self\u{1f}");
        return;
    }
    match node.kind() {
        "comment" => {}
        "string" if node.is_named() => {
            out.push('"');
            out.push_str(&string_value(node, src).unwrap_or_default());
            out.push_str("\"\u{1f}");
        }
        "identifier"
        | "shorthand_property_identifier"
        | "shorthand_property_identifier_pattern"
            if locals.contains(text(node, src)) =>
        {
            let n = numbering.len();
            let idx = *numbering.entry(text(node, src).to_string()).or_insert(n);
            out.push_str(&format!("${idx}\u{1f}"));
        }
        _ if node.child_count() == 0 => {
            let t = text(node, src);
            if !matches!(t, ";" | ",") {
                out.push_str(t);
                out.push('\u{1f}');
            }
        }
        _ => {
            for child in children(node) {
                fp_tokens(child, src, own_name, locals, numbering, out);
            }
        }
    }
}

struct Extractor<'a> {
    path: &'a str,
    src: &'a [u8],
    patterns: &'a Patterns,
    facts: &'a mut FileFacts,
}

impl Extractor<'_> {
    fn t(&self, node: Node) -> String {
        text(node, self.src).to_string()
    }

    /// Redacts `value` if it, or the source line it comes from, contains a
    /// secret. Some secret patterns need the variable name for context.
    fn scrub(&self, value: String, node: Node) -> String {
        scrub(self.src, value, node)
    }

    // ---- modules ----

    fn extract_module(&mut self, root: Node) {
        let mut top_locals = HashSet::new();
        for child in named_children(root) {
            self.top_level(child, &mut top_locals);
        }
    }

    fn top_level(&mut self, node: Node, top_locals: &mut HashSet<String>) {
        match node.kind() {
            "import_statement" => self.import_statement(node),
            "export_statement" => self.export_statement(node),
            "ambient_declaration" => {
                for child in named_children(node) {
                    self.declaration(child, None);
                }
            }
            "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration"
            | "lexical_declaration"
            | "variable_declaration" => {
                self.declaration(node, None);
            }
            "comment" => {}
            _ => self.scan(node, None, top_locals),
        }
    }

    fn import_statement(&mut self, node: Node) {
        let Some(source) = node.child_by_field_name("source") else {
            return;
        };
        let spec = string_value(source, self.src).unwrap_or_default();
        let mut import = Import {
            spec,
            line: line(node),
            bindings: Vec::new(),
        };
        for child in named_children(node) {
            if child.kind() == "import_clause" {
                for part in named_children(child) {
                    match part.kind() {
                        "identifier" => import.bindings.push(Binding {
                            local: self.t(part),
                            imported: Imported::Default,
                            line: line(part),
                        }),
                        "namespace_import" => {
                            if let Some(id) = named_children(part)
                                .into_iter()
                                .find(|c| c.kind() == "identifier")
                            {
                                import.bindings.push(Binding {
                                    local: self.t(id),
                                    imported: Imported::Namespace,
                                    line: line(part),
                                });
                            }
                        }
                        "named_imports" => {
                            for spec in named_children(part) {
                                if spec.kind() != "import_specifier" {
                                    continue;
                                }
                                let Some(name) = spec.child_by_field_name("name") else {
                                    continue;
                                };
                                let local = spec
                                    .child_by_field_name("alias")
                                    .map(|a| self.t(a))
                                    .unwrap_or_else(|| self.t(name));
                                let imported = match self.t(name).as_str() {
                                    "default" => Imported::Default,
                                    n => Imported::Named(n.trim_matches('"').to_string()),
                                };
                                import.bindings.push(Binding {
                                    local,
                                    imported,
                                    line: line(spec),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        self.facts.imports.push(import);
    }

    fn export_statement(&mut self, node: Node) {
        let is_default = has_child_token(node, "default");
        let source = node
            .child_by_field_name("source")
            .and_then(|s| string_value(s, self.src));
        if let Some(decl) = node.child_by_field_name("declaration") {
            let names = self.declaration(decl, None);
            for name in names {
                let exported = if is_default {
                    "default".to_string()
                } else {
                    name.clone()
                };
                self.facts.exports.push(Export::Local {
                    exported,
                    local: name,
                    line: line(node),
                });
            }
            return;
        }
        let clause = named_children(node)
            .into_iter()
            .find(|c| c.kind() == "export_clause");
        if let Some(clause) = clause {
            for spec in named_children(clause) {
                if spec.kind() != "export_specifier" {
                    continue;
                }
                let Some(name) = spec.child_by_field_name("name") else {
                    continue;
                };
                let local = self.t(name);
                let exported = spec
                    .child_by_field_name("alias")
                    .map(|a| self.t(a))
                    .unwrap_or_else(|| local.clone());
                match &source {
                    Some(spec_text) => self.facts.exports.push(Export::From {
                        spec: spec_text.clone(),
                        exported,
                        imported: if local == "default" {
                            Imported::Default
                        } else {
                            Imported::Named(local)
                        },
                        line: line(spec),
                    }),
                    None => self.facts.exports.push(Export::Local {
                        exported,
                        local,
                        line: line(spec),
                    }),
                }
            }
            return;
        }
        if let Some(spec_text) = source {
            let ns = named_children(node)
                .into_iter()
                .find(|c| c.kind() == "namespace_export");
            match ns {
                Some(ns) => {
                    let exported = named_children(ns)
                        .first()
                        .map(|n| self.t(*n))
                        .unwrap_or_default();
                    self.facts.exports.push(Export::From {
                        spec: spec_text,
                        exported,
                        imported: Imported::Namespace,
                        line: line(node),
                    });
                }
                None => self.facts.exports.push(Export::Star {
                    spec: spec_text,
                    line: line(node),
                }),
            }
            return;
        }
        if is_default {
            if let Some(value) = node.child_by_field_name("value") {
                if value.kind() == "identifier" {
                    self.facts.exports.push(Export::Local {
                        exported: "default".into(),
                        local: self.t(value),
                        line: line(node),
                    });
                } else {
                    self.value_decl("default", None, Some(value), node, node);
                    self.facts.exports.push(Export::Local {
                        exported: "default".into(),
                        local: "default".into(),
                        line: line(node),
                    });
                }
            }
        }
    }

    /// Records a top-level declaration and returns the names it declares.
    fn declaration(&mut self, node: Node, _prefix: Option<&str>) -> Vec<String> {
        match node.kind() {
            "function_declaration" | "generator_function_declaration" | "function_signature" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return vec![];
                };
                let name = self.t(name_node);
                self.function_like(&name, SymbolKind::Function, node, Some(name_node), node);
                vec![name]
            }
            "class_declaration" | "abstract_class_declaration" | "class" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return vec![];
                };
                let name = self.t(name_node);
                self.class(&name, node, name_node);
                vec![name]
            }
            "interface_declaration" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return vec![];
                };
                let name = self.t(name_node);
                let mut shape = ContractShape {
                    kind: ShapeKind::Object,
                    type_params: node
                        .child_by_field_name("type_parameters")
                        .map(|t| norm_type(t, self.src)),
                    params: vec![],
                    returns: None,
                    members: vec![],
                    type_text: named_children(node)
                        .into_iter()
                        .find(|c| c.kind() == "extends_type_clause")
                        .map(|c| norm_type(c, self.src)),
                    unverified: false,
                };
                if let Some(body) = node.child_by_field_name("body") {
                    shape.members = self.object_members(body);
                }
                self.type_decl(&name, SymbolKind::Interface, node, name_node, shape);
                vec![name]
            }
            "type_alias_declaration" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return vec![];
                };
                let name = self.t(name_node);
                let value = node.child_by_field_name("value");
                let shape = match value {
                    Some(v) if v.kind() == "object_type" => ContractShape {
                        kind: ShapeKind::Object,
                        type_params: node
                            .child_by_field_name("type_parameters")
                            .map(|t| norm_type(t, self.src)),
                        params: vec![],
                        returns: None,
                        members: self.object_members(v),
                        type_text: None,
                        unverified: false,
                    },
                    Some(v) => ContractShape {
                        kind: ShapeKind::Alias,
                        type_params: node
                            .child_by_field_name("type_parameters")
                            .map(|t| norm_type(t, self.src)),
                        params: vec![],
                        returns: None,
                        members: vec![],
                        type_text: Some(norm_type(v, self.src)),
                        unverified: false,
                    },
                    None => ContractShape {
                        kind: ShapeKind::Alias,
                        type_params: None,
                        params: vec![],
                        returns: None,
                        members: vec![],
                        type_text: None,
                        unverified: true,
                    },
                };
                self.type_decl(&name, SymbolKind::Type, node, name_node, shape);
                vec![name]
            }
            "enum_declaration" => {
                let Some(name_node) = node.child_by_field_name("name") else {
                    return vec![];
                };
                let name = self.t(name_node);
                let mut members = Vec::new();
                let mut values = Vec::new();
                if let Some(body) = node.child_by_field_name("body") {
                    for m in named_children(body) {
                        let (mname, value) = match m.kind() {
                            "property_identifier" => (self.t(m), None),
                            "enum_assignment" => (
                                m.child_by_field_name("name")
                                    .map(|n| self.t(n))
                                    .unwrap_or_default(),
                                m.child_by_field_name("value"),
                            ),
                            _ => continue,
                        };
                        let mname = mname.trim_matches(|c| c == '"' || c == '\'').to_string();
                        let literal = value
                            .and_then(|v| string_value(v, self.src))
                            .unwrap_or_else(|| mname.clone());
                        values.push((mname.clone(), literal));
                        members.push(Member {
                            name: mname,
                            kind: MemberKind::EnumMember,
                            type_text: value.map(|v| norm(v, self.src)),
                            optional: false,
                            readonly: false,
                            line: line(m),
                        });
                    }
                }
                let shape = ContractShape {
                    kind: ShapeKind::Enum,
                    type_params: None,
                    params: vec![],
                    returns: None,
                    members,
                    type_text: None,
                    unverified: false,
                };
                self.type_decl(&name, SymbolKind::Enum, node, name_node, shape);
                if let Some(d) = self.facts.decls.last_mut() {
                    d.enum_values = values;
                }
                vec![name]
            }
            "lexical_declaration" | "variable_declaration" => {
                let mut names = Vec::new();
                for declarator in named_children(node) {
                    if declarator.kind() != "variable_declarator" {
                        continue;
                    }
                    let Some(name_node) = declarator.child_by_field_name("name") else {
                        continue;
                    };
                    if name_node.kind() != "identifier" {
                        // Destructuring at the top level: record the names as
                        // unverified consts and scan the initializer.
                        let mut bound = HashSet::new();
                        collect_pattern(name_node, self.src, &mut bound);
                        let mut bound: Vec<String> = bound.into_iter().collect();
                        bound.sort();
                        if let Some(value) = declarator.child_by_field_name("value") {
                            self.env_destructure(name_node, value, None);
                            self.scan(value, None, &HashSet::new());
                        }
                        for b in bound {
                            self.facts.decls.push(Decl {
                                name: b.clone(),
                                kind: SymbolKind::Const,
                                shape: Some(unverified_value()),
                                fingerprint: short_hash(norm(declarator, self.src).as_bytes()),
                                start: line(node),
                                end: end_line(node),
                                signature_end: None,
                                facts: None,
                                literal: None,
                                enum_values: vec![],
                            });
                            names.push(b);
                        }
                        continue;
                    }
                    let name = self.t(name_node);
                    let value = declarator.child_by_field_name("value");
                    let ty = declarator.child_by_field_name("type");
                    self.value_decl(&name, ty, value, declarator, node);
                    names.push(name);
                }
                names
            }
            _ => vec![],
        }
    }

    /// A `const`/`let` declaration, or an `export default <expression>`.
    fn value_decl(
        &mut self,
        name: &str,
        ty: Option<Node>,
        value: Option<Node>,
        declarator: Node,
        stmt: Node,
    ) {
        if let Some(v) = value {
            if matches!(
                v.kind(),
                "arrow_function" | "function_expression" | "function"
            ) {
                let name_node = declarator.child_by_field_name("name");
                self.function_like(name, SymbolKind::Function, v, name_node, stmt);
                return;
            }
            if matches!(v.kind(), "class") {
                if let Some(name_node) = declarator.child_by_field_name("name") {
                    self.class(name, v, name_node);
                    return;
                }
            }
        }
        let literal = value.and_then(|v| string_value(v, self.src));
        let shape = match (ty, value) {
            (Some(t), _) => ContractShape {
                kind: ShapeKind::Value,
                type_params: None,
                params: vec![],
                returns: None,
                members: vec![],
                type_text: Some(strip_colon(norm_type(t, self.src))),
                unverified: false,
            },
            (None, Some(v)) => match literal_type(v) {
                Some(t) => ContractShape {
                    kind: ShapeKind::Value,
                    type_params: None,
                    params: vec![],
                    returns: None,
                    members: vec![],
                    type_text: Some(t.to_string()),
                    unverified: false,
                },
                None => unverified_value(),
            },
            (None, None) => unverified_value(),
        };
        let mut locals = HashSet::new();
        collect_bindings(declarator, self.src, &mut locals);
        locals.remove(name);
        let own = declarator.child_by_field_name("name");
        let fp = fingerprint(value.unwrap_or(declarator), self.src, own, &locals);
        if let (Some(n), Some(v)) = (own, value) {
            self.env_destructure(n, v, Some(name));
        }
        if let Some(t) = ty {
            self.scan(t, Some(name), &locals);
        }
        if let Some(v) = value {
            self.scan(v, Some(name), &locals);
        }
        self.facts.decls.push(Decl {
            name: name.to_string(),
            kind: SymbolKind::Const,
            shape: Some(shape),
            fingerprint: fp,
            start: line(stmt),
            end: end_line(stmt),
            signature_end: None,
            facts: None,
            literal: literal.map(|l| self.scrub(l, declarator)),
            enum_values: vec![],
        });
    }

    fn function_like(
        &mut self,
        name: &str,
        kind: SymbolKind,
        node: Node,
        name_node: Option<Node>,
        span: Node,
    ) {
        let shape = self.function_shape(node);
        let mut locals = HashSet::new();
        collect_bindings(node, self.src, &mut locals);
        locals.remove(name);
        let fp = fingerprint(node, self.src, name_node, &locals);
        let facts = self.body_facts(node);
        self.scan_children_except(node, name_node, Some(name), &locals);
        self.facts.decls.push(Decl {
            name: name.to_string(),
            kind,
            shape: Some(shape),
            fingerprint: fp,
            start: line(span),
            end: end_line(span),
            signature_end: signature_end(node),
            facts: (!facts.is_empty()).then_some(facts),
            literal: None,
            enum_values: vec![],
        });
    }

    fn function_shape(&self, node: Node) -> ContractShape {
        let mut params = Vec::new();
        let mut unverified = false;
        if let Some(ps) = node.child_by_field_name("parameters") {
            for p in named_children(ps) {
                match p.kind() {
                    "required_parameter" | "optional_parameter" => {
                        let pattern = p.child_by_field_name("pattern");
                        let rest = pattern.is_some_and(|pt| pt.kind() == "rest_pattern");
                        let name = pattern
                            .map(|pt| norm(pt, self.src))
                            .unwrap_or_default()
                            .trim_start_matches("...")
                            .to_string();
                        if name == "this" {
                            continue;
                        }
                        let type_text = p
                            .child_by_field_name("type")
                            .map(|t| strip_colon(norm_type(t, self.src)));
                        let optional = p.kind() == "optional_parameter"
                            || p.child_by_field_name("value").is_some();
                        if type_text.is_none() {
                            unverified = true;
                        }
                        params.push(Param {
                            name,
                            type_text,
                            optional,
                            rest,
                        });
                    }
                    _ => {}
                }
            }
        } else if let Some(p) = node.child_by_field_name("parameter") {
            params.push(Param {
                name: self.t(p),
                type_text: None,
                optional: false,
                rest: false,
            });
            unverified = true;
        }
        let returns = node
            .child_by_field_name("return_type")
            .map(|r| strip_colon(norm_type(r, self.src)));
        if returns.is_none() {
            unverified = true;
        }
        ContractShape {
            kind: ShapeKind::Function,
            type_params: node
                .child_by_field_name("type_parameters")
                .map(|t| norm_type(t, self.src)),
            params,
            returns,
            members: vec![],
            type_text: None,
            unverified,
        }
    }

    fn class(&mut self, name: &str, node: Node, name_node: Node) {
        let mut members = Vec::new();
        let mut locals = HashSet::new();
        if let Some(tp) = node.child_by_field_name("type_parameters") {
            collect_bindings(tp, self.src, &mut locals);
        }
        let heritage = named_children(node)
            .into_iter()
            .find(|c| c.kind() == "class_heritage")
            .map(|h| norm_type(h, self.src));
        if let Some(h) = named_children(node)
            .into_iter()
            .find(|c| c.kind() == "class_heritage")
        {
            self.scan(h, Some(name), &locals);
        }
        let mut unverified = false;
        if let Some(body) = node.child_by_field_name("body") {
            for m in named_children(body) {
                let private = named_children(m).iter().any(|c| {
                    c.kind() == "accessibility_modifier"
                        && matches!(text(*c, self.src), "private" | "protected")
                }) || m
                    .child_by_field_name("name")
                    .is_some_and(|n| n.kind() == "private_property_identifier");
                match m.kind() {
                    "method_definition" | "method_signature" | "abstract_method_signature" => {
                        let Some(mname_node) = m.child_by_field_name("name") else {
                            continue;
                        };
                        let mname = self.t(mname_node);
                        let qualified = format!("{name}.{mname}");
                        let shape = self.function_shape(m);
                        if !private {
                            if shape.unverified {
                                unverified = true;
                            }
                            members.push(Member {
                                name: mname.clone(),
                                kind: if mname == "constructor" {
                                    MemberKind::Constructor
                                } else {
                                    MemberKind::Method
                                },
                                type_text: Some(signature_text(&shape)),
                                optional: has_child_token(m, "?"),
                                readonly: false,
                                line: line(m),
                            });
                        }
                        if mname == "constructor" {
                            let mut l = locals.clone();
                            collect_bindings(m, self.src, &mut l);
                            self.scan_children_except(m, Some(mname_node), Some(name), &l);
                            continue;
                        }
                        let mut l = locals.clone();
                        collect_bindings(m, self.src, &mut l);
                        let fp = fingerprint(m, self.src, None, &l);
                        let facts = self.body_facts(m);
                        self.scan_children_except(m, Some(mname_node), Some(&qualified), &l);
                        self.facts.decls.push(Decl {
                            name: qualified,
                            kind: SymbolKind::Method,
                            shape: Some(shape),
                            fingerprint: fp,
                            start: line(m),
                            end: end_line(m),
                            signature_end: signature_end(m),
                            facts: (!facts.is_empty()).then_some(facts),
                            literal: None,
                            enum_values: vec![],
                        });
                    }
                    "public_field_definition" | "property_signature" => {
                        let Some(fname) = m.child_by_field_name("name") else {
                            continue;
                        };
                        let ty = m
                            .child_by_field_name("type")
                            .map(|t| strip_colon(norm_type(t, self.src)));
                        if !private {
                            if ty.is_none() {
                                unverified = true;
                            }
                            members.push(Member {
                                name: self.t(fname),
                                kind: MemberKind::Property,
                                type_text: ty,
                                optional: has_child_token(m, "?"),
                                readonly: named_children(m).iter().any(|c| c.kind() == "readonly")
                                    || has_child_token(m, "readonly"),
                                line: line(m),
                            });
                        }
                        let mut l = locals.clone();
                        collect_bindings(m, self.src, &mut l);
                        self.scan_children_except(m, Some(fname), Some(name), &l);
                    }
                    _ => {
                        self.scan(m, Some(name), &locals);
                    }
                }
            }
        }
        let mut all_locals = HashSet::new();
        collect_bindings(node, self.src, &mut all_locals);
        all_locals.remove(name);
        let fp = fingerprint(node, self.src, Some(name_node), &all_locals);
        self.facts.decls.push(Decl {
            name: name.to_string(),
            kind: SymbolKind::Class,
            shape: Some(ContractShape {
                kind: ShapeKind::Class,
                type_params: node
                    .child_by_field_name("type_parameters")
                    .map(|t| norm_type(t, self.src)),
                params: vec![],
                returns: None,
                members,
                type_text: heritage,
                unverified,
            }),
            fingerprint: fp,
            start: line(node),
            end: end_line(node),
            signature_end: None,
            facts: None,
            literal: None,
            enum_values: vec![],
        });
    }

    fn type_decl(
        &mut self,
        name: &str,
        kind: SymbolKind,
        node: Node,
        name_node: Node,
        shape: ContractShape,
    ) {
        let mut locals = HashSet::new();
        collect_bindings(node, self.src, &mut locals);
        let fp = fingerprint(node, self.src, Some(name_node), &locals);
        self.scan_children_except(node, Some(name_node), Some(name), &locals);
        self.facts.decls.push(Decl {
            name: name.to_string(),
            kind,
            shape: Some(shape),
            fingerprint: fp,
            start: line(node),
            end: end_line(node),
            signature_end: None,
            facts: None,
            literal: None,
            enum_values: vec![],
        });
    }

    fn object_members(&self, body: Node) -> Vec<Member> {
        let mut out = Vec::new();
        for m in named_children(body) {
            match m.kind() {
                "property_signature" => {
                    let Some(n) = m.child_by_field_name("name") else {
                        continue;
                    };
                    out.push(Member {
                        name: self
                            .t(n)
                            .trim_matches(|c| c == '"' || c == '\'')
                            .to_string(),
                        kind: MemberKind::Property,
                        type_text: m
                            .child_by_field_name("type")
                            .map(|t| strip_colon(norm_type(t, self.src))),
                        optional: has_child_token(m, "?"),
                        readonly: has_child_token(m, "readonly")
                            || named_children(m).iter().any(|c| c.kind() == "readonly"),
                        line: line(m),
                    });
                }
                "method_signature" => {
                    let Some(n) = m.child_by_field_name("name") else {
                        continue;
                    };
                    let shape = self.function_shape(m);
                    out.push(Member {
                        name: self.t(n),
                        kind: MemberKind::Method,
                        type_text: Some(signature_text(&shape)),
                        optional: has_child_token(m, "?"),
                        readonly: false,
                        line: line(m),
                    });
                }
                "call_signature" | "construct_signature" | "index_signature" => {
                    out.push(Member {
                        name: format!("[{}]", m.kind().trim_end_matches("_signature")),
                        kind: MemberKind::Method,
                        type_text: Some(norm_type(m, self.src)),
                        optional: false,
                        readonly: false,
                        line: line(m),
                    });
                }
                _ => {}
            }
        }
        out
    }

    // ---- body facts ----

    fn body_facts(&self, node: Node) -> BodyFacts {
        let mut facts = BodyFacts::default();
        if let Some(body) = node.child_by_field_name("body") {
            self.collect_facts(body, &mut facts);
        }
        facts
    }

    fn collect_facts(&self, node: Node, facts: &mut BodyFacts) {
        match node.kind() {
            "binary_expression" => {
                if let (Some(op), Some(l), Some(r)) = (
                    node.child_by_field_name("operator"),
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) {
                    let op = self.t(op);
                    if COMPARISON_OPS.contains(&op.as_str()) {
                        facts.comparisons.push(Comparison {
                            op,
                            left: self.scrub(norm(l, self.src), node),
                            right: self.scrub(norm(r, self.src), node),
                            line: line(node),
                        });
                    }
                }
            }
            "throw_statement" => facts.throws.push(FactSite {
                text: self.scrub(norm(node, self.src), node),
                line: line(node),
            }),
            "await_expression" => {
                let inner = named_children(node).into_iter().next();
                let text = match inner {
                    Some(c) if c.kind() == "call_expression" => c
                        .child_by_field_name("function")
                        .map(|f| norm(f, self.src))
                        .unwrap_or_default(),
                    Some(c) => truncate(&norm(c, self.src), 80),
                    None => String::new(),
                };
                facts.awaits.push(FactSite {
                    text: self.scrub(text, node),
                    line: line(node),
                });
            }
            "if_statement" => {
                if node.child_by_field_name("alternative").is_none() {
                    if let Some(cons) = node.child_by_field_name("consequence") {
                        if exits(cons) {
                            let cond = node
                                .child_by_field_name("condition")
                                .map(|c| norm(c, self.src))
                                .unwrap_or_default();
                            facts.guards.push(FactSite {
                                text: self.scrub(cond, node),
                                line: line(node),
                            });
                        }
                    }
                }
            }
            "catch_clause" => {
                if let Some(body) = node.child_by_field_name("body") {
                    if named_children(body).iter().all(|c| c.kind() == "comment") {
                        facts.empty_catches.push(FactSite {
                            text: "catch {}".into(),
                            line: line(node),
                        });
                    }
                }
            }
            _ => {}
        }
        for child in named_children(node) {
            self.collect_facts(child, facts);
        }
    }

    // ---- references and extractors ----

    fn scan_children_except(
        &mut self,
        node: Node,
        skip: Option<Node>,
        from: Option<&str>,
        locals: &HashSet<String>,
    ) {
        for child in named_children(node) {
            if Some(child) == skip {
                continue;
            }
            self.scan(child, from, locals);
        }
    }

    fn push_ref(
        &mut self,
        from: Option<&str>,
        name: String,
        member: Option<String>,
        kind: RefKind,
        at: Node,
    ) {
        self.facts.refs.push(Ref {
            from: from.map(str::to_string),
            name,
            member,
            kind,
            line: line(at),
        });
    }

    /// Walks `node`, recording references, events, data access, config reads,
    /// outbound hosts and dynamic code.
    fn scan(&mut self, node: Node, from: Option<&str>, locals: &HashSet<String>) {
        match node.kind() {
            "comment" => return,
            "call_expression" => self.call(node, from, locals),
            "new_expression" => {
                if let Some(c) = node.child_by_field_name("constructor") {
                    if let Some((root, member)) = root_of(c, self.src) {
                        if !locals.contains(&root) {
                            self.push_ref(from, root, member, RefKind::Call, node);
                        }
                    }
                }
            }
            "decorator" => self.decorator(node, from),
            "type_identifier" => {
                if !is_declaration_name(node) {
                    let name = self.t(node);
                    if !locals.contains(&name) {
                        self.push_ref(from, name, None, RefKind::Type, node);
                    }
                }
                return;
            }
            "nested_type_identifier" => {
                let parts: Vec<String> = named_children(node).iter().map(|c| self.t(*c)).collect();
                if parts.len() == 2 && !locals.contains(&parts[0]) {
                    self.push_ref(
                        from,
                        parts[0].clone(),
                        Some(parts[1].clone()),
                        RefKind::Type,
                        node,
                    );
                }
                return;
            }
            "identifier" | "shorthand_property_identifier" => {
                if !is_binding_position(node) {
                    let name = self.t(node);
                    if !locals.contains(&name) {
                        self.push_ref(from, name, None, RefKind::Value, node);
                    }
                }
                return;
            }
            "member_expression" => {
                if let Some(key) = env_key(node, self.src) {
                    self.facts.env.push(SiteUse {
                        from: from.map(str::to_string),
                        value: key,
                        line: line(node),
                    });
                    return;
                }
                if let (Some(obj), Some(prop)) = (
                    node.child_by_field_name("object"),
                    node.child_by_field_name("property"),
                ) {
                    if obj.kind() == "identifier" && !is_call_function(node) {
                        let name = self.t(obj);
                        if !locals.contains(&name) {
                            self.push_ref(from, name, Some(self.t(prop)), RefKind::Value, node);
                        }
                        return;
                    }
                }
            }
            "subscript_expression" => {
                if let Some(obj) = node.child_by_field_name("object") {
                    if norm(obj, self.src) == "process.env" {
                        let idx = node.child_by_field_name("index");
                        match idx.and_then(|i| string_value(i, self.src)) {
                            Some(key) => self.facts.env.push(SiteUse {
                                from: from.map(str::to_string),
                                value: key,
                                line: line(node),
                            }),
                            None => self.facts.diagnostics.push(diag(
                                "dynamic-config-key",
                                self.path,
                                line(node),
                                "process.env is read with a computed key",
                            )),
                        }
                        return;
                    }
                }
            }
            _ => {}
        }
        for child in named_children(node) {
            self.scan(child, from, locals);
        }
    }

    fn call(&mut self, node: Node, from: Option<&str>, locals: &HashSet<String>) {
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
        let args: Vec<Node> = node
            .child_by_field_name("arguments")
            .map(|a| {
                named_children(a)
                    .into_iter()
                    .filter(|c| c.kind() != "comment")
                    .collect()
            })
            .unwrap_or_default();
        let callee = norm(func, self.src);

        // Dynamic imports and require.
        if func.kind() == "import" || (func.kind() == "identifier" && callee == "require") {
            match args.first().and_then(|a| string_value(*a, self.src)) {
                Some(spec) => self.facts.imports.push(Import {
                    spec,
                    line: line(node),
                    bindings: vec![],
                }),
                None => self.facts.diagnostics.push(diag(
                    "dynamic-import",
                    self.path,
                    line(node),
                    &format!("`{callee}` with a computed module path"),
                )),
            }
            return;
        }

        if func.kind() == "subscript_expression" {
            let literal_index = func
                .child_by_field_name("index")
                .is_some_and(|i| matches!(i.kind(), "string" | "number"));
            if !literal_index {
                self.facts.diagnostics.push(diag(
                    "dynamic-access",
                    self.path,
                    line(node),
                    "call through a computed member; the target is unknown",
                ));
            }
        }

        // Events.
        for (publish, patterns) in [
            (true, &self.patterns.publish),
            (false, &self.patterns.subscribe),
        ] {
            for p in patterns.iter().filter(|p| !p.decorator) {
                if p.matches(&callee) {
                    let name = args
                        .get(p.event_arg)
                        .map(|a| self.event_expr(*a))
                        .unwrap_or_else(|| EventExpr::Dynamic(String::new()));
                    self.facts.events.push(EventUse {
                        from: from.map(str::to_string),
                        publish,
                        name,
                        line: line(node),
                    });
                }
            }
        }

        // Prisma: <client>.<model>.<operation>(...)
        if func.kind() == "member_expression" {
            if let (Some(obj), Some(op)) = (
                func.child_by_field_name("object"),
                func.child_by_field_name("property"),
            ) {
                let op = self.t(op);
                if obj.kind() == "member_expression" {
                    if let (Some(client), Some(model)) = (
                        obj.child_by_field_name("object"),
                        obj.child_by_field_name("property"),
                    ) {
                        let client_text = norm(client, self.src);
                        let client_last = client_text.rsplit('.').next().unwrap_or("");
                        if self
                            .patterns
                            .prisma_clients
                            .iter()
                            .any(|c| c == client_last)
                        {
                            let model = self.t(model);
                            let write = PRISMA_WRITES.contains(&op.as_str());
                            if write || PRISMA_READS.contains(&op.as_str()) {
                                self.facts.data.push(DataUse {
                                    from: from.map(str::to_string),
                                    model,
                                    write,
                                    line: line(node),
                                });
                            }
                        }
                    }
                } else {
                    let client_last = callee.split('.').rev().nth(1).unwrap_or("");
                    if self
                        .patterns
                        .prisma_clients
                        .iter()
                        .any(|c| c == client_last)
                        && matches!(
                            op.as_str(),
                            "$queryRaw" | "$executeRaw" | "$queryRawUnsafe" | "$executeRawUnsafe"
                        )
                    {
                        self.facts.diagnostics.push(diag(
                            "raw-query",
                            self.path,
                            line(node),
                            "raw SQL query; the tables it reads or writes are unknown",
                        ));
                    }
                }
            }
        }

        // Outbound HTTP with a literal host.
        let root_name = callee.split('.').next().unwrap_or("");
        let is_http = (HTTP_CLIENTS.contains(&callee.as_str()))
            || (root_name == "axios"
                && callee
                    .split('.')
                    .nth(1)
                    .is_some_and(|m| AXIOS_METHODS.contains(&m)));
        if is_http {
            if let Some(host) = args.first().and_then(|a| self.literal_host(*a)) {
                self.facts.hosts.push(SiteUse {
                    from: from.map(str::to_string),
                    value: host,
                    line: line(node),
                });
            }
        }

        // The call itself.
        if let Some((root, member)) = root_of(func, self.src) {
            if !locals.contains(&root) {
                self.push_ref(from, root, member, RefKind::Call, node);
            }
        }
        // Recurse into the callee's inner parts (for chained calls) and the
        // arguments.
        match func.kind() {
            "identifier" => {}
            "member_expression" => {
                if let Some(obj) = func.child_by_field_name("object") {
                    if obj.kind() != "identifier" {
                        self.scan(obj, from, locals);
                    }
                }
            }
            _ => self.scan(func, from, locals),
        }
        if let Some(a) = node.child_by_field_name("arguments") {
            self.scan(a, from, locals);
        }
    }

    fn decorator(&mut self, node: Node, from: Option<&str>) {
        let Some(call) = named_children(node)
            .into_iter()
            .find(|c| c.kind() == "call_expression")
        else {
            return;
        };
        let Some(func) = call.child_by_field_name("function") else {
            return;
        };
        let callee = norm(func, self.src);
        let args: Vec<Node> = call
            .child_by_field_name("arguments")
            .map(named_children)
            .unwrap_or_default();
        for (publish, patterns) in [
            (true, &self.patterns.publish),
            (false, &self.patterns.subscribe),
        ] {
            for p in patterns.iter().filter(|p| p.decorator) {
                if p.matches(&callee) {
                    let name = args
                        .get(p.event_arg)
                        .map(|a| self.event_expr(*a))
                        .unwrap_or_else(|| EventExpr::Dynamic(String::new()));
                    self.facts.events.push(EventUse {
                        from: from.map(str::to_string),
                        publish,
                        name,
                        line: line(node),
                    });
                }
            }
        }
    }

    fn event_expr(&self, arg: Node) -> EventExpr {
        if let Some(s) = string_value(arg, self.src) {
            return EventExpr::Literal(s);
        }
        match arg.kind() {
            "identifier" => EventExpr::Ident(self.t(arg)),
            "member_expression" => {
                match (
                    arg.child_by_field_name("object"),
                    arg.child_by_field_name("property"),
                ) {
                    (Some(o), Some(p)) if o.kind() == "identifier" => {
                        EventExpr::Member(self.t(o), self.t(p))
                    }
                    _ => EventExpr::Dynamic(norm(arg, self.src)),
                }
            }
            _ => EventExpr::Dynamic(norm(arg, self.src)),
        }
    }

    fn literal_host(&self, arg: Node) -> Option<String> {
        let raw = match arg.kind() {
            "string" => string_value(arg, self.src)?,
            "template_string" => {
                // The literal prefix before the first substitution.
                let full = self.t(arg);
                let inner = &full[1..];
                inner.split("${").next()?.to_string()
            }
            _ => return None,
        };
        let rest = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))?;
        let host = rest.split(['/', '?', '#']).next()?;
        let host = host.split('@').next_back()?.split(':').next()?;
        (!host.is_empty() && host.contains('.')).then(|| host.to_ascii_lowercase())
    }

    fn env_destructure(&mut self, pattern: Node, value: Node, from: Option<&str>) {
        if pattern.kind() != "object_pattern" || norm(value, self.src) != "process.env" {
            return;
        }
        for child in named_children(pattern) {
            let key = match child.kind() {
                "shorthand_property_identifier_pattern" => Some(self.t(child)),
                "pair_pattern" => child.child_by_field_name("key").map(|k| self.t(k)),
                "object_assignment_pattern" => child.child_by_field_name("left").map(|k| self.t(k)),
                _ => None,
            };
            if let Some(key) = key {
                self.facts.env.push(SiteUse {
                    from: from.map(str::to_string),
                    value: key,
                    line: line(child),
                });
            }
        }
    }

    // ---- test files ----

    fn extract_test_file(&mut self, root: Node) {
        for child in named_children(root) {
            if child.kind() == "import_statement" {
                self.import_statement(child);
            }
        }
        let mut locals = HashSet::new();
        collect_bindings(root, self.src, &mut locals);
        for child in named_children(root) {
            if child.kind() != "import_statement" {
                self.scan(child, None, &locals);
            }
        }
        let mut cases = Vec::new();
        self.collect_cases(root, "", &[], &mut cases);
        self.facts.test_cases = cases;
        let mut literals = BTreeSet::new();
        collect_literals(root, self.src, &mut literals);
        self.facts.literals = literals.into_iter().collect();
    }

    fn collect_cases(
        &self,
        node: Node,
        prefix: &str,
        inherited: &[String],
        out: &mut Vec<TestCase>,
    ) {
        if node.kind() == "call_expression" {
            if let Some((base, mods)) = self.test_callee(node) {
                let args: Vec<Node> = node
                    .child_by_field_name("arguments")
                    .map(|a| {
                        named_children(a)
                            .into_iter()
                            .filter(|c| c.kind() != "comment")
                            .collect()
                    })
                    .unwrap_or_default();
                let title = args
                    .first()
                    .map(|a| string_value(*a, self.src).unwrap_or_else(|| norm(*a, self.src)))
                    .unwrap_or_default();
                let name = if prefix.is_empty() {
                    title
                } else {
                    format!("{prefix} › {title}")
                };
                let mut markers: Vec<String> = inherited.to_vec();
                for m in mods {
                    if !markers.contains(&m) {
                        markers.push(m);
                    }
                }
                markers.sort();
                let body = args.get(1).copied().filter(|b| {
                    matches!(
                        b.kind(),
                        "arrow_function" | "function_expression" | "function"
                    )
                });
                if base == "describe" {
                    if let Some(b) = body {
                        self.collect_cases(b, &name, &markers, out);
                    }
                    return;
                }
                let mut assertions = Vec::new();
                let mut expected = Vec::new();
                if let Some(b) = body {
                    self.collect_assertions(b, &mut assertions, &mut expected);
                }
                let fingerprint = body
                    .map(|b| {
                        let mut l = HashSet::new();
                        collect_bindings(b, self.src, &mut l);
                        fingerprint(b, self.src, None, &l)
                    })
                    .unwrap_or_default();
                if body.is_none() && !markers.iter().any(|m| m == "todo") {
                    markers.push("todo".into());
                    markers.sort();
                }
                out.push(TestCase {
                    name: secrets::redact(&name),
                    line: line(node),
                    end_line: end_line(node),
                    assertions,
                    markers,
                    expected,
                    fingerprint,
                });
                return;
            }
        }
        for child in named_children(node) {
            self.collect_cases(child, prefix, inherited, out);
        }
    }

    /// `it`, `test.skip`, `xit`, `describe.only`, `it.each(...)` → base and
    /// markers.
    fn test_callee(&self, call: Node) -> Option<(String, Vec<String>)> {
        let mut func = call.child_by_field_name("function")?;
        if func.kind() == "call_expression" {
            // it.each(table)("name", fn)
            func = func.child_by_field_name("function")?;
        }
        let text = norm(func, self.src);
        let mut parts = text.split('.');
        let head = parts.next()?;
        let (base, mut markers) = match head {
            "it" | "test" => ("case", vec![]),
            "xit" | "xtest" => ("case", vec!["skip".to_string()]),
            "fit" => ("case", vec!["only".to_string()]),
            "describe" | "suite" | "context" => ("describe", vec![]),
            "xdescribe" => ("describe", vec!["skip".to_string()]),
            "fdescribe" => ("describe", vec!["only".to_string()]),
            _ => return None,
        };
        for p in parts {
            match p {
                "skip" | "skipIf" => markers.push("skip".into()),
                "only" => markers.push("only".into()),
                "todo" => markers.push("todo".into()),
                "each" | "concurrent" | "sequential" | "fails" | "runIf" => {}
                _ => return None,
            }
        }
        Some((base.to_string(), markers))
    }

    fn collect_assertions(
        &self,
        node: Node,
        assertions: &mut Vec<FactSite>,
        expected: &mut Vec<FactSite>,
    ) {
        if node.kind() == "call_expression" {
            if let Some(func) = node.child_by_field_name("function") {
                let callee = norm(func, self.src);
                let is_expect = func.kind() == "identifier" && callee == "expect";
                let is_assert = callee == "assert"
                    || (func.kind() == "member_expression"
                        && func
                            .child_by_field_name("object")
                            .is_some_and(|o| text(o, self.src) == "assert"));
                if is_expect || is_assert {
                    // The whole chain: expect(x).not.toBe(y)
                    let mut top = node;
                    while let Some(p) = top.parent() {
                        let continues = (p.kind() == "member_expression"
                            && p.child_by_field_name("object") == Some(top))
                            || (p.kind() == "call_expression"
                                && p.child_by_field_name("function") == Some(top));
                        if !continues {
                            break;
                        }
                        top = p;
                    }
                    assertions.push(FactSite {
                        text: self.scrub(norm(top, self.src), top),
                        line: line(node),
                    });
                    if is_expect && top.kind() == "call_expression" && top != node {
                        let matcher = top
                            .child_by_field_name("function")
                            .and_then(|f| f.child_by_field_name("property"))
                            .map(|p| self.t(p))
                            .unwrap_or_default();
                        if let Some(args) = top.child_by_field_name("arguments") {
                            for a in named_children(args) {
                                if matches!(
                                    a.kind(),
                                    "number"
                                        | "string"
                                        | "true"
                                        | "false"
                                        | "null"
                                        | "template_string"
                                ) {
                                    expected.push(FactSite {
                                        text: self
                                            .scrub(format!("{matcher}:{}", norm(a, self.src)), a),
                                        line: line(a),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        for child in named_children(node) {
            self.collect_assertions(child, assertions, expected);
        }
    }
}

/// The full source line(s) a node starts on.
fn line_text<'a>(src: &'a [u8], node: Node) -> &'a str {
    let start = node.start_byte();
    let begin = src[..start]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1);
    let end = src[start..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(src.len(), |i| start + i);
    std::str::from_utf8(&src[begin..end]).unwrap_or("")
}

fn scrub(src: &[u8], value: String, node: Node) -> String {
    if secrets::contains_secret(&value) {
        secrets::redact(&value)
    } else if secrets::contains_secret(line_text(src, node)) {
        "<redacted>".to_string()
    } else {
        value
    }
}

fn collect_literals(node: Node, src: &[u8], out: &mut BTreeSet<String>) {
    if node.kind() == "string" {
        if let Some(v) = string_value(node, src) {
            if v.chars().count() >= 3 && v.chars().count() <= 120 {
                out.insert(scrub(src, v, node));
            }
        }
        return;
    }
    for child in named_children(node) {
        collect_literals(child, src, out);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

fn exits(node: Node) -> bool {
    match node.kind() {
        "return_statement" | "throw_statement" => true,
        "statement_block" => {
            // A guard is short: `if (x) { log(); return; }` at most.
            let stmts: Vec<Node> = named_children(node)
                .into_iter()
                .filter(|c| c.kind() != "comment")
                .collect();
            stmts.len() <= 2 && stmts.last().is_some_and(|l| exits(*l))
        }
        _ => false,
    }
}

/// The line where a function's body starts, i.e. the end of its signature.
fn signature_end(node: Node) -> Option<u32> {
    node.child_by_field_name("body").map(line)
}

fn strip_colon(s: String) -> String {
    s.strip_prefix(':').map(str::to_string).unwrap_or(s)
}

fn unverified_value() -> ContractShape {
    ContractShape {
        kind: ShapeKind::Value,
        type_params: None,
        params: vec![],
        returns: None,
        members: vec![],
        type_text: None,
        unverified: true,
    }
}

fn literal_type(node: Node) -> Option<&'static str> {
    match node.kind() {
        "number" => Some("number"),
        "string" | "template_string" => Some("string"),
        "true" | "false" => Some("boolean"),
        _ => None,
    }
}

/// `(a:string,b?:number)=>void`
pub fn signature_text(shape: &ContractShape) -> String {
    let params: Vec<String> = shape
        .params
        .iter()
        .map(|p| {
            format!(
                "{}{}{}:{}",
                if p.rest { "..." } else { "" },
                p.name,
                if p.optional { "?" } else { "" },
                p.type_text.as_deref().unwrap_or("?")
            )
        })
        .collect();
    format!(
        "{}({})=>{}",
        shape.type_params.as_deref().unwrap_or(""),
        params.join(","),
        shape.returns.as_deref().unwrap_or("?")
    )
}

/// The root identifier of a callee and its first property:
/// `bus.publish` → (`bus`, `publish`); `formatMoney` → (`formatMoney`, None).
fn root_of(node: Node, src: &[u8]) -> Option<(String, Option<String>)> {
    match node.kind() {
        "identifier" => Some((text(node, src).to_string(), None)),
        "member_expression" => {
            let mut cur = node;
            let mut last_prop = None;
            while cur.kind() == "member_expression" {
                last_prop = cur.child_by_field_name("property");
                cur = cur.child_by_field_name("object")?;
            }
            if cur.kind() != "identifier" {
                return None;
            }
            // The property directly on the root.
            let mut first_prop = last_prop;
            let mut walk = node;
            while walk.kind() == "member_expression" {
                let obj = walk.child_by_field_name("object")?;
                if obj.kind() == "identifier" {
                    first_prop = walk.child_by_field_name("property");
                    break;
                }
                walk = obj;
            }
            Some((
                text(cur, src).to_string(),
                first_prop.map(|p| text(p, src).to_string()),
            ))
        }
        _ => None,
    }
}

fn env_key(node: Node, src: &[u8]) -> Option<String> {
    let obj = node.child_by_field_name("object")?;
    if norm(obj, src) != "process.env" {
        return None;
    }
    Some(text(node.child_by_field_name("property")?, src).to_string())
}

fn is_call_function(node: Node) -> bool {
    node.parent().is_some_and(|p| {
        matches!(p.kind(), "call_expression" | "new_expression")
            && (p.child_by_field_name("function") == Some(node)
                || p.child_by_field_name("constructor") == Some(node))
    })
}

fn is_declaration_name(node: Node) -> bool {
    node.parent().is_some_and(|p| {
        p.child_by_field_name("name") == Some(node)
            && matches!(
                p.kind(),
                "interface_declaration"
                    | "type_alias_declaration"
                    | "class_declaration"
                    | "abstract_class_declaration"
                    | "type_parameter"
                    | "enum_declaration"
            )
    })
}

/// Identifiers that declare a name rather than use one.
fn is_binding_position(node: Node) -> bool {
    let Some(p) = node.parent() else {
        return false;
    };
    let field_is = |f: &str| p.child_by_field_name(f) == Some(node);
    match p.kind() {
        "variable_declarator" => field_is("name"),
        "required_parameter" | "optional_parameter" => field_is("pattern"),
        "arrow_function" => field_is("parameter"),
        "function_declaration"
        | "function_expression"
        | "class_declaration"
        | "generator_function_declaration"
        | "method_definition" => field_is("name"),
        "import_specifier" | "export_specifier" | "namespace_import" | "import_clause"
        | "namespace_export" | "labeled_statement" | "break_statement" | "continue_statement" => {
            true
        }
        "catch_clause" => field_is("parameter"),
        "object_pattern"
        | "array_pattern"
        | "rest_pattern"
        | "assignment_pattern"
        | "pair_pattern"
        | "object_assignment_pattern" => true,
        "for_in_statement" => field_is("left"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns() -> Patterns {
        Patterns {
            publish: vec![CallPattern::parse("bus.publish($EVENT, ...)").unwrap()],
            subscribe: vec![
                CallPattern::parse("bus.subscribe($EVENT, ...)").unwrap(),
                CallPattern::parse("@OnEvent($EVENT)").unwrap(),
            ],
            prisma_clients: vec!["prisma".into()],
        }
    }

    fn decl<'a>(f: &'a FileFacts, name: &str) -> &'a Decl {
        f.decls.iter().find(|d| d.name == name).unwrap()
    }

    #[test]
    fn parses_call_patterns() {
        assert_eq!(
            CallPattern::parse("bus.publish($EVENT, ...)"),
            Some(CallPattern {
                callee: "bus.publish".into(),
                decorator: false,
                event_arg: 0
            })
        );
        let p = CallPattern::parse("emit(topic, $EVENT)").unwrap();
        assert_eq!(p.event_arg, 1);
        assert!(CallPattern::parse("@OnEvent($EVENT)").unwrap().decorator);
        assert!(p.matches("this.emit"));
        assert!(!p.matches("submit"));
    }

    #[test]
    fn fingerprints_ignore_formatting_comments_and_local_names() {
        let a = "export function total(items: number[], tax: number): number {\n  let sum = 0;\n  for (const i of items) { sum += i; }\n  return sum * (1 + tax);\n}\n";
        let b = "// totals\nexport function grandTotal(xs: number[], rate: number): number {\n    let acc = 0 // running\n    for (const x of xs) {\n        acc += x\n    }\n    return acc * (1 + rate)\n}\n";
        let c = "export function total(items: number[], tax: number): number {\n  let sum = 0;\n  for (const i of items) { sum += i; }\n  return sum * (2 + tax);\n}\n";
        let fa = extract("a.ts", a, false, &patterns());
        let fb = extract("b.ts", b, false, &patterns());
        let fc = extract("c.ts", c, false, &patterns());
        assert_eq!(
            decl(&fa, "total").fingerprint,
            decl(&fb, "grandTotal").fingerprint
        );
        assert_ne!(
            decl(&fa, "total").fingerprint,
            decl(&fc, "total").fingerprint
        );
        assert_ne!(fa.content_hash, fb.content_hash);
    }

    #[test]
    fn content_hash_ignores_formatting_and_import_paths() {
        let a = "import { x } from './util/ids';\nexport const y = { a: 1, b: \"q\" };\n";
        let b = "import { x } from \"./lib/ids\"\n\nexport const y = {\n  a: 1,\n  b: 'q',\n}\n";
        let fa = extract("a.ts", a, false, &patterns());
        let fb = extract("b.ts", b, false, &patterns());
        assert_eq!(fa.content_hash, fb.content_hash);
    }

    #[test]
    fn reads_shapes_with_optionality() {
        let src = r#"
export interface UserPreferences {
  userId: string;
  phone?: string;
  readonly tags: Array<string>;
  notify(channel: "sms" | "email", at?: Date): void;
}
export function send(to: string, body?: string, ...rest: string[]): Promise<void> {}
export const go = (x) => x;
export type Kind = "b" | "a";
export enum Color { Red = "red", Blue = "blue" }
"#;
        let f = extract("a.ts", src, false, &patterns());
        let up = decl(&f, "UserPreferences").shape.clone().unwrap();
        let names: Vec<(&str, bool)> = up
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.optional))
            .collect();
        assert_eq!(
            names,
            [
                ("userId", false),
                ("phone", true),
                ("tags", false),
                ("notify", false)
            ]
        );
        assert!(up.members[2].readonly);
        assert_eq!(
            up.members[3].type_text.as_deref(),
            Some("(channel:\"email\"|\"sms\",at?:Date)=>void")
        );
        let send = decl(&f, "send").shape.clone().unwrap();
        assert_eq!(send.params.len(), 3);
        assert!(!send.params[0].optional && send.params[1].optional && send.params[2].rest);
        assert_eq!(send.returns.as_deref(), Some("Promise<void>"));
        assert!(!send.unverified);
        assert!(decl(&f, "go").shape.as_ref().unwrap().unverified);
        assert_eq!(
            decl(&f, "Kind")
                .shape
                .as_ref()
                .unwrap()
                .type_text
                .as_deref(),
            Some("\"a\"|\"b\"")
        );
        assert_eq!(
            decl(&f, "Color").enum_values,
            [("Red".into(), "red".into()), ("Blue".into(), "blue".into())]
        );
        assert_eq!(f.exports.len(), 5);
    }

    #[test]
    fn records_scope_aware_references() {
        let src = r#"
import { formatMoney as fm } from "@shop/money";
import * as ids from "./ids";
import type { Order } from "./types";
function local(n: number): string { return String(n); }
export function label(order: Order, fm2: number): string {
  const fmLocal = (x: number) => x;
  ids.newOrderId();
  fmLocal(fm2);
  return fm(order.total) + local(1);
}
"#;
        let f = extract("a.ts", src, false, &patterns());
        let calls: Vec<(String, Option<String>)> = f
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::Call && r.from.as_deref() == Some("label"))
            .map(|r| (r.name.clone(), r.member.clone()))
            .collect();
        assert!(calls.contains(&("fm".into(), None)));
        assert!(calls.contains(&("local".into(), None)));
        assert!(calls.contains(&("ids".into(), Some("newOrderId".into()))));
        assert!(!calls.iter().any(|(n, _)| n == "fmLocal"));
        assert!(f.refs.iter().any(|r| r.kind == RefKind::Type
            && r.name == "Order"
            && r.from.as_deref() == Some("label")));
        assert_eq!(f.imports.len(), 3);
        assert_eq!(f.imports[0].bindings[0].local, "fm");
        assert_eq!(
            f.imports[0].bindings[0].imported,
            Imported::Named("formatMoney".into())
        );
    }

    #[test]
    fn extracts_events_data_env_hosts_and_facts() {
        let src = r#"
export async function ship(id: string, total: number): Promise<void> {
  if (!id) { throw new Error("missing id"); }
  if (total > LIMIT) { await prisma.payment.update({ where: { id } }); }
  await prisma.order.findMany({});
  await bus.publish("OrderShipped", { id });
  bus.subscribe(Events.Placed, () => {});
  const key = process.env.SMS_KEY;
  await fetch(`https://api.example.com/v1/${id}`);
  try { x(); } catch (e) {}
  handlers[id]();
}
"#;
        let f = extract("a.ts", src, false, &patterns());
        assert_eq!(f.events.len(), 2);
        assert_eq!(f.events[0].name, EventExpr::Literal("OrderShipped".into()));
        assert!(f.events[0].publish);
        assert_eq!(
            f.events[1].name,
            EventExpr::Member("Events".into(), "Placed".into())
        );
        let data: Vec<(&str, bool)> = f.data.iter().map(|d| (d.model.as_str(), d.write)).collect();
        assert_eq!(data, [("payment", true), ("order", false)]);
        assert_eq!(f.env[0].value, "SMS_KEY");
        assert_eq!(f.hosts[0].value, "api.example.com");
        let facts = decl(&f, "ship").facts.clone().unwrap();
        assert_eq!(facts.comparisons[0].op, ">");
        assert_eq!(facts.comparisons[0].left, "total");
        assert_eq!(facts.throws.len(), 1);
        assert_eq!(facts.guards.len(), 1);
        assert_eq!(facts.awaits.len(), 4);
        assert_eq!(facts.empty_catches.len(), 1);
        assert!(f.diagnostics.iter().any(|d| d.kind == "dynamic-access"));
    }

    #[test]
    fn extracts_test_cases_markers_and_assertions() {
        let src = r#"
import { describe, it, expect } from "vitest";
describe("applyDiscount", () => {
  it("applies the rate", () => {
    expect(apply(100)).toBe(90);
    expect(apply(0)).not.toBe(1);
  });
  it.skip("rounds", () => { expect(r(1.5)).toEqual(2); });
  it.todo("handles refunds");
  describe.only("nested", () => { test("x", () => { assert.equal(1, 1); }); });
});
"#;
        let f = extract("a.test.ts", src, true, &patterns());
        let cases: Vec<(&str, usize, Vec<String>)> = f
            .test_cases
            .iter()
            .map(|c| (c.name.as_str(), c.assertions.len(), c.markers.clone()))
            .collect();
        assert_eq!(
            cases,
            [
                ("applyDiscount › applies the rate", 2, vec![]),
                ("applyDiscount › rounds", 1, vec!["skip".to_string()]),
                (
                    "applyDiscount › handles refunds",
                    0,
                    vec!["todo".to_string()]
                ),
                ("applyDiscount › nested › x", 1, vec!["only".to_string()]),
            ]
        );
        assert_eq!(f.test_cases[0].expected[0].text, "toBe:90");
        assert!(f.literals.contains(&"applyDiscount".to_string()));
    }
}
