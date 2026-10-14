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
use crate::config::{ApprovalRule, Lane, MetricGuard};

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
    /// Rollbacks, not planned, among its recent changes.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub recent_rollbacks: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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
    /// The agent's own submission, handed in beside the run (`onus ci pr
    /// --submission`, `onus provenance --agent-submission`), such as one
    /// kept on the records branch. On its word: this run cannot tell it was
    /// verified when it was made, and whoever can write where it was kept
    /// could have written it. Ranked below `audit` and `inspection`.
    AgentSubmission,
    /// The door log's `mint` entry for the task, widened by its grants.
    Audit,
    /// The door's inspection of the token (`--token-inspection`, or the one
    /// Onus Cloud keeps with an escalation), widened by the escalations
    /// granted to the task. What the door said it held: this run did not
    /// verify it.
    Inspection,
    /// The door log the agent attached to its work in Onus Cloud: the
    /// door's inspection sent with it, or its `mint` entry, widened by its
    /// grants. Taken only when its chain verifies, and ranked below the
    /// rest: whoever attached it chose what it holds.
    Attached,
}

/// What a door says its task token holds: never the token itself. The door
/// sends one with each escalation it files in Onus Cloud, and `onus token
/// inspect` prints one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenInspection {
    pub task: String,
    /// The token's rights (`write:path:…`).
    #[serde(default)]
    pub rights: Vec<String>,
    /// Attenuations, as Datalog.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attenuations: Vec<String>,
    /// Unix seconds. 0, as `onus token inspect` prints it, is none, and so
    /// is a negative or fractional number.
    #[serde(
        default,
        deserialize_with = "seconds_or_none",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<f64>")]
    pub expires: Option<u64>,
}

/// A time in Unix seconds, read leniently: a door may send any number, and
/// anything but a positive whole number is no time.
fn seconds_or_none<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let n = Option::<serde_json::Number>::deserialize(d)?;
    Ok(n.and_then(|n| n.as_u64()).filter(|&n| n > 0))
}

/// One escalation as Onus Cloud keeps it, with its decision: the input of
/// `onus provenance --escalations FILE` and `onus ci pr --escalations FILE`,
/// which take a JSON array of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EscalationDecision {
    /// Onus Cloud's id for it.
    pub id: String,
    /// The door's own request id (`.onus/escalations/<id>.json`), which a
    /// submission lists once it is granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    /// The request id `onus escalation assess` gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// `permission`, `broken-test`, `contradictory-spec` or `impossible-task`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The repository (`owner/repo`); one for another repository is left out.
    #[serde(
        default,
        alias = "repositoryFullName",
        skip_serializing_if = "Option::is_none"
    )]
    pub repository: Option<String>,
    /// The pull request number; one for another pull request is left out.
    #[serde(default, alias = "pullNumber", skip_serializing_if = "Option::is_none")]
    pub pull: Option<u64>,
    /// What the door filed it at: the commit it had pushed, a branch, or
    /// `refs/pull/<n>/head`. With no pull request, it ties the escalation
    /// to the change whose commit, branch or pull request it names.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The commit Onus Cloud resolved `ref` to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The rights asked for.
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(alias = "status")]
    pub state: CloudState,
    /// The strongest evidence grade (1 is strongest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_grade: Option<u8>,
    /// `system:policy` for an automatic grant, else the person (`user:<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// The person's name, shown instead of `decidedBy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<Moment>,
    /// When a waiting escalation expires: a stored row past it is
    /// `expired`, as Onus Cloud shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Moment>,
    /// Why the agent asked. Read, but not kept: the record's `reason` is
    /// the decider's, from `answer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why it was decided so: a denial's reason, or the answer to a blocked
    /// report. The record keeps it as `reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// The door's inspection of the task token it filed under; never the
    /// token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<TokenInspection>,
}

/// Where an escalation is in Onus Cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CloudState {
    /// Being checked.
    Assessing,
    /// Waiting for a person.
    Waiting,
    Granted,
    Denied,
    /// A blocked report a person answered.
    Resolved,
    Withdrawn,
    Expired,
    Failed,
}

/// A time: Unix seconds, or an RFC 3339 date in UTC
/// (`2026-10-12T10:00:00Z`, `Z` or `+00:00`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Moment {
    Seconds(u64),
    Date(
        #[schemars(regex(
            pattern = r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|\+00:00)$"
        ))]
        String,
    ),
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
    /// `granted`, `denied`, `needs-person`, `answered` or `open`; from Onus
    /// Cloud also `withdrawn`, `expired` or `failed`.
    pub decision: String,
    /// Who decided: a person, or `policy` for an automatic grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// Why it was decided so: the door log's reason, or Onus Cloud's
    /// `answer`. Never why the agent asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Onus Cloud's id for it, when it was filed there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_id: Option<String>,
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
    /// `default`, `policy`, `input`, `floor`, `model`, `record` or `audit`.
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
    /// Whose approving review counted (`lanes.approval`): `owner` or
    /// `member`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ApprovalRule>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// Where the change is in production, from its outcomes and its deploy
    /// and rollout stages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RolloutStatus>,
    /// The share of production traffic last reported (a deploy or a rollout
    /// stage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// The plan for the change's final lane, from its pull record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanFacts>,
    /// Every deploy and rollout stage of the change, in time order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<StageEvent>,
    /// Every other production signal tied to the change, in time order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<SignalRef>,
}

/// Where a change is in production.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RolloutStatus {
    /// Still open.
    Pending,
    /// Closed without merging.
    NotShipped,
    /// Merged; no production deploy or rollout stage reported yet.
    AwaitingDeploy,
    /// Reaching a share of production traffic.
    InProgress,
    /// The last stage holds: not baked yet, or a sample is missing.
    Holding,
    /// At all of production traffic.
    Complete,
    /// Rolled back: by Onus's decision on a stage, or reported.
    RolledBack,
    /// Reverted after it merged.
    Reverted,
}

