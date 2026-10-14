//! The behavior diff (ADR 0019): the tests that reach a change, run at the
//! base and at the head, and what those runs showed.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What `onus report --behavior` ran and how far it got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorRun {
    pub status: BehaviorStatus,
    /// Why it is partial or skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The command template, as configured (`environment.behavior.command`).
    pub command: String,
    pub budget_secs: u32,
    pub elapsed_secs: u32,
    /// The test files that reach the change, changed tests first.
    pub selected: Vec<SelectedTest>,
    /// Test files that reach the change but were left out by `maxFiles`.
    pub truncated: u32,
    pub runs: Vec<BehaviorSideRun>,
    /// Cases that failed at the head and passed when run again: `file › case`.
    pub flaky: Vec<String>,
    /// Cases that fail at the base and at the head with the same message.
    pub already_failing: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum BehaviorStatus {
    /// Every run it needed ran.
    Complete,
    /// Some runs ran; `reason` says which did not.
    Partial,
    /// Nothing was compared; `reason` says why.
    Skipped,
}

impl BehaviorStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            BehaviorStatus::Complete => "complete",
            BehaviorStatus::Partial => "partial",
            BehaviorStatus::Skipped => "skipped",
        }
    }
}

/// A test file chosen from the maps.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SelectedTest {
    pub file: String,
    /// The test file itself changed.
    pub changed: bool,
    /// Code-dependency hops from a changed file (0: it exercises one).
    pub depth: u32,
    /// The changed file it reaches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// It exists at the base, and so ran there.
    pub base: bool,
    /// It exists at the head.
    pub head: bool,
}

/// Which run.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum RunSide {
    /// The base's tests on the base's code.
    Base,
    /// The head's tests on the head's code.
    Head,
    /// The head's files with failing cases, run once more at the head.
    Retry,
    /// The base versions of edited tests on the head's code.
    Cross,
}

impl RunSide {
    pub fn as_str(self) -> &'static str {
        match self {
            RunSide::Base => "base",
            RunSide::Head => "head",
            RunSide::Retry => "retry",
            RunSide::Cross => "cross",
        }
    }
}

/// One run of the behavior command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorSideRun {
    pub side: RunSide,
    /// The commit whose code ran.
    pub commit: String,
    /// Test files given to the command.
    pub files: u32,
    /// The run's id in the evidence store (`onus evidence show <id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// None when it timed out.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cases: u32,
    pub failed: u32,
    pub skipped: u32,
}

/// How a test case ended, as a JUnit file reports it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum CaseOutcome {
    Passed,
    Failed,
    Error,
    Skipped,
}

impl CaseOutcome {
    /// A failure or an error.
    pub fn failed(self) -> bool {
        matches!(self, CaseOutcome::Failed | CaseOutcome::Error)
    }
}

/// One `<testcase>` of a JUnit file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JunitCase {
    pub name: String,
    pub classname: String,
    /// The `file` attribute, relative to the repository root.
    pub file: Option<String>,
    pub outcome: CaseOutcome,
    /// The failure or error `message`, else the start of its text.
    pub message: Option<String>,
}
