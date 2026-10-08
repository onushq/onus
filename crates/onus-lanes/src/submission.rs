//! A change submission (`change.submit`): everything an agent hands in with
//! a change, and nothing of its reasoning. The judge and the classifier read
//! only this.

use onus_core::SemanticReport;
use onus_doors::runner::TestRun;
use serde::{Deserialize, Serialize};

/// Who made the change: the agent setup, whose track record the lanes use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSetup {
    /// The harness: `claude-code`, `codex`, `cursor`, `person`, …
    pub tool: String,
    /// The model, if any.
    #[serde(default)]
    pub model: String,
    /// A name for the configuration (prompts, tools, settings).
    #[serde(default)]
    pub config: String,
    /// The team the setup works for.
    #[serde(default)]
    pub team: String,
}

impl AgentSetup {
    /// The key outcomes are recorded under: `tool/model/config`.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.tool, self.model, self.config)
    }
}

/// The scope the change was made under, from its verified token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeUsed {
    pub task: String,
    /// The token's rights (`write:path:…`).
    pub rights: Vec<String>,
    /// Attenuations, as Datalog.
    #[serde(default)]
    pub attenuations: Vec<String>,
}

/// A person's approval of one row of the report (a weakened test, say).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    /// The row's id.
    pub row: String,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Submission {
    pub schema: u32,
    /// The base and head commits.
    pub base: String,
    pub head: String,
    /// The stated intent (an `onus-intent` block), if any.
    #[serde(default)]
    pub intent: Option<String>,
    /// Onus's report of the change.
    pub report: SemanticReport,
    /// Every path the change touches.
    pub changed_files: Vec<String>,
    /// Test runs the agent made (`onus run-test`), to be re-run.
    #[serde(default)]
    pub evidence: Vec<TestRun>,
    #[serde(default)]
    pub scope: Option<ScopeUsed>,
    /// Ids of escalations granted during the task.
    #[serde(default)]
    pub escalations: Vec<String>,
    #[serde(default)]
    pub approvals: Vec<Approval>,
    pub agent: AgentSetup,
}

pub const SCHEMA: u32 = 1;

impl Submission {
    pub fn approved(&self, row: &str) -> bool {
        self.approvals.iter().any(|a| a.row == row)
    }
}
