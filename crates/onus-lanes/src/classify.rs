//! The rules-first classifier: which lane a change takes, and why.
//!
//! 1. Start at the policy's default lane, or lower when a `match: every`
//!    rule covers every row.
//! 2. Raise it with every other rule that applies.
//! 3. Raise it with the built-in risk inputs (`lanes.inputs`): blast
//!    radius, untested code, low map confidence and the history of the
//!    components the change touches. They only raise (ADR 0014). When the
//!    policy sets `lanes.inputs.coverage`, too few changed lines run in
//!    tests, a changed runtime file the coverage leaves out, or no coverage
//!    at all, sends the change to the judge (ADR 0017).
//! 4. Raise it with the hard floors, which no policy can lower: sensitive
//!    code, rules of the game and third-party code new to the repository go
//!    to a person; a new dependency never auto-merges; committed secrets,
//!    writes outside the token's scope, and weakened tests or tests that
//!    pass at the base and fail at the head (`onus report --behavior`, ADR
//!    0019) without an approval are blocked.
//! 5. Raise it with the learned model, when `lanes.learned` is set: at or
//!    above its thresholds a change goes to the judge or a person. It never
//!    lowers a lane and never blocks (ADR 0015).
//! 6. A setup with too short a record, or whose recent changes caused
//!    incidents or were rolled back, may not auto-merge, and a share of the
//!    remaining auto-merges is audited by a person.
//!
//! `classify_with` then attaches the rollout plan for the final lane
//! (`rollout::plan`, ADR 0016).

use std::collections::{BTreeMap, BTreeSet};

use onus_core::{
    ChangeKind, Confidence, CoverageOrigin, Lane, LaneRule, LanesConfig, LearnedConfig, RuleMatch,
    SemanticChange, SemanticReport, TestReach, Thresholds,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::learned::{Factor, Features, Model, Prediction};
use crate::rollout::RolloutPlan;
use crate::submission::Submission;

/// One reason the lane is what it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub lane: Lane,
    /// `default`, `policy`, `input`, `floor`, `model`, `record` or `audit`.
    pub source: String,
    pub reason: String,
    /// The ids of the report rows behind it, when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub lane: Lane,
    /// Every rule that applied, in the order it was applied.
    pub applied: Vec<Applied>,
    /// The learned model's view, when `lanes.learned` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned: Option<LearnedRisk>,
    /// What the change looked like when classified: kept with its records
    /// for training.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Features>,
    /// How the change should meet production traffic, for its lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout: Option<RolloutPlan>,
}

/// The learned model's view of a change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnedRisk {
    /// `applied`, `missing` (no model file at the commit), `stale` (another
    /// feature set) or `invalid` (it does not parse).
    pub status: String,
    /// The model's id (`learned::model_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The change's risk, to 3 decimal places.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<f64>,
    pub judge_at: f64,
    pub human_at: f64,
    /// Whether the model raised the lane.
    pub raised: bool,
    /// The features that pushed the risk up most, largest first.
    #[serde(default)]
    pub factors: Vec<Factor>,
    /// Why a missing, stale or invalid model was ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl LearnedRisk {
    /// A model that could not be used: the lane comes from rules only.
    pub fn ignored(status: &str, config: &LearnedConfig, note: String) -> LearnedRisk {
        LearnedRisk {
            status: status.into(),
            model: None,
            risk: None,
            judge_at: config.judge,
            human_at: config.human,
            raised: false,
            factors: vec![],
            note: Some(note),
        }
    }
}

/// A learned model, ready to apply.
#[derive(Debug, Clone, Copy)]
pub struct Learned<'a> {
    pub model: &'a Model,
    /// Its id (`learned::model_id`).
    pub id: &'a str,
    pub config: &'a LearnedConfig,
    /// The change's features.
    pub features: &'a Features,
}

fn kind_name(k: ChangeKind) -> &'static str {
    k.as_str()
}

fn matches(rule: &LaneRule, row: &SemanticChange) -> bool {
    let any = |list: &[String], value: &str| list.is_empty() || list.iter().any(|v| v == value);
    any(&rule.kinds, kind_name(row.kind))
        && any(&rule.subkinds, &row.subkind)
        && any(&rule.components, row.component.as_deref().unwrap_or("root"))
        && (rule.labels.is_empty() || row.hints.labels.iter().any(|l| rule.labels.contains(l)))
}

fn describe(rule: &LaneRule) -> String {
    let mut parts = Vec::new();
    for (name, list) in [
        ("kinds", &rule.kinds),
        ("subkinds", &rule.subkinds),
        ("components", &rule.components),
        ("labels", &rule.labels),
    ] {
        if !list.is_empty() {
            parts.push(format!("{name} {}", list.join(", ")));
        }
    }
    if parts.is_empty() {
        "every change".into()
    } else {
        parts.join("; ")
    }
}

/// What the classifier needs to know about the agent setup's record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Record {
    /// Changes on record for this setup (merged, not reverted).
    pub changes: u32,
    /// Incidents among its recent changes.
    pub recent_incidents: u32,
    /// Recent changes rolled back, not on purpose.
    pub recent_rollbacks: u32,
}

/// Reverts (rollbacks included) and incidents recorded against one
/// component in the history window, and the production signals that name
/// it (`signals::extend_history`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AreaHistory {
    pub reverts: u32,
    pub incidents: u32,
    /// The changes behind them, sorted; a signal tied to no change stands
    /// for itself by its id.
    pub changes: Vec<String>,
    /// Error spikes, latency and metric signals (medium severity or more,
    /// or none given).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub error_spikes: u32,
    /// When the latest of them was recorded (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_at: Option<u64>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Component → its history.
pub type AreaHistories = BTreeMap<String, AreaHistory>;

/// The area history counts a row against: its component, `root` for code
/// outside every component, and none for a row that spans the repository
/// (a widespread or summary row).
pub fn area(row: &SemanticChange) -> Option<&str> {
    match &row.component {
        Some(c) => Some(c),
        None if row.subject == "repository" => None,
        None => Some("root"),
    }
}

static NO_HISTORY: AreaHistories = BTreeMap::new();

/// Everything the classifier reads besides the report, submission and
/// policy. Later inputs are added here as fields, never as parameters.
pub struct ClassifyContext<'a> {
    /// The agent setup's record, when the change has a submission.
    pub record: Option<Record>,
    /// The history of the components the change touches.
    pub areas: &'a AreaHistories,
    /// Whether the submission's token allows writing a path (`None` when
    /// there is no token).
    pub writable: Option<&'a dyn Fn(&str) -> bool>,
    /// The learned model, when `lanes.learned` is set and one was found.
    pub learned: Option<Learned<'a>>,
}

