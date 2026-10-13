//! The codebase map: components, symbols, edges and everything else Onus
//! derives from a tree of source files.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version of the map and report JSON formats. Bumped on breaking changes.
pub const SCHEMA_VERSION: u32 = 1;

/// How a fact in the map is known.
///
/// Ordered from most to least trustworthy, so `max()` over a set of facts
/// gives the weakest one.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    /// Written down by a person in `onus.yaml`.
    Declared,
    /// Confirmed by the language's own compiler, indexer or language server.
    Compiler,
    /// Read directly from the syntax.
    Static,
    /// Observed at runtime (later phases).
    Traced,
    /// Guessed from conventions or heuristics.
    Inferred,
    /// Onus could not resolve it; treat as higher risk.
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Declared => "declared",
            Confidence::Compiler => "compiler",
            Confidence::Static => "static",
            Confidence::Traced => "traced",
            Confidence::Inferred => "inferred",
            Confidence::Low => "low",
        }
    }
}

/// The map of one tree of source files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodebaseMap {
    pub schema_version: u32,
    /// The commit the map was built from, or a label such as `worktree`.
    pub commit: String,
    pub built_with: BuiltWith,
    pub components: Vec<Component>,
    /// Every analyzed source file, with the facts needed to tell formatting
    /// and moves apart from real edits.
    pub files: Vec<SourceFile>,
    pub symbols: Vec<SymbolNode>,
    pub edges: Vec<Edge>,
    pub externals: Vec<ExternalService>,
    pub packages: Vec<PackageDep>,
    pub tests: Vec<TestNode>,
    pub rules: Vec<BoundaryRule>,
    pub diagnostics: Vec<MapDiagnostic>,
    /// Line coverage of this tree from test runs (lcov, Cobertura, Istanbul),
    /// when given. Added after the map is built (`onus_map::coverage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuiltWith {
    pub onus: String,
    /// Every provider that contributed, with its version.
    pub providers: BTreeMap<String, String>,
    /// sha256 of the `onus.yaml` used, or `inferred` when there was none.
    pub config_hash: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentKind {
    Package,
    Service,
    Module,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: String,
    pub kind: ComponentKind,
    /// Globs relative to the repository root.
    pub roots: Vec<String>,
    /// Source files that define the public surface, relative to the root.
    pub public_entrypoints: Vec<String>,
    pub owners: Vec<String>,
    pub labels: Vec<String>,
    /// The name other workspaces import this component by, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    /// Path relative to the repository root, with `/` separators.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_id: Option<String>,
    pub language: String,
    pub is_test: bool,
    pub lines: u32,
    /// Hash of the syntax tokens with comments, formatting and import
    /// specifiers removed. Equal hashes mean "same code, maybe reformatted".
    pub content_hash: String,
    /// Resolved targets of the file's imports, in source order: a file path,
    /// `npm:<package>`, or `?<specifier>` when unresolved.
    pub imports: Vec<String>,
    /// The APIs of third-party packages the file uses, as
    /// `<package> <name>[.<member>]` (`effect Effect.void`, `zod z.object`
    /// for `import { z }`, `zod object` through a namespace import), sorted.
    /// Lets a diff spot APIs the repository has never used before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_apis: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolKind {
    Function,
    Class,
    Method,
    Interface,
    Type,
    Enum,
    Const,
    /// A top-level `let` or `var`.
    Variable,
    Event,
    HttpRoute,
    DbTable,
    ConfigKey,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Class => "class",
            SymbolKind::Method => "method",
            SymbolKind::Interface => "interface",
            SymbolKind::Type => "type",
            SymbolKind::Enum => "enum",
            SymbolKind::Const => "const",
            SymbolKind::Variable => "variable",
            SymbolKind::Event => "event",
            SymbolKind::HttpRoute => "http-route",
            SymbolKind::DbTable => "db-table",
            SymbolKind::ConfigKey => "config-key",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    Public,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SymbolNode {
    /// Stable id: `<component>:<path in component>#<qualified name>`, or
    /// `event:<name>`, `db-table:<model>`, `config-key:<KEY>`.
    pub id: String,
    /// Absent for nodes shared by the whole repository (events, tables,
    /// config keys).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_id: Option<String>,
    pub kind: SymbolKind,
    pub name: String,
    pub visibility: Visibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<ContractShape>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loc: Option<Loc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<BodyFacts>,
    /// The value of a `const` or variable initialized with a string, number
    /// or boolean literal (secrets redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Loc {
    pub file: String,
    pub start: u32,
    pub end: u32,
    /// Last line of the signature, when the symbol has a separate body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_end: Option<u32>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ShapeKind {
    Function,
    Object,
    Enum,
    Alias,
    Class,
    Value,
}

/// A normalized signature, read from the syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContractShape {
    pub kind: ShapeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_params: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
    /// Normalized type text for aliases, consts and heritage clauses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_text: Option<String>,
    /// True when part of the shape is inferred rather than written.
    pub unverified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Param {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_text: Option<String>,
    pub optional: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rest: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub name: String,
    pub kind: MemberKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_text: Option<String>,
    pub optional: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub readonly: bool,
    /// Line of the member in its file.
    pub line: u32,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum MemberKind {
    Property,
    Method,
    Constructor,
    EnumMember,
}

/// Facts about a function body used to spot notable edits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BodyFacts {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparisons: Vec<Comparison>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub throws: Vec<FactSite>,
    /// Early exits: `if (...) return/throw` without an `else`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guards: Vec<FactSite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub awaits: Vec<FactSite>,
    /// `catch` blocks with an empty body.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub empty_catches: Vec<FactSite>,
}

impl BodyFacts {
    pub fn is_empty(&self) -> bool {
        self.comparisons.is_empty()
            && self.throws.is_empty()
            && self.guards.is_empty()
            && self.awaits.is_empty()
            && self.empty_catches.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub op: String,
    pub left: String,
    pub right: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FactSite {
    /// Normalized source text of the fact.
    pub text: String,
    pub line: u32,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Imports,
    Calls,
    ReferencesType,
    Reads,
    Writes,
    Publishes,
    Consumes,
    CallsExternal,
    ReadsConfig,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Imports => "imports",
            EdgeKind::Calls => "calls",
            EdgeKind::ReferencesType => "references-type",
            EdgeKind::Reads => "reads",
            EdgeKind::Writes => "writes",
            EdgeKind::Publishes => "publishes",
            EdgeKind::Consumes => "consumes",
            EdgeKind::CallsExternal => "calls-external",
            EdgeKind::ReadsConfig => "reads-config",
        }
    }

    /// Edges that make one piece of code depend on another.
    pub fn is_code_dependency(self) -> bool {
        matches!(
            self,
            EdgeKind::Imports | EdgeKind::Calls | EdgeKind::ReferencesType
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    /// A symbol id, or a module id (`<component>:<path>`) for code at the
    /// top level of a file.
    pub from: String,
    /// A symbol, module, `event:`, `db-table:`, `external:`, `config-key:` or
    /// `npm:` id.
    pub to: String,
    pub kind: EdgeKind,
    pub confidence: Confidence,
    pub sites: Vec<Site>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub file: String,
    pub line: u32,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalSource {
    /// From Onus's built-in registry of SDKs.
    BuiltIn,
    /// From `extractors.externals` in `onus.yaml`.
    Declared,
    /// Seen as a literal host in a `fetch`/`axios`/`got` call.
    Observed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExternalService {
    /// `external:<slug>`.
    pub id: String,
    pub vendor: String,
    /// `sms`, `email`, `payments`, `http`, …
    pub category: String,
    /// Kinds of data that leave the system through this service.
    pub egress: Vec<String>,
    pub packages: Vec<String>,
    pub hosts: Vec<String>,
    pub source: ExternalSource,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PackageDep {
    pub component_id: String,
    pub name: String,
    pub version: String,
    /// `dependencies`, `devDependencies`, `peerDependencies` or
    /// `optionalDependencies`.
    pub section: String,
    /// The `package.json` that declares it.
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestNode {
    /// `<component>:<path in component>`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_id: Option<String>,
    pub file: String,
    pub cases: Vec<TestCase>,
    /// Symbols referenced from the test file.
    pub exercises: Vec<String>,
    /// String literals used in the file, to spot source code that
    /// special-cases test inputs. Secrets are redacted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub literals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestCase {
    /// Full name, with enclosing `describe` blocks joined by ` › `.
    pub name: String,
    pub line: u32,
    pub end_line: u32,
    /// `expect(...)`/`assert(...)` calls, with their normalized text.
    pub assertions: Vec<FactSite>,
    /// `skip`, `only` or `todo`, including markers inherited from a
    /// `describe` block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<String>,
    /// Literal expected values, as `matcher:value`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected: Vec<FactSite>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryRule {
    pub id: String,
    pub deny: RuleEndpoints,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleEndpoints {
    /// A component id.
    pub from: String,
    /// A component id, or `<component>.internal` for its internal symbols.
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MapDiagnostic {
    /// `unresolved-import`, `dynamic-import`, `dynamic-access`,
    /// `dynamic-event-name`, `parse-error`, `no-entrypoint`, …
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub message: String,
    pub confidence: Confidence,
}

/// Line coverage of a tree, from the test runs that measured it (ADR 0017).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Sorted by path.
    pub sources: Vec<CoverageSource>,
    /// Map files the coverage measured, sorted by path.
    pub files: Vec<FileCoverage>,
    /// Tests the coverage names running a symbol (lcov `TN:` records),
    /// sorted by test, then target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<CoverageLink>,
    /// What was left out and why (stale or mismatched files).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageFormat {
    Lcov,
    Cobertura,
    Istanbul,
}

impl CoverageFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            CoverageFormat::Lcov => "lcov",
            CoverageFormat::Cobertura => "cobertura",
            CoverageFormat::Istanbul => "istanbul",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageOrigin {
    /// A file given on the command line or in onus.yaml.
    Supplied,
    /// An artifact of a run an environment recorded at this commit.
    Recorded,
}

impl CoverageOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            CoverageOrigin::Supplied => "supplied",
            CoverageOrigin::Recorded => "recorded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoverageSource {
    pub format: CoverageFormat,
    /// The file as given, or the artifact's path in the environment.
    pub path: String,
    pub sha256: String,
    pub origin: CoverageOrigin,
    /// The evidence manifest it was recorded in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// The command of the run that recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Its files that matched a map file.
    pub files: u32,
    /// Its files that matched none.
    pub unmatched: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileCoverage {
    pub path: String,
    /// Executable lines some test ran, as inclusive [first, last] ranges.
    pub covered: Vec<[u32; 2]>,
    /// Executable lines no test ran.
    pub uncovered: Vec<[u32; 2]>,
}

impl FileCoverage {
    /// The covered lines and the executable lines (covered or not).
    pub fn lines(&self) -> (BTreeSet<u32>, BTreeSet<u32>) {
        let expand = |rs: &[[u32; 2]]| -> BTreeSet<u32> {
            rs.iter()
                .flat_map(|[a, b]| *a..=*b)
                .collect::<BTreeSet<u32>>()
        };
        let covered = expand(&self.covered);
        let mut executable = expand(&self.uncovered);
        executable.extend(covered.iter().copied());
        (covered, executable)
    }

    /// The executable lines from `start` to `end` (inclusive), and how many
    /// of them ran.
    pub fn in_range(&self, start: u32, end: u32) -> (u32, u32) {
        let count = |rs: &[[u32; 2]]| -> u32 {
            rs.iter()
                .map(|[a, b]| {
                    let (a, b) = ((*a).max(start), (*b).min(end));
                    if a <= b { b - a + 1 } else { 0 }
                })
                .sum()
        };
        let ran = count(&self.covered);
        (ran + count(&self.uncovered), ran)
    }
}

/// Collapses sorted line numbers into inclusive ranges.
pub fn line_ranges(lines: &BTreeSet<u32>) -> Vec<[u32; 2]> {
    let mut out: Vec<[u32; 2]> = Vec::new();
    for &l in lines {
        match out.last_mut() {
            Some([_, end]) if *end + 1 == l => *end = l,
            _ => out.push([l, l]),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoverageLink {
    /// A `TestNode` id.
    pub test: String,
    /// A symbol id.
    pub target: String,
    /// Lines of the target the test ran.
    pub lines: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_coverage_counts_lines_in_ranges() {
        let f = FileCoverage {
            path: "a.ts".into(),
            covered: vec![[1, 2], [15, 16]],
            uncovered: vec![[3, 3], [20, 22]],
        };
        let (covered, executable) = f.lines();
        assert_eq!(covered.len(), 4);
        assert_eq!(executable.len(), 8);
        assert_eq!(f.in_range(2, 21), (6, 3));
        assert_eq!(f.in_range(4, 14), (0, 0));
        assert_eq!(line_ranges(&executable), [[1, 3], [15, 16], [20, 22]]);
    }
}
