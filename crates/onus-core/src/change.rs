//! Semantic changes and the report built from them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::map::{Confidence, CoverageSource, MapDiagnostic};

/// The manifesto's change kinds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeKind {
    Additive,
    Breaking,
    Internal,
    Dependency,
    Config,
    Test,
    SecuritySensitive,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Additive => "additive",
            ChangeKind::Breaking => "breaking",
            ChangeKind::Internal => "internal",
            ChangeKind::Dependency => "dependency",
            ChangeKind::Config => "config",
            ChangeKind::Test => "test",
            ChangeKind::SecuritySensitive => "security-sensitive",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeLevel {
    Relationship,
    Behavior,
    Structure,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    Base,
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SemanticChange {
    /// Deterministic id: `<subkind>:<subject>` plus a suffix when needed.
    pub id: String,
    pub kind: ChangeKind,
    pub subkind: String,
    pub level: ChangeLevel,
    /// The map id the change is about.
    pub subject: String,
    /// The component the change happens in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// Short label for the Kind column, e.g. "Contract change, additive".
    pub kind_label: String,
    pub title: String,
    pub why_it_matters: String,
    pub hints: Hints,
    pub locations: Vec<Location>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<Stats>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hints {
    /// Sensitivity labels of the components involved.
    pub labels: Vec<String>,
    /// Files (outside tests) that depend on the subject in the head map.
    pub blast_radius: u32,
    pub novelty: Vec<String>,
    pub confidence: Confidence,
    /// Tests, CI, policies, labels or declared contracts were touched.
    pub rules_of_the_game: bool,
    /// The change is outside the stated intent.
    pub intent_mismatch: bool,
    /// A person should look at this row.
    pub needs_person: bool,
    /// Other components (not the row's own) that depend on what the row
    /// changed in the head map (the base map for removed code): the subject,
    /// or for a component's internal row, the symbols its changed lines are
    /// in.
    #[serde(default)]
    pub dependent_components: u32,
    /// Those components' ids, sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependents: Vec<String>,
    /// How well tests and traces cover the code the row changes; absent
    /// when the row changes no code (config, dependencies, tests).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tested: Option<TestReach>,
    /// Owners of the components involved (onus.yaml, else CODEOWNERS), for
    /// routing a review. Sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    /// The row's changed executable lines and how many ran in tests; absent
    /// without coverage, or when the row changes no executable head line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<LineCoverage>,
}

/// Changed executable lines of a row (or a report) and how many ran in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LineCoverage {
    pub changed: u32,
    pub ran: u32,
}

/// Whether tests or traces reach the files a row changes. File granularity:
/// a file counts as tested when a test exercises any symbol in it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum TestReach {
    /// A test in the head map exercises every changed code file.
    Tested,
    /// Every changed code file is tested or seen in a runtime trace.
    Traced,
    /// At least one changed code file is neither.
    Untested,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub file: String,
    /// First and last line, inclusive, 1-based.
    pub lines: [u32; 2],
    /// Which tree the lines refer to.
    pub side: Side,
}

impl Location {
    pub fn head(file: impl Into<String>, start: u32, end: u32) -> Self {
        Location {
            file: file.into(),
            lines: [start, end.max(start)],
            side: Side::Head,
        }
    }

    pub fn base(file: impl Into<String>, start: u32, end: u32) -> Self {
        Location {
            file: file.into(),
            lines: [start, end.max(start)],
            side: Side::Base,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub lines_added: u32,
    pub lines_removed: u32,
    pub files: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SemanticReport {
    pub schema_version: u32,
    pub base: String,
    pub head: String,
    pub summary: ReportSummary,
    /// Ranked, most important first.
    pub changes: Vec<SemanticChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_check: Option<IntentCheck>,
    /// New boundary-rule violations (also listed in `changes`).
    pub rule_violations: Vec<SemanticChange>,
    pub structure: StructureNotes,
    pub text_stats: TextStats,
    /// Diagnostics of the head map in files this change touches.
    pub map_diagnostics: Vec<MapDiagnostic>,
    /// Which changed lines tests ran, when coverage of the head was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<ReportCoverage>,
}

/// The changed lines tests ran, from coverage of the head (ADR 0017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportCoverage {
    pub sources: Vec<CoverageSource>,
    /// Added or edited head lines the coverage counts as executable.
    pub changed: u32,
    pub ran: u32,
    /// Changed executable lines no test ran (head side), grouped into ranges.
    pub not_run: Vec<Location>,
    /// Changed source files with runtime code that the coverage does not
    /// include.
    pub not_measured: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub meaning_changes: u32,
    pub needs_attention: u32,
    pub secrets: u32,
    pub new_rule_violations: u32,
    pub intent_mismatches: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntentCheck {
    pub stated: String,
    pub touches: Vec<String>,
    pub contracts: Vec<String>,
    pub externals: Vec<String>,
    pub mismatches: Vec<IntentMismatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntentMismatch {
    pub change_id: String,
    pub reason: String,
}

/// Changes that move code without changing what it means.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StructureNotes {
    pub moved_files: Vec<MovedFile>,
    /// Files whose only changes are formatting, comments or updated import
    /// paths.
    pub formatting_only: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MovedFile {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextStats {
    pub files: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
}
