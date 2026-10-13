//! The provenance record: everything Onus knows about why one change was
//! allowed through. The intent, the agent and model, the token it held, the
//! evidence, any escalations, the lane and the judge's steps, who or what
//! approved it, and how it rolled out. `onus ci pr` writes one while a pull
//! request is open, and `onus provenance` rebuilds it as outcomes arrive.
//!
//! Every field is computed by the binary (`onus_lanes::provenance::build`).
//! A host stores the record and shows it as it is.
//!
//! Compatibility: new optional fields may be added under `schema: 1`.
//! Removing or renaming a field bumps [`SCHEMA`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::change::{IntentCheck, ReportSummary};
use crate::config::Lane;

/// The version of the record's format.
pub const SCHEMA: u32 = 1;

/// Everything Onus knows about why one change was allowed through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProvenanceRecord {
    /// [`SCHEMA`].
    pub schema: u32,
    /// `owner/repo#123`, or a head commit for a local change.
    pub change: String,
    pub stage: Stage,
    /// Unix seconds.
    pub generated_at: u64,
    /// The Onus version that built the record.
    pub onus: String,
    pub commits: Commits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<ReportFacts>,
    pub agent: Agent,
    /// The task token the change was made under. Never the token itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<Token>,
    /// Test runs handed in with the change.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub escalations: Vec<Escalation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<LaneFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<Judge>,
    pub approval: Approval,
    pub rollout: Rollout,
    /// Door decisions for the task, from the audit log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doors: Option<Doors>,
    /// What this record could not establish, and why.
    #[serde(default)]
    pub gaps: Vec<Gap>,
    /// sha256 (hex) of this record's compact JSON with `digest` empty.
    pub digest: String,
}

/// Where the change is: still open, merged, or closed without merging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Commits {
    pub base: String,
    pub head: String,
    /// The commit it landed as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Intent {
    pub source: IntentSource,
    /// sha256 (hex) of the intent text as given.
    pub sha256: String,
    /// The report's check of the intent against what changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<IntentCheck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IntentSource {
    /// The pull request body.
    PullRequest,
    /// A submission (`onus submit`, `onus_submit`).
    Submission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportFacts {
    pub summary: ReportSummary,
    /// Ids of the rows that need a person.
    #[serde(default)]
    pub needs_person: Vec<String>,
    pub changed_files: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub tool: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub config: String,
    #[serde(default)]
    pub team: String,
    /// `tool/model/config`, the key outcomes are recorded under.
    pub key: String,
    /// How it was known: `trailer`, `branch`, `bot`, `co-author`, `person`,
    /// `submission`, `record` (a pull record) or `outcome`.
    pub detected_from: String,
    /// Its track record when the record was built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<TrackRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackRecord {
    /// Changes still merged (not reverted, no incident).
    pub merged: u32,
    /// Incidents among its recent changes.
    pub recent_incidents: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub task: String,
    /// The token's rights (`write:path:…`).
    #[serde(default)]
    pub rights: Vec<String>,
    /// Attenuations, as Datalog.
    #[serde(default)]
    pub attenuations: Vec<String>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    /// The first 16 hex digits of the token's sha256; never the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    pub source: TokenSource,
}

/// Where the token facts come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TokenSource {
    /// Verified by the run that built the record.
    Token,
    /// Verified when the submission was made.
    Submission,
    /// The door log's `mint` entry for the task, widened by its grants.
    Audit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub command: String,
    pub commit: String,
    pub image: String,
    pub exit_code: i32,
    /// The evidence-store manifest, when it ran in an environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// It ran at the head commit.
    pub at_head: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Escalation {
    pub id: String,
    /// `permission`, `broken-test`, `contradictory-spec` or `impossible-task`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The rights asked for.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The strongest evidence grade (1 is strongest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_grade: Option<u8>,
    /// `granted`, `denied`, `needs-person`, `answered` or `open`.
    pub decision: String,
    /// Who decided: a person, or `policy` for an automatic grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LaneFacts {
    /// The final lane, after the judge.
    pub lane: Lane,
    /// Every rule that applied, in order.
    #[serde(default)]
    pub applied: Vec<LaneReason>,
    /// The lane policy's hash (the judge configuration id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LaneReason {
    pub lane: Lane,
    /// `default`, `policy`, `input`, `floor`, `record` or `audit`.
    pub source: String,
    pub reason: String,
    /// The ids of the report rows behind it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Judge {
    /// The judge configuration that decided.
    pub id: String,
    /// `approve`, `reject` or `escalate`.
    pub verdict: String,
    #[serde(default)]
    pub steps: Vec<JudgeStep>,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JudgeStep {
    pub name: String,
    /// `passed`, `failed`, `skipped` or `concern`.
    pub status: String,
    #[serde(default)]
    pub details: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub by: ApprovedBy,
    /// Everyone who approved (`@login`): row approvers, approving reviewers
    /// and who merged it, each once, in that order.
    #[serde(default)]
    pub people: Vec<String>,
    /// Rows of the report a person approved.
    #[serde(default)]
    pub rows: Vec<RowApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_by: Option<String>,
    /// The judge configuration, when the judge approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<String>,
}

/// Who or what let the change through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovedBy {
    /// The lane policy: it was in the auto-merge lane.
    Policy,
    /// The judge approved it.
    Judge,
    /// A person.
    Person,
    /// It merged although Onus blocked it.
    Override,
    /// Still open.
    Pending,
    /// Closed without merging.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RowApproval {
    pub row: String,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Rollout {
    /// `open`, `merged`, `closed`, `reverted`, `rolled-back` or `incident`.
    pub result: String,
    /// The commit it landed as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// When it merged or closed (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// A person audited it after it merged.
    #[serde(default)]
    pub audited: bool,
    /// The audit found a problem the automatic path missed.
    #[serde(default)]
    pub missed: bool,
    /// Every outcome of the change, in order.
    #[serde(default)]
    pub history: Vec<RolloutEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolloutEvent {
    /// Unix seconds.
    pub at: u64,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// For an incident: the components and symbols it involved.
    #[serde(default)]
    pub involved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Doors {
    /// The audit log's hash chain verified.
    pub intact: bool,
    #[serde(default)]
    pub entries: Vec<DoorEntry>,
}

/// One door decision, as the audit log holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DoorEntry {
    pub seq: u64,
    pub at: u64,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub decision: String,
    pub reason: String,
    pub hash: String,
}

/// Something the record cannot vouch for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub field: String,
    /// Why, and how to fill it.
    pub reason: String,
}