/// A rollout plan, as the record keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanFacts {
    /// `none`, `canary` or `flag`.
    pub strategy: String,
    #[serde(default)]
    pub steps: Vec<u8>,
    #[serde(default)]
    pub bake_minutes: u32,
    #[serde(default)]
    pub guards: Vec<MetricGuard>,
    #[serde(default)]
    pub automatic_rollback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// A deploy or rollout stage reported for the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StageEvent {
    /// Unix seconds.
    pub at: u64,
    /// `deploy` or `rollout`.
    pub kind: String,
    /// The share of traffic; 100 for a deploy that names none.
    pub percent: u8,
    /// Onus's decision on a rollout stage: `promote`, `hold`, `rollback` or
    /// `complete`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// Absent means production. Stages elsewhere are listed, and do not
    /// count toward `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A production signal tied to the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalRef {
    pub id: String,
    /// `incident`, `error-spike`, `latency`, `metric` or `rollback`.
    pub kind: String,
    /// Unix seconds.
    pub at: u64,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// The door log's hash chain verified: the whole log, or an attached
    /// slice from its first entry. False when only Onus Cloud's calls are
    /// known (`source: onus-cloud`): they have no chain to check.
    pub intact: bool,
    /// The task's door decisions, from the door log.
    #[serde(default)]
    pub entries: Vec<DoorEntry>,
    /// Where `entries` came from. Absent: the door log this run read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DoorSource>,
    /// Onus Cloud's log of the calls made under the task token, summed up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<CloudCalls>,
}

/// Where door decisions came from, when not the door log (audit.jsonl)
/// this run read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DoorSource {
    /// The task's slice of the door log, attached to its work in Onus Cloud
    /// (`onus cloud evidence --door-log`); its chain is checked here.
    Attached,
    /// Onus Cloud's own log of the calls made under the task token. Onus
    /// Cloud's word: it has no hash chain.
    OnusCloud,
}

/// Onus Cloud's log of the calls made under a task token, summed up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloudCalls {
    /// Always `onus-cloud`, so these are never taken for the door log.
    pub source: CallsSource,
    pub total: u32,
    pub allowed: u32,
    pub denied: u32,
    /// Calls the API could not answer.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed: u32,
    /// Calls that name no task, counted as the change's on Onus Cloud's
    /// word.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub untasked: u32,
    /// Unix seconds of the first and the last call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_at: Option<u64>,
    /// The ids of the tokens the calls were made with; never a token.
    #[serde(default)]
    pub tokens: Vec<String>,
    /// The calls by route and scope, with how each was decided.
    #[serde(default)]
    pub routes: Vec<RouteCalls>,
    /// Every call Onus Cloud refused, in time order (at most 100).
    #[serde(default)]
    pub denials: Vec<CallDenial>,
}

/// Where Onus Cloud's calls came from: only ever Onus Cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CallsSource {
    /// Onus Cloud's own log of the calls made under the task token, on its
    /// word: it has no hash chain.
    OnusCloud,
}

/// The calls to one route, under one scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteCalls {
    pub method: String,
    /// `/v1/orgs/:org/environments/:env/runs`.
    pub route: String,
    /// What the route needs of a scoped token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub allowed: u32,
    pub denied: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed: u32,
}

/// A call Onus Cloud refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CallDenial {
    /// Unix seconds.
    pub at: u64,
    pub method: String,
    pub route: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The denial, as Onus Cloud sent it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_id: Option<String>,
}

/// One call Onus Cloud logged under a task token: the input of `onus
/// provenance --calls FILE` and `onus ci pr --calls FILE`, which take a
/// JSON array of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloudCall {
    /// When it was made (`createdAt` is read too).
    #[serde(alias = "createdAt")]
    pub at: Moment,
    pub method: String,
    pub route: String,
    /// What the route needs of a scoped token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub decision: CallDecision,
    /// The token's id (`credentialId` is read too); never the token.
    #[serde(
        default,
        alias = "credentialId",
        skip_serializing_if = "Option::is_none"
    )]
    pub token_id: Option<String>,
    /// The task the token was minted for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The denial, as it was sent back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// How Onus Cloud decided a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CallDecision {
    Allowed,
    Denied,
    /// The API could not answer it.
    Failed,
}

/// A task's slice of its door log (audit.jsonl): the run of entries from
/// the task's first to its last, whatever lies between, so its hash chain
/// can be checked where it is read. `onus cloud evidence --door-log` and
/// `onus_cloud_attach_evidence` attach one to a pull request, and `onus ci
/// pr --audit` and `onus provenance --audit` read it. Each hash is over the
/// entry's JSON as the door wrote it: keep the slice as it was sent (its
/// text), since a number written again another way (1.0 as 1, an integer
/// past 2^53) no longer verifies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DoorLogSlice {
    /// The task whose slice it is.
    pub task: String,
    /// When it was cut (Unix seconds).
    pub at: u64,
    /// The worktree's commit when it was cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// The door's inspection of the token it held for the task, when the
    /// door that cut the slice holds one (`onus mcp --token`); never the
    /// token. Outside the chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection: Option<TokenInspection>,
    pub entries: Vec<DoorLogEntry>,
}

/// One entry of a door log, as the log holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DoorLogEntry {
    pub seq: u64,
    pub at: u64,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub decision: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
    /// The hash of the entry before it.
    pub prev: String,
    /// sha256 of the entry's JSON with `hash` empty.
    pub hash: String,
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
