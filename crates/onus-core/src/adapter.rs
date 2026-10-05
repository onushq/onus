//! The interface every language adapter implements.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::ResolvedExtractors;
use crate::map::{Component, Edge, MapDiagnostic, SourceFile, SymbolNode, TestNode};

/// Everything an adapter needs to know about the tree it analyzes.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Root of the tree. Adapters read files below it and never write.
    pub root: PathBuf,
    /// Files to analyze, sorted by path. Paths are relative with `/`.
    pub files: Vec<WorkspaceFile>,
    pub components: Vec<Component>,
    /// Workspace packages by npm name, so imports of `@shop/events` resolve
    /// to source instead of `npm:` nodes.
    pub packages: BTreeMap<String, WorkspacePackage>,
    pub extractors: ResolvedExtractors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFile {
    pub path: String,
    pub component: Option<String>,
    pub is_test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePackage {
    pub name: String,
    pub component: String,
    /// Directory of the package, relative to the root.
    pub dir: String,
    /// Source entrypoints, relative to the root.
    pub entrypoints: Vec<String>,
}

/// The part of the map one adapter contributes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartialMap {
    pub files: Vec<SourceFile>,
    pub symbols: Vec<SymbolNode>,
    pub edges: Vec<Edge>,
    pub tests: Vec<TestNode>,
    pub diagnostics: Vec<MapDiagnostic>,
}

/// A language adapter turns source files into map facts. It only reads
/// files: it never installs or executes anything from the analyzed tree.
pub trait LanguageAdapter: Send + Sync {
    /// Short id, e.g. `typescript`.
    fn id(&self) -> &'static str;
    /// Version string recorded in `builtWith.adapters`.
    fn version(&self) -> String;
    /// Whether this adapter analyzes the file at `path`.
    fn handles(&self, path: &str) -> bool;
    /// Analyzes the files of `workspace` that this adapter handles.
    fn build(&self, workspace: &Workspace) -> PartialMap;
}