impl std::fmt::Debug for ClassifyContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClassifyContext")
            .field("record", &self.record)
            .field("areas", &self.areas)
            .field("writable", &self.writable.is_some())
            .field("learned", &self.learned.map(|l| l.id))
            .finish()
    }
}

impl Default for ClassifyContext<'_> {
    fn default() -> Self {
        ClassifyContext {
            record: None,
            areas: &NO_HISTORY,
            writable: None,
            learned: None,
        }
    }
}

/// "a, b and c", or "a, b, c and 2 more" past three.
fn some(items: &[String]) -> String {
    let shown: Vec<&str> = items.iter().take(3).map(String::as_str).collect();
    let more = items.len().saturating_sub(3);
    match (shown.as_slice(), more) {
        ([], _) => String::new(),
        ([one], _) => one.to_string(),
        (_, 0) => format!(
            "{} and {}",
            shown[..shown.len() - 1].join(", "),
            shown[shown.len() - 1]
        ),
        (_, n) => format!("{} and {n} more", shown.join(", ")),
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Hundredths of a percent as a percentage: `9000` is `90`, `5550` `55.5`.
fn percent(hundredths: u64) -> String {
    let s = format!("{}.{:02}", hundredths / 100, hundredths % 100);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The lane a threshold puts a count in, with the threshold.
fn threshold_met(t: &Thresholds, n: u32) -> Option<(Lane, u32)> {
    let lane = t.lane_for(n)?;
    Some((lane, t.get(lane).unwrap_or(n)))
}

/// Classifies a change.
pub fn classify(
    report: &SemanticReport,
    submission: Option<&Submission>,
    config: &LanesConfig,
    ctx: &ClassifyContext,
) -> Classification {
    let record = ctx.record;
    let mut applied = Vec::new();
    let mut lane = config.default;
    applied.push(Applied {
        lane,
        source: "default".into(),
        reason: format!("the policy's default lane is {}", lane.as_str()),
        rows: vec![],
    });
    let rows = &report.changes;
    // 1. A lower start when one rule covers the whole change.
    for rule in config.rules.iter().filter(|r| r.match_ == RuleMatch::Every) {
        if !rows.is_empty() && rows.iter().all(|r| matches(rule, r)) && rule.lane < lane {
            lane = rule.lane;
            applied.push(Applied {
                lane,
                source: "policy".into(),
                reason: format!("every row matches the rule for {}", describe(rule)),
                rows: vec![],
            });
        }
    }
    let raise = |to: Lane,
                 source: &str,
                 reason: String,
                 rows: Vec<String>,
                 applied: &mut Vec<Applied>,
                 lane: &mut Lane| {
        if to > *lane {
            *lane = to;
        }
        applied.push(Applied {
            lane: to,
            source: source.into(),
            reason,
            rows,
        });
    };
    // 2. Rules that raise.
    for rule in config.rules.iter().filter(|r| r.match_ == RuleMatch::Any) {
        if let Some(row) = rows.iter().find(|r| matches(rule, r))
            && rule.lane > lane
        {
            raise(
                rule.lane,
                "policy",
                format!("“{}” matches the rule for {}", row.title, describe(rule)),
                vec![row.id.clone()],
                &mut applied,
                &mut lane,
            );
        }
    }
    // 3. Built-in risk inputs: they only raise.
    let inputs = &config.inputs;
    if let Some(row) = rows
        .iter()
        .max_by_key(|r| (r.hints.dependent_components, std::cmp::Reverse(&r.id)))
        && let Some((to, at)) = threshold_met(&inputs.blast_radius, row.hints.dependent_components)
        && to > Lane::AutoMerge
    {
        // Reports from before `dependents` have only the count.
        let named = if row.hints.dependents.is_empty() {
            String::new()
        } else {
            format!(" ({})", some(&row.hints.dependents))
        };
        raise(
            to,
            "input",
            format!(
                "“{}” is used by {}{named}; {at} or more go to {}",
                row.title,
                plural(
                    row.hints.dependent_components,
                    "other component",
                    "other components"
                ),
                to.as_str()
            ),
            vec![row.id.clone()],
            &mut applied,
            &mut lane,
        );
    }
    let untested: Vec<&SemanticChange> = rows
        .iter()
        .filter(|r| r.hints.tested == Some(TestReach::Untested))
        .collect();
    if !untested.is_empty() && inputs.tests.untested > Lane::AutoMerge {
        let named: Vec<String> = untested
            .iter()
            .map(|r| match untested_file(r) {
                Some(f) => format!("“{}” (`{f}`)", r.title),
                None => format!("“{}”", r.title),
            })
            .collect();
        raise(
            inputs.tests.untested,
            "input",
            format!(
                "{} {} code no test exercises and no trace has observed: {}",
                plural(untested.len() as u32, "row", "rows"),
                if untested.len() == 1 {
                    "changes"
                } else {
                    "change"
                },
                some(&named)
            ),
            untested.iter().map(|r| r.id.clone()).collect(),
            &mut applied,
            &mut lane,
        );
    }
    let low: Vec<&SemanticChange> = rows
        .iter()
        .filter(|r| r.hints.confidence == Confidence::Low)
        .collect();
    if let Some(first) = low.first()
        && inputs.confidence.low > Lane::AutoMerge
    {
        let more = if low.len() > 1 {
            format!(
                " (and {} more)",
                plural(low.len() as u32 - 1, "row", "rows")
            )
        } else {
            String::new()
        };
        raise(
            inputs.confidence.low,
            "input",
            format!(
                "“{}”{more} rests on parts of the map Onus could not resolve",
                first.title
            ),
            low.iter().map(|r| r.id.clone()).collect(),
            &mut applied,
            &mut lane,
        );
    }
    let touched: BTreeSet<&str> = rows.iter().filter_map(area).collect();
    for component in touched {
        let Some(h) = ctx.areas.get(component) else {
            continue;
        };
        let to = [
            inputs.history.reverts.lane_for(h.reverts),
            inputs.history.incidents.lane_for(h.incidents),
        ]
        .into_iter()
        .flatten()
        .max();
        let Some(to) = to.filter(|l| *l > Lane::AutoMerge) else {
            continue;
        };
        let mut what = Vec::new();
        if h.incidents > 0 {
            what.push(plural(h.incidents, "incident", "incidents"));
        }
        if h.reverts > 0 {
            what.push(plural(h.reverts, "revert", "reverts"));
        }
        raise(
            to,
            "input",
            format!(
                "`{component}` had {} in the last {} days ({})",
                what.join(" and "),
                inputs.history.days,
                some(&h.changes)
            ),
            rows.iter()
                .filter(|r| area(r) == Some(component))
                .map(|r| r.id.clone())
                .collect(),
            &mut applied,
            &mut lane,
        );
    }
    // Changed lines tests ran, when the policy asks (ADR 0017). A changed
    // file with runtime code the coverage leaves out counts as not run, so
    // giving coverage never lowers a lane. An agent picks the commands it
    // runs in an environment, and one of them can write a coverage file, so
    // recorded coverage counts only from the commands the policy names. The
    // report's counts merge every source, so recorded coverage from any other
    // command voids them, and the change goes to the judge: dropping it to
    // "no coverage" would let an extra run hide an honest one.
    if lane == Lane::AutoMerge
        && let Some(policy) = &inputs.coverage
    {
        let uncounted: BTreeSet<String> = report
            .coverage
            .iter()
            .flat_map(|c| &c.sources)
            .filter(|s| {
                s.origin == CoverageOrigin::Recorded
                    && !s
                        .command
                        .as_ref()
                        .is_some_and(|c| policy.commands.contains(c))
            })
            .map(|s| {
                s.command
                    .as_deref()
                    .map_or("a command it does not name".into(), |c| format!("`{c}`"))
            })
            .collect();
        let uncounted: Vec<String> = uncounted.into_iter().collect();
        match &report.coverage {
            _ if !uncounted.is_empty() => raise(
                Lane::Judge,
                "input",
                format!(
                    "coverage was recorded by {}, which lanes.inputs.coverage.commands does not name; auto-merge needs all recorded coverage from named commands",
                    some(&uncounted)
                ),
                vec![],
                &mut applied,
                &mut lane,
            ),
            None if policy.required => raise(
                Lane::Judge,
                "input",
                "no coverage was given; auto-merge needs it (lanes.inputs.coverage.required)"
                    .into(),
                vec![],
                &mut applied,
                &mut lane,
            ),
            None => {}
            Some(c) => {
                if !c.not_measured.is_empty() {
                    let files: Vec<String> =
                        c.not_measured.iter().map(|f| format!("`{f}`")).collect();
                    raise(
                        Lane::Judge,
                        "input",
                        format!(
                            "the coverage leaves out {} with runtime code ({}); auto-merge needs every changed file measured",
                            plural(files.len() as u32, "changed file", "changed files"),
                            some(&files)
                        ),
                        rows.iter()
                            .filter(|r| {
                                r.locations.iter().any(|l| {
                                    l.side == onus_core::Side::Head
                                        && c.not_measured.contains(&l.file)
                                })
                            })
                            .map(|r| r.id.clone())
                            .collect(),
                        &mut applied,
                        &mut lane,
                    );
                }
                // In hundredths of a percent, so a share met exactly is met.
                let needed = (policy.min_changed * 10_000.0).round() as u64;
                if c.changed > 0 && u64::from(c.ran) * 10_000 < needed * u64::from(c.changed) {
                    raise(
                        Lane::Judge,
                        "input",
                        format!(
                            "tests ran {} of {} ({}%); auto-merge needs {}%",
                            c.ran,
                            plural(c.changed, "changed line", "changed lines"),
                            100 * u64::from(c.ran) / u64::from(c.changed),
                            percent(needed)
                        ),
                        rows.iter()
                            .filter(|r| r.hints.coverage.is_some_and(|c| c.ran < c.changed))
                            .map(|r| r.id.clone())
                            .collect(),
                        &mut applied,
                        &mut lane,
                    );
                }
            }
        }
    }
    // 4. Hard floors.
    if report.summary.secrets > 0 {
        raise(
            Lane::Blocked,
            "floor",
            "a secret is committed".into(),
            rows.iter()
                .filter(|r| r.subkind == "secret-committed")
                .map(|r| r.id.clone())
                .collect(),
            &mut applied,
            &mut lane,
        );
    }
    if let (Some(sub), Some(writable)) = (submission, ctx.writable) {
        let outside: Vec<&String> = sub.changed_files.iter().filter(|f| !writable(f)).collect();
        if !outside.is_empty() {
            raise(
                Lane::Blocked,
                "floor",
                format!(
                    "the change writes outside its token's scope: {}",
                    outside
                        .iter()
                        .take(5)
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                vec![],
                &mut applied,
                &mut lane,
            );
        }
    }
    for row in rows.iter().filter(|r| r.subkind == "test-weakened") {
        if !submission.is_some_and(|s| s.approved(&row.id)) {
            raise(
                Lane::Blocked,
                "floor",
                format!("“{}” without a person's approval", row.title),
                vec![row.id.clone()],
                &mut applied,
                &mut lane,
            );
        }
    }
    for row in rows
        .iter()
        .filter(|r| r.subkind == "behavior-newly-failing")
    {
        if !submission.is_some_and(|s| s.approved(&row.id)) {
            raise(
                Lane::Blocked,
                "floor",
                format!(
                    "“{}”: tests that pass at the base fail at the head",
                    row.title
                ),
                vec![row.id.clone()],
                &mut applied,
                &mut lane,
            );
        }
    }
    if let Some(row) = rows.iter().find(|r| !r.hints.labels.is_empty()) {
        raise(
            Lane::Human,
            "floor",
            format!(
                "“{}” is in sensitive code ({})",
                row.title,
                row.hints.labels.join(", ")
            ),
            vec![row.id.clone()],
            &mut applied,
            &mut lane,
        );
    }
    if let Some(row) = rows
        .iter()
        .find(|r| r.kind == ChangeKind::SecuritySensitive && r.hints.labels.is_empty())
    {
        raise(
            Lane::Human,
            "floor",
            format!("“{}” is security-sensitive", row.title),
            vec![row.id.clone()],
            &mut applied,
            &mut lane,
        );
    }
    if let Some(row) = rows.iter().find(|r| r.hints.rules_of_the_game) {
        raise(
            Lane::Human,
            "floor",
            format!("“{}” changes the rules of the game", row.title),
            vec![row.id.clone()],
            &mut applied,
            &mut lane,
        );
    }
    if let Some(row) = rows.iter().find(|r| r.hints.intent_mismatch) {
        raise(
            Lane::Human,
            "floor",
            format!("“{}” is outside the stated intent", row.title),
            vec![row.id.clone()],
            &mut applied,
            &mut lane,
        );
    }
    // Supply chain: third-party code new to the repository.
    let new_package = |r: &SemanticChange| {
        r.hints
            .novelty
            .iter()
            .filter_map(|n| n.strip_prefix("new-package:"))
            .map(|p| format!("`{p}`"))
            .collect::<Vec<_>>()
    };
    if let Some(row) = rows.iter().find(|r| !new_package(r).is_empty()) {
        raise(
            Lane::Human,
            "floor",
            format!(
                "“{}” brings third-party code new to this repository ({})",
                row.title,
                some(&new_package(row))
            ),
            vec![row.id.clone()],
            &mut applied,
            &mut lane,
        );
    }
    // A new dependency never auto-merges.
    if let Some(row) = rows.iter().find(|r| {
        matches!(
            r.subkind.as_str(),
            "new-dependency" | "new-cross-component-dependency"
        ) || r
            .hints
            .novelty
            .iter()
            .any(|n| n.starts_with("new-dependency:"))
    }) {
        raise(
            Lane::Judge,
            "floor",
            format!("“{}” adds a new dependency", row.title),
            vec![row.id.clone()],
            &mut applied,
            &mut lane,
        );
    }
    // 5. The learned model: it only raises, to the judge or a person, never
    // to blocked. Its entry is added only when it raises, so it never
    // crowds out a rule's reason; its view is always in `learned`.
    let mut learned = None;
    if let Some(l) = &ctx.learned {
        let p = l.model.predict(l.features);
        let to = if p.risk >= l.config.human {
            Lane::Human
        } else if p.risk >= l.config.judge {
            Lane::Judge
        } else {
            lane
        };
        let factors = l.model.factors(&p, l.features);
        let raised = to > lane;
        if raised {
            raise(
                to,
                "model",
                model_reason(&p, &factors, l),
                vec![],
                &mut applied,
                &mut lane,
            );
        }
        learned = Some(LearnedRisk {
            status: "applied".into(),
            model: Some(l.id.to_string()),
            risk: Some((p.risk * 1000.0).round() / 1000.0),
            judge_at: l.config.judge,
            human_at: l.config.human,
            raised,
            factors,
            note: None,
        });
    }
    // 6. Track record and random audits.
    if lane == Lane::AutoMerge && record.is_some_and(|r| r.recent_incidents > 0) {
        raise(
            Lane::Judge,
            "record",
            format!(
                "the agent setup caused {} incidents among its recent changes",
                record.map_or(0, |r| r.recent_incidents)
            ),
            vec![],
            &mut applied,
            &mut lane,
        );
    }
    if lane == Lane::AutoMerge && record.is_some_and(|r| r.recent_rollbacks > 0) {
        let n = record.map_or(0, |r| r.recent_rollbacks);
        raise(
            Lane::Judge,
            "record",
            format!(
                "the agent setup's changes were rolled back {} among its recent changes",
                plural(n, "time", "times")
            ),
            vec![],
            &mut applied,
            &mut lane,
        );
    }
    if lane == Lane::AutoMerge {
        let changes = record.map_or(0, |r| r.changes);
        if changes < config.min_record {
            raise(
                Lane::Judge,
                "record",
                format!(
                    "the agent setup has {changes} changes on record; auto-merge needs {}",
                    config.min_record
                ),
                vec![],
                &mut applied,
                &mut lane,
            );
        }
    }
    if lane == Lane::AutoMerge && audited(&report.head, config.audit_rate) {
        raise(
            Lane::Human,
            "audit",
            format!(
                "chosen for a random human audit ({:.0}% of auto-merges)",
                config.audit_rate * 100.0
            ),
            vec![],
            &mut applied,
            &mut lane,
        );
    }
    Classification {
        lane,
        applied,
        learned,
        features: None,
        rollout: None,
    }
}

/// "the learned model puts this change's risk at 0.91 (a person from 0.85,
/// the judge from 0.60): 2 breaking rows (+1.42), …; model 3f2a9c1e0b7d,
/// trained on 412 outcomes"
fn model_reason(p: &Prediction, factors: &[Factor], l: &Learned) -> String {
    let why: Vec<String> = factors
        .iter()
        .map(|f| format!("{} (+{:.2})", f.text, f.contribution))
        .collect();
    format!(
        "the learned model puts this change's risk at {:.2} (a person from {:.2}, the judge from {:.2}){}; model {}, trained on {} outcomes",
        p.risk,
        l.config.human,
        l.config.judge,
        if why.is_empty() {
            String::new()
        } else {
            format!(": {}", why.join(", "))
        },
        l.id,
        l.model.training.examples
    )
}

/// The first changed file of an untested row.
fn untested_file(r: &SemanticChange) -> Option<&str> {
    r.locations
        .iter()
        .find(|l| l.side == onus_core::Side::Head)
        .map(|l| l.file.as_str())
}

/// Whether the change at `head` is in the audited share: a hash of the
/// head, so the choice is reproducible but not known in advance.
pub fn audited(head: &str, rate: f64) -> bool {
    chosen("onus audit ", head, rate)
}

/// Whether a change approved without a person is sampled for an audit after
/// it merges: a hash of the merge commit, which no one knows before the merge.
pub fn audited_after_merge(commit: &str, rate: f64) -> bool {
    chosen("onus audit after merge ", commit, rate)
}

/// Whether `key` falls in the `rate` share, by a hash salted with `salt`, so
/// draws with different salts are independent.
fn chosen(salt: &str, key: &str, rate: f64) -> bool {
    if rate <= 0.0 {
        return false;
    }
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(key.as_bytes());
    let bytes = h.finalize();
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    (n as f64 / u32::MAX as f64) < rate
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_core::{ChangeLevel, Hints, ReportSummary, StructureNotes, TextStats};

    fn row(kind: ChangeKind, subkind: &str, labels: &[&str]) -> SemanticChange {
        SemanticChange {
            id: format!("{subkind}:x"),
            kind,
            subkind: subkind.into(),
            level: ChangeLevel::Structure,
            subject: "x".into(),
            component: Some("docs".into()),
            kind_label: String::new(),
            title: subkind.into(),
            why_it_matters: String::new(),
            hints: Hints {
                labels: labels.iter().map(|s| s.to_string()).collect(),
                blast_radius: 0,
                novelty: vec![],
                confidence: Confidence::Static,
                rules_of_the_game: false,
                intent_mismatch: false,
                needs_person: false,
                dependent_components: 0,
                dependents: vec![],
                tested: None,
                owners: vec![],
                coverage: None,
            },
            locations: vec![],
            stats: None,
        }
    }

    fn report(rows: Vec<SemanticChange>) -> SemanticReport {
        SemanticReport {
            schema_version: 1,
            base: "base".into(),
            head: "head-1".into(),
            summary: ReportSummary {
                meaning_changes: rows.len() as u32,
                needs_attention: 0,
                secrets: 0,
                new_rule_violations: 0,
                intent_mismatches: 0,
            },
            changes: rows,
            intent_check: None,
            rule_violations: vec![],
            structure: StructureNotes::default(),
            text_stats: TextStats::default(),
            map_diagnostics: vec![],
            coverage: None,
            behavior: None,
        }
    }

    fn policy() -> LanesConfig {
        LanesConfig {
            rules: vec![
                LaneRule {
                    lane: Lane::AutoMerge,
                    match_: RuleMatch::Every,
                    kinds: vec!["internal".into()],
                    subkinds: vec![],
                    components: vec!["docs".into()],
                    labels: vec![],
                },
                LaneRule {
                    lane: Lane::Human,
                    match_: RuleMatch::Any,
                    kinds: vec![],
                    subkinds: vec!["migration-changed".into()],
                    components: vec![],
                    labels: vec![],
                },
            ],
            audit_rate: 0.0,
            ..LanesConfig::default()
        }
    }

    fn with(record: Option<Record>) -> ClassifyContext<'static> {
        ClassifyContext {
            record,
            ..ClassifyContext::default()
        }
    }

    const VETERAN: Option<Record> = Some(Record {
        changes: 50,
        recent_incidents: 0,
        recent_rollbacks: 0,
    });

    fn lane_of(r: &SemanticReport, p: &LanesConfig) -> Lane {
        classify(r, None, p, &with(VETERAN)).lane
    }

    fn internal(f: impl FnOnce(&mut Hints)) -> SemanticReport {
        let mut r = row(ChangeKind::Internal, "internal-changes", &[]);
        f(&mut r.hints);
        report(vec![r])
    }

    fn inputs<'a>(c: &'a Classification, source: &str) -> Vec<&'a Applied> {
        c.applied.iter().filter(|a| a.source == source).collect()
    }

    #[test]
    fn rules_lower_only_for_whole_changes_and_floors_always_raise() {
        let p = policy();
        let docs = report(vec![row(ChangeKind::Internal, "internal-changes", &[])]);
        assert_eq!(lane_of(&docs, &p), Lane::AutoMerge);
        // A new setup cannot auto-merge yet.
        assert_eq!(
            classify(&docs, None, &p, &ClassifyContext::default()).lane,
            Lane::Judge
        );
        // One row outside the rule keeps the default.
        let mixed = report(vec![
            row(ChangeKind::Internal, "internal-changes", &[]),
            row(ChangeKind::Additive, "export-added", &[]),
        ]);
        assert_eq!(lane_of(&mixed, &p), Lane::Judge);
        // Policy raises; floors raise further.
        let migration = report(vec![row(ChangeKind::Config, "migration-changed", &[])]);
        assert_eq!(lane_of(&migration, &p), Lane::Human);
        let payments = report(vec![row(
            ChangeKind::Internal,
            "internal-changes",
            &["payments"],
        )]);
        let c = classify(&payments, None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Human);
        assert!(c.applied.iter().any(|a| a.reason.contains("payments")));
        assert_eq!(inputs(&c, "floor")[0].rows, ["internal-changes:x"]);
        let mut secret = docs.clone();
        secret.summary.secrets = 1;
        assert_eq!(lane_of(&secret, &p), Lane::Blocked);
        let weakened = report(vec![row(ChangeKind::Test, "test-weakened", &[])]);
        assert_eq!(lane_of(&weakened, &p), Lane::Blocked);
    }

    #[test]
    fn newly_failing_tests_block_unless_approved() {
        use crate::submission::{AgentSetup, Approval};
        let p = policy();
        let failing = report(vec![row(
            ChangeKind::Internal,
            "behavior-newly-failing",
            &[],
        )]);
        let c = classify(&failing, None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Blocked);
        let floor = inputs(&c, "floor");
        assert_eq!(floor[0].rows, ["behavior-newly-failing:x"]);
        assert!(
            floor[0]
                .reason
                .contains("tests that pass at the base fail at the head"),
            "{}",
            floor[0].reason
        );
        // A person's approval of the row lifts the block: the lane falls back
        // to the policy's (internal rows in `docs` auto-merge).
        let sub = Submission {
            schema: 1,
            base: "b".into(),
            head: "h".into(),
            intent: None,
            report: failing.clone(),
            changed_files: vec![],
            evidence: vec![],
            scope: None,
            escalations: vec![],
            approvals: vec![Approval {
                row: "behavior-newly-failing:x".into(),
                by: "@lead".into(),
            }],
            agent: AgentSetup::default(),
        };
        let c = classify(&failing, Some(&sub), &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::AutoMerge);
        assert!(inputs(&c, "floor").is_empty());
        // Behavior pinned by an edited test goes to a person, through the
        // rules of the game.
        let mut changed = row(ChangeKind::Test, "behavior-changed", &[]);
        changed.hints.rules_of_the_game = true;
        let c = classify(&report(vec![changed]), None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Human);
        assert!(
            inputs(&c, "floor")[0]
                .reason
                .contains("changes the rules of the game")
        );
    }

    #[test]
    fn blast_radius_tests_and_confidence_raise_lanes() {
        let p = policy();
        let wide = internal(|h| {
            h.dependent_components = 4;
            h.dependents = ["billing", "notifications", "orders", "web"]
                .map(String::from)
                .into();
        });
        let c = classify(&wide, None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Judge);
        let input = inputs(&c, "input");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].rows, ["internal-changes:x"]);
        assert!(
            input[0].reason.contains(
                "is used by 4 other components (billing, notifications, orders and 1 more); 3 or more go to judge"
            ),
            "{}",
            input[0].reason
        );
        // A report from before `dependents` has only the count.
        let counted = internal(|h| h.dependent_components = 3);
        let c = classify(&counted, None, &p, &with(VETERAN));
        assert!(
            inputs(&c, "input")[0]
                .reason
                .contains("is used by 3 other components; 3 or more go to judge"),
            "{c:?}"
        );
        assert_eq!(
            lane_of(&internal(|h| h.dependent_components = 2), &p),
            Lane::AutoMerge
        );
        let mut off = policy();
        off.inputs.blast_radius = Thresholds::default();
        assert_eq!(lane_of(&wide, &off), Lane::AutoMerge);

        let untested = internal(|h| h.tested = Some(TestReach::Untested));
        let c = classify(&untested, None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Judge);
        assert!(inputs(&c, "input")[0].reason.contains("no test exercises"));
        assert_eq!(
            lane_of(&internal(|h| h.tested = Some(TestReach::Tested)), &p),
            Lane::AutoMerge
        );
        assert_eq!(
            lane_of(&internal(|h| h.tested = Some(TestReach::Traced)), &p),
            Lane::AutoMerge
        );
        let mut off = policy();
        off.inputs.tests.untested = Lane::AutoMerge;
        let c = classify(&untested, None, &off, &with(VETERAN));
        assert_eq!(c.lane, Lane::AutoMerge);
        assert!(inputs(&c, "input").is_empty());

        let low = internal(|h| h.confidence = Confidence::Low);
        let c = classify(&low, None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Judge);
        assert!(inputs(&c, "input")[0].reason.contains("could not resolve"));
    }

    #[test]
    fn history_of_the_touched_components_raises_lanes() {
        let p = policy();
        let docs = report(vec![row(ChangeKind::Internal, "internal-changes", &[])]);
        let lane_with = |component: &str, h: AreaHistory| {
            let areas: AreaHistories = [(component.to_string(), h)].into();
            let ctx = ClassifyContext {
                areas: &areas,
                ..with(VETERAN)
            };
            classify(&docs, None, &p, &ctx)
        };
        let incident = AreaHistory {
            incidents: 1,
            changes: vec!["acme/shop#1".into()],
            ..AreaHistory::default()
        };
        let c = lane_with("docs", incident.clone());
        assert_eq!(c.lane, Lane::Human);
        assert_eq!(
            inputs(&c, "input")[0].reason,
            "`docs` had 1 incident in the last 90 days (acme/shop#1)"
        );
        let revert = AreaHistory {
            reverts: 1,
            changes: vec!["acme/shop#2".into()],
            ..AreaHistory::default()
        };
        assert_eq!(lane_with("docs", revert).lane, Lane::Judge);
        assert_eq!(lane_with("billing", incident).lane, Lane::AutoMerge);
    }

    #[test]
    fn rows_outside_every_component_have_root_history() {
        let mut root = row(ChangeKind::Internal, "internal-changes", &[]);
        root.component = None;
        let mut spread = root.clone();
        spread.subject = "repository".into();
        assert_eq!(area(&root), Some("root"));
        assert_eq!(area(&spread), None);
        let areas: AreaHistories = [(
            "root".to_string(),
            AreaHistory {
                incidents: 1,
                changes: vec!["acme/shop#1".into()],
                ..AreaHistory::default()
            },
        )]
        .into();
        let ctx = ClassifyContext {
            areas: &areas,
            ..with(VETERAN)
        };
        let c = classify(&report(vec![root]), None, &LanesConfig::default(), &ctx);
        assert_eq!(c.lane, Lane::Human);
        assert!(
            inputs(&c, "input")[0]
                .reason
                .starts_with("`root` had 1 incident")
        );
    }

    #[test]
    fn new_packages_and_dependencies_have_floors() {
        let mut p = policy();
        p.rules.push(LaneRule {
            lane: Lane::AutoMerge,
            match_: RuleMatch::Every,
            kinds: vec!["dependency".into(), "internal".into()],
            subkinds: vec![],
            components: vec![],
            labels: vec![],
        });
        let mut pkg = row(ChangeKind::Dependency, "new-dependency", &[]);
        pkg.hints.novelty = vec!["new-package:left-pad".into()];
        let c = classify(&report(vec![pkg]), None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Human);
        assert!(c.applied.iter().any(
            |a| a.source == "floor" && a.reason.contains("new to this repository (`left-pad`)")
        ));
        let existing = row(ChangeKind::Dependency, "new-dependency", &[]);
        assert_eq!(lane_of(&report(vec![existing]), &p), Lane::Judge);
        let cross = row(ChangeKind::Internal, "new-cross-component-dependency", &[]);
        assert_eq!(lane_of(&report(vec![cross]), &p), Lane::Judge);
        let mut sdk = row(ChangeKind::Internal, "internal-changes", &[]);
        sdk.hints.novelty = vec!["new-dependency:@acme/sms".into()];
        assert_eq!(lane_of(&report(vec![sdk]), &p), Lane::Judge);
    }

    #[test]
    fn inputs_never_lower_a_lane() {
        let mut off = policy();
        off.inputs = onus_core::RiskInputs {
            blast_radius: Thresholds::default(),
            tests: onus_core::TestsInput {
                untested: Lane::AutoMerge,
            },
            confidence: onus_core::ConfidenceInput {
                low: Lane::AutoMerge,
            },
            history: onus_core::HistoryInput {
                reverts: Thresholds::default(),
                incidents: Thresholds::default(),
                ..Default::default()
            },
            coverage: None,
        };
        let areas: AreaHistories = [(
            "docs".to_string(),
            AreaHistory {
                reverts: 2,
                incidents: 1,
                changes: vec!["acme/shop#1".into()],
                ..AreaHistory::default()
            },
        )]
        .into();
        let reports = [
            internal(|_| {}),
            internal(|h| h.dependent_components = 12),
            internal(|h| h.tested = Some(TestReach::Untested)),
            internal(|h| h.confidence = Confidence::Low),
            report(vec![row(ChangeKind::Config, "migration-changed", &[])]),
            report(vec![row(
                ChangeKind::Internal,
                "internal-changes",
                &["payments"],
            )]),
        ];
        for history in [&AreaHistories::new(), &areas] {
            let ctx = ClassifyContext {
                areas: history,
                ..with(VETERAN)
            };
            for r in &reports {
                let on = classify(r, None, &policy(), &ctx).lane;
                assert!(on >= classify(r, None, &off, &ctx).lane);
                let mut human = policy();
                human.default = Lane::Human;
                human.rules.clear();
                assert!(classify(r, None, &human, &ctx).lane >= Lane::Human);
            }
        }
    }

    #[test]
    fn coverage_policy_raises_auto_merge() {
        use onus_core::{CoveragePolicy, LineCoverage, Location, ReportCoverage};
        let mut p = policy();
        p.inputs.coverage = Some(CoveragePolicy {
            min_changed: 0.8,
            required: false,
            commands: vec![],
        });
        let with_coverage = |changed, ran| {
            let mut r = internal(|h| h.coverage = Some(LineCoverage { changed, ran }));
            r.coverage = Some(ReportCoverage {
                sources: vec![],
                changed,
                ran,
                not_run: vec![],
                not_measured: vec![],
                notes: vec![],
            });
            r
        };
        let c = classify(&with_coverage(10, 3), None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Judge);
        let input = inputs(&c, "input");
        assert_eq!(
            input[0].reason,
            "tests ran 3 of 10 changed lines (30%); auto-merge needs 80%"
        );
        assert_eq!(input[0].rows, ["internal-changes:x"]);
        assert_eq!(lane_of(&with_coverage(10, 9), &p), Lane::AutoMerge);
        // The share is rounded down, like the report's.
        let mut strict = p.clone();
        strict.inputs.coverage = Some(CoveragePolicy {
            min_changed: 0.9,
            required: false,
            commands: vec![],
        });
        let c = classify(&with_coverage(500, 448), None, &strict, &with(VETERAN));
        assert_eq!(
            inputs(&c, "input")[0].reason,
            "tests ran 448 of 500 changed lines (89%); auto-merge needs 90%"
        );
        // A share met exactly is met, despite floating point.
        for (share, changed, ran) in [(0.55, 100, 55), (0.07, 100, 7), (0.14, 50, 7)] {
            let mut exact = p.clone();
            exact.inputs.coverage.as_mut().unwrap().min_changed = share;
            assert_eq!(
                lane_of(&with_coverage(changed, ran), &exact),
                Lane::AutoMerge,
                "{share}"
            );
        }
        let mut half = p.clone();
        half.inputs.coverage.as_mut().unwrap().min_changed = 0.555;
        let c = classify(&with_coverage(1, 0), None, &half, &with(VETERAN));
        assert_eq!(
            inputs(&c, "input")[0].reason,
            "tests ran 0 of 1 changed line (0%); auto-merge needs 55.5%"
        );
        // Recorded coverage counts only from the commands the policy names:
        // an agent picks its commands, and one can write a coverage file.
        let recorded = |command: &str| {
            let mut r = with_coverage(10, 10);
            r.coverage.as_mut().unwrap().sources = vec![onus_core::CoverageSource {
                format: onus_core::CoverageFormat::Lcov,
                path: "coverage/lcov.info".into(),
                sha256: "0".repeat(64),
                origin: CoverageOrigin::Recorded,
                manifest: Some("1".repeat(64)),
                command: Some(command.into()),
                files: 1,
                unmatched: 0,
            }];
            r
        };
        let mut named = p.clone();
        named.inputs.coverage = Some(CoveragePolicy {
            min_changed: 0.8,
            required: true,
            commands: vec!["npm test -- --coverage".into()],
        });
        assert_eq!(
            lane_of(&recorded("npm test -- --coverage"), &named),
            Lane::AutoMerge
        );
        let c = classify(
            &recorded("sh -c 'cp lcov.info coverage/'"),
            None,
            &named,
            &with(VETERAN),
        );
        assert_eq!(c.lane, Lane::Judge);
        assert_eq!(
            inputs(&c, "input")[0].reason,
            "coverage was recorded by `sh -c 'cp lcov.info coverage/'`, which lanes.inputs.coverage.commands does not name; auto-merge needs all recorded coverage from named commands"
        );
        // Required or not: an extra run cannot hide an honest one. Named
        // coverage that sends the change to the judge keeps it there when an
        // unnamed run adds its own.
        named.inputs.coverage.as_mut().unwrap().required = false;
        assert_eq!(lane_of(&recorded("sh"), &named), Lane::Judge);
        let mut low = recorded("npm test -- --coverage");
        low.coverage.as_mut().unwrap().ran = 0;
        low.changes[0].hints.coverage = Some(LineCoverage {
            changed: 10,
            ran: 0,
        });
        assert_eq!(lane_of(&low, &named), Lane::Judge);
        let cov = low.coverage.as_mut().unwrap();
        let mut forged = cov.sources[0].clone();
        forged.command = Some("true".into());
        cov.sources.push(forged);
        cov.ran = 10;
        let c = classify(&low, None, &named, &with(VETERAN));
        assert_eq!(c.lane, Lane::Judge);
        assert!(inputs(&c, "input")[0].reason.contains("`true`"));
        // It runs only while the change would auto-merge.
        let mut judged = p.clone();
        judged.default = Lane::Judge;
        judged.rules.clear();
        let c = classify(&with_coverage(10, 3), None, &judged, &with(VETERAN));
        assert!(inputs(&c, "input").is_empty(), "{:?}", c.applied);
        // No changed executable lines, or no coverage, passes unless required.
        assert_eq!(lane_of(&with_coverage(0, 0), &p), Lane::AutoMerge);
        assert_eq!(lane_of(&internal(|_| {}), &p), Lane::AutoMerge);
        // A changed runtime file the coverage leaves out counts as not run:
        // coverage of other files cannot let it through.
        let mut unmeasured = with_coverage(0, 0);
        unmeasured.changes[0].locations = vec![Location::head("src/eta.ts", 1, 1)];
        unmeasured.coverage.as_mut().unwrap().not_measured = vec!["src/eta.ts".into()];
        let c = classify(&unmeasured, None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Judge);
        let input = inputs(&c, "input");
        assert_eq!(
            input[0].reason,
            "the coverage leaves out 1 changed file with runtime code (`src/eta.ts`); auto-merge needs every changed file measured"
        );
        assert_eq!(input[0].rows, ["internal-changes:x"]);
        p.inputs.coverage = Some(CoveragePolicy {
            min_changed: 0.0,
            required: true,
            commands: vec![],
        });
        let c = classify(&internal(|_| {}), None, &p, &with(VETERAN));
        assert_eq!(c.lane, Lane::Judge);
        assert!(
            inputs(&c, "input")[0]
                .reason
                .contains("no coverage was given")
        );
        assert_eq!(lane_of(&with_coverage(10, 0), &p), Lane::AutoMerge);
        assert_eq!(lane_of(&unmeasured, &p), Lane::Judge);
        // Off unless written.
        assert_eq!(lane_of(&with_coverage(10, 0), &policy()), Lane::AutoMerge);
        // It never lowers a lane.
        let mut human = p.clone();
        human.default = Lane::Human;
        human.rules.clear();
        assert_eq!(lane_of(&with_coverage(10, 10), &human), Lane::Human);
        let mut secret = with_coverage(10, 10);
        secret.summary.secrets = 1;
        assert_eq!(lane_of(&secret, &p), Lane::Blocked);
    }

    fn model(bias: f64, weights: &[(&str, f64)]) -> Model {
        use crate::learned::{FEATURE_SET, FEATURES, Holdout, Rates, SCHEMA, Training};
        let d = FEATURES.len();
        let none = || Rates {
            raised: None,
            recall: None,
        };
        Model {
            schema: SCHEMA,
            feature_set: FEATURE_SET,
            features: FEATURES.iter().map(|f| f.to_string()).collect(),
            mean: vec![0.0; d],
            scale: vec![1.0; d],
            weights: FEATURES
                .iter()
                .map(|f| weights.iter().find(|w| w.0 == *f).map_or(0.0, |w| w.1))
                .collect(),
            bias,
            training: Training {
                examples: 412,
                positives: 23,
                skipped: BTreeMap::new(),
                settle_days: 14,
                through: 0,
                outcomes_sha256: String::new(),
                l2: 0.01,
                iterations: 3000,
                learning_rate: 0.5,
            },
            holdout: Holdout {
                examples: 0,
                positives: 0,
                auc: None,
                at_judge: none(),
                at_human: none(),
            },
        }
    }

    fn with_model(
        r: &SemanticReport,
        p: &LanesConfig,
        m: &Model,
        config: &LearnedConfig,
    ) -> Classification {
        let features = crate::learned::features(r, None, &crate::learned::History::default());
        let ctx = ClassifyContext {
            learned: Some(Learned {
                model: m,
                id: "3f2a9c1e0b7d",
                config,
                features: &features,
            }),
            ..with(VETERAN)
        };
        classify(r, None, p, &ctx)
    }

    #[test]
    fn a_model_only_raises() {
        let p = policy();
        let mut secret = report(vec![row(ChangeKind::Internal, "internal-changes", &[])]);
        secret.summary.secrets = 1;
        let reports = [
            report(vec![row(ChangeKind::Internal, "internal-changes", &[])]),
            report(vec![
                row(ChangeKind::Internal, "internal-changes", &[]),
                row(ChangeKind::Additive, "export-added", &[]),
            ]),
            report(vec![row(
                ChangeKind::Internal,
                "internal-changes",
                &["payments"],
            )]),
            secret,
        ];
        let models = [
            model(-20.0, &[]),
            model(0.0, &[]),
            model(20.0, &[]),
            model(-1.0, &[("rows", 3.0), ("kind.internal", -2.0)]),
        ];
        let thresholds = [(0.6, 0.85), (0.1, 0.2), (0.5, 0.5), (1.0, 1.0)];
        let rules_and_floors = |c: &Classification| -> Vec<Applied> {
            c.applied
                .iter()
                .filter(|a| ["default", "policy", "input", "floor"].contains(&a.source.as_str()))
                .cloned()
                .collect()
        };
        for r in &reports {
            let without = classify(r, None, &p, &with(VETERAN));
            for m in &models {
                for (judge, human) in thresholds {
                    let config = LearnedConfig {
                        judge,
                        human,
                        ..LearnedConfig::default()
                    };
                    let c = with_model(r, &p, m, &config);
                    assert!(c.lane >= without.lane, "{c:?}");
                    if without.lane != Lane::Blocked {
                        assert_ne!(c.lane, Lane::Blocked);
                    }
                    assert_eq!(rules_and_floors(&c), rules_and_floors(&without));
                    let l = c.learned.unwrap();
                    assert_eq!(l.status, "applied");
                    assert_eq!(l.raised, c.applied.iter().any(|a| a.source == "model"));
                }
            }
        }
    }

    #[test]
    fn the_model_raises_at_its_thresholds() {
        let p = policy();
        let docs = report(vec![row(ChangeKind::Internal, "internal-changes", &[])]);
        let config = LearnedConfig::default();
        // One internal row adds ln 2 to the logit; the bias sets the risk.
        let at = |risk: f64| {
            let internal =
                crate::learned::features(&docs, None, &Default::default()).get("kind.internal");
            let bias = (risk / (1.0 - risk)).ln() - internal;
            with_model(&docs, &p, &model(bias, &[("kind.internal", 1.0)]), &config)
        };
        let c = at(0.7);
        assert_eq!(c.lane, Lane::Judge);
        let m = inputs(&c, "model");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].lane, Lane::Judge);
        assert!(m[0].reason.contains("0.70"), "{}", m[0].reason);
        assert!(
            m[0].reason.contains("1 internal row (+0.69)"),
            "{}",
            m[0].reason
        );
        assert!(
            m[0].reason
                .contains("(a person from 0.85, the judge from 0.60)"),
            "{}",
            m[0].reason
        );
        assert!(
            m[0].reason
                .ends_with("model 3f2a9c1e0b7d, trained on 412 outcomes"),
            "{}",
            m[0].reason
        );
        let l = c.learned.unwrap();
        assert!(l.raised);
        assert_eq!(l.factors[0].text, "1 internal row");
        assert_eq!(at(0.9).lane, Lane::Human);
        let low = at(0.1);
        assert_eq!(low.lane, Lane::AutoMerge);
        assert!(inputs(&low, "model").is_empty());
        let l = low.learned.unwrap();
        assert_eq!((l.risk, l.raised), (Some(0.1), false));
        // Already with a person: nothing to raise.
        let payments = report(vec![row(
            ChangeKind::Internal,
            "internal-changes",
            &["payments"],
        )]);
        let c = with_model(&payments, &p, &model(20.0, &[]), &config);
        assert_eq!(c.lane, Lane::Human);
        assert!(inputs(&c, "model").is_empty());
    }

    #[test]
    fn a_setup_whose_changes_were_rolled_back_does_not_auto_merge() {
        let p = policy();
        let docs = report(vec![row(ChangeKind::Internal, "internal-changes", &[])]);
        let rolled = Some(Record {
            recent_rollbacks: 1,
            ..VETERAN.unwrap()
        });
        let c = classify(&docs, None, &p, &with(rolled));
        assert_eq!(c.lane, Lane::Judge);
        let record = inputs(&c, "record");
        assert_eq!(record.len(), 1);
        assert_eq!(
            record[0].reason,
            "the agent setup's changes were rolled back 1 time among its recent changes"
        );
        // A floor stays where it is.
        let payments = report(vec![row(
            ChangeKind::Internal,
            "internal-changes",
            &["payments"],
        )]);
        let c = classify(&payments, None, &p, &with(rolled));
        assert_eq!(c.lane, Lane::Human);
        assert!(inputs(&c, "record").is_empty());
        // The plan is attached by classify_with, not here.
        assert!(c.rollout.is_none());
    }

    #[test]
    fn audits_are_a_deterministic_share() {
        let n = (0..10_000)
            .filter(|i| audited(&format!("{i:040}"), 0.05))
            .count();
        assert!((400..600).contains(&n), "{n}");
        assert_eq!(audited("abc", 0.05), audited("abc", 0.05));
        assert!(!audited("abc", 0.0));
        // Pinned before `audited` shared its hash: lane decisions stay put.
        let one = format!("{:040}", 1);
        assert!(!audited("aaaa", 0.5));
        assert!(!audited("bbbb", 0.5));
        assert!(audited(&one, 0.5));
    }

    #[test]
    fn audits_after_merge_are_their_own_draw() {
        assert!(!audited_after_merge("aaaa", 0.0));
        assert!(audited_after_merge("aaaa", 1.0));
        let differ = (0..100)
            .map(|i| format!("{i:040}"))
            .any(|k| audited(&k, 0.5) != audited_after_merge(&k, 0.5));
        assert!(differ);
        assert!(audited_after_merge("aaaa", 0.5) && !audited("aaaa", 0.5));
    }
}
