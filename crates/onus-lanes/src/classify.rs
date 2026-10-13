//! The rules-first classifier: which lane a change takes, and why.
//!
//! 1. Start at the policy's default lane, or lower when a `match: every`
//!    rule covers every row.
//! 2. Raise it with every other rule that applies.
//! 3. Raise it with the built-in risk inputs (`lanes.inputs`): blast
//!    radius, untested code, low map confidence and the history of the
//!    components the change touches. They only raise (ADR 0014).
//! 4. Raise it with the hard floors, which no policy can lower: sensitive
//!    code, rules of the game and third-party code new to the repository go
//!    to a person; a new dependency never auto-merges; committed secrets,
//!    writes outside the token's scope and weakened tests without an
//!    approval are blocked.
//! 5. A setup with too short a record may not auto-merge, and a share of
//!    the remaining auto-merges is audited by a person.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::{
    ChangeKind, Confidence, Lane, LaneRule, LanesConfig, RuleMatch, SemanticChange, SemanticReport,
    TestReach, Thresholds,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::submission::Submission;

/// One reason the lane is what it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub lane: Lane,
    /// `default`, `policy`, `input`, `floor`, `record` or `audit`.
    pub source: String,
    pub reason: String,
    /// The ids of the report rows behind it, when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub lane: Lane,
    /// Every rule that applied, in the order it was applied.
    pub applied: Vec<Applied>,
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
}

/// Reverts (rollbacks included) and incidents recorded against one
/// component in the history window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AreaHistory {
    pub reverts: u32,
    pub incidents: u32,
    /// The changes behind them, sorted.
    pub changes: Vec<String>,
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
}

impl std::fmt::Debug for ClassifyContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClassifyContext")
            .field("record", &self.record)
            .field("areas", &self.areas)
            .field("writable", &self.writable.is_some())
            .finish()
    }
}

impl Default for ClassifyContext<'_> {
    fn default() -> Self {
        ClassifyContext {
            record: None,
            areas: &NO_HISTORY,
            writable: None,
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
    // 5. Track record and random audits.
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
    Classification { lane, applied }
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
        };
        let areas: AreaHistories = [(
            "docs".to_string(),
            AreaHistory {
                reverts: 2,
                incidents: 1,
                changes: vec!["acme/shop#1".into()],
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
