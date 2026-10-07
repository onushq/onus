//! The interfaces map providers implement.
//!
//! Map building is a pipeline: *discovery* providers find components,
//! *language* providers turn source files into symbols, shapes, references
//! and tests, and *fact* providers add relationships on top (events, data,
//! routes, compiler-confirmed references). Built-in providers and external
//! plugins (ADR 0006) implement the same interfaces, and every type here
//! serializes so it can cross a process boundary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::ResolvedExtractors;
use crate::map::{Component, Edge, MapDiagnostic, SourceFile, SymbolNode, TestNode};

/// Everything a provider needs to know about the tree it analyzes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Root of the tree. Providers read files below it and never write.
    pub root: PathBuf,
    /// Files to analyze, sorted by path. Paths are relative with `/`.
    pub files: Vec<WorkspaceFile>,
    pub components: Vec<Component>,
    /// Workspace packages by name, so imports of `@shop/events` resolve to
    /// source instead of third-party nodes.
    pub packages: BTreeMap<String, WorkspacePackage>,
    pub extractors: ResolvedExtractors,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFile {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    pub is_test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePackage {
    pub name: String,
    pub component: String,
    /// Directory of the package, relative to the root.
    pub dir: String,
    /// Source entrypoints, relative to the root.
    pub entrypoints: Vec<String>,
}

/// The part of the map one provider contributes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PartialMap {
    #[serde(default)]
    pub files: Vec<SourceFile>,
    #[serde(default)]
    pub symbols: Vec<SymbolNode>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub tests: Vec<TestNode>,
    #[serde(default)]
    pub diagnostics: Vec<MapDiagnostic>,
}

/// Finds components. Its results are combined with the other discovery
/// providers'; for each file, the most specific component wins.
pub trait DiscoveryProvider: Send + Sync {
    fn id(&self) -> &str;
    fn version(&self) -> String;
    /// Components of the tree at `root`, given its files.
    fn discover(&self, root: &Path, files: &[String]) -> Result<Vec<Component>, ProviderError>;
}

/// A language provider turns source files into map facts.
pub trait LanguageAdapter: Send + Sync {
    /// Short id, e.g. `typescript`.
    fn id(&self) -> &str;
    /// Version string recorded in `builtWith.providers`.
    fn version(&self) -> String;
    /// Whether this provider analyzes the file at `path`.
    fn handles(&self, path: &str) -> bool;
    /// Analyzes the files of `workspace` that this provider handles.
    fn build(&self, workspace: &Workspace) -> Result<PartialMap, ProviderError>;
}

/// A fact provider adds facts to the map built so far: relationships,
/// compiler-confirmed references, symbols of languages no other provider
/// handles.
pub trait FactProvider: Send + Sync {
    fn id(&self) -> &str;
    fn version(&self) -> String;
    fn facts(
        &self,
        workspace: &Workspace,
        so_far: &PartialMap,
    ) -> Result<PartialMap, ProviderError>;
}

/// Why a provider produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// Skipped on purpose, for example a provider that runs repository code
    /// outside trusted mode.
    Skipped(String),
    /// It ran and failed.
    Failed(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Skipped(m) => write!(f, "skipped: {m}"),
            ProviderError::Failed(m) => write!(f, "failed: {m}"),
        }
    }
}
