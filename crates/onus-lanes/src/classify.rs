//! The rules-first classifier: which lane a change takes, and why.
//!
//! 1. Start at the policy's default lane, or lower when a `match: every`
//!    rule covers every row.
//! 2. Raise it with every other rule that applies.
//! 3. Raise it with the hard floors, which no policy can lower: sensitive
//!    code and rules of the game go to a person; committed secrets, writes
//!    outside the token's scope and weakened tests without an approval are
//!    blocked.
//! 4. A setup with too short a record may not auto-merge, and a share of
//!    the remaining auto-merges is audited by a person.

use onus_core::{
    ChangeKind, Lane, LaneRule, LanesConfig, RuleMatch, SemanticChange, SemanticReport,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::submission::Submission;

/// One reason the lane is what it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub lane: Lane,
    /// `default`, `policy`, `floor`, `record` or `audit`.
    pub source: String,
    pub reason: String,
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
    /// Changes on record for this setup.
    pub changes: u32,
}

/// Classifies a change. `writable` says whether the submission's token
/// allows writing a path (`None` when there is no token).
pub fn classify(
    report: &SemanticReport,
    submission: Option<&Submission>,
    config: &LanesConfig,
    record: Option<Record>,
    writable: Option<&dyn Fn(&str) -> bool>,
) -> Classification {
    let mut applied = Vec::new();
    let mut lane = config.default;
    applied.push(Applied {
        lane,
        source: "default".into(),
        reason: format!("the policy's default lane is {}", lane.as_str()),
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
            });
        }
    }
    let raise =
        |to: Lane, source: &str, reason: String, applied: &mut Vec<Applied>, lane: &mut Lane| {
            if to > *lane {
                *lane = to;
            }
            applied.push(Applied {
                lane: to,
                source: source.into(),
                reason,
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
                format!("`{}` matches the rule for {}", row.title, describe(rule)),
                &mut applied,
                &mut lane,
            );
        }
    }
    // 3. Hard floors.
    if report.summary.secrets > 0 {
        raise(
            Lane::Blocked,
            "floor",
            "a secret is committed".into(),
            &mut applied,
            &mut lane,
        );
    }
    if let (Some(sub), Some(writable)) = (submission, writable) {
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
                format!("`{}` without a person's approval", row.title),
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
                "`{}` is in sensitive code ({})",
                row.title,
                row.hints.labels.join(", ")
            ),
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
            format!("`{}` is security-sensitive", row.title),
            &mut applied,
            &mut lane,
        );
    }
    if let Some(row) = rows.iter().find(|r| r.hints.rules_of_the_game) {
        raise(
            Lane::Human,
            "floor",
            format!("`{}` changes the rules of the game", row.title),
            &mut applied,
            &mut lane,
        );
    }
    if let Some(row) = rows.iter().find(|r| r.hints.intent_mismatch) {
        raise(
            Lane::Human,
            "floor",
            format!("`{}` is outside the stated intent", row.title),
            &mut applied,
            &mut lane,
        );
    }
    // 4. Track record and random audits.
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
            &mut applied,
            &mut lane,
        );
    }
    Classification { lane, applied }
}

/// Whether the change at `head` is in the audited share: a hash of the
/// head, so the choice is reproducible but not known in advance.
pub fn audited(head: &str, rate: f64) -> bool {
    if rate <= 0.0 {
        return false;
    }
    let mut h = Sha256::new();
    h.update(b"onus audit ");
    h.update(head.as_bytes());
    let bytes = h.finalize();
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    (n as f64 / u32::MAX as f64) < rate
}

#[cfg(test)]
mod tests {
    use super::*;
    use onus_core::{ChangeLevel, Confidence, Hints, ReportSummary, StructureNotes, TextStats};

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

    #[test]
    fn rules_lower_only_for_whole_changes_and_floors_always_raise() {
        let p = policy();
        let docs = report(vec![row(ChangeKind::Internal, "internal-changes", &[])]);
        let veteran = Some(Record { changes: 50 });
        assert_eq!(
            classify(&docs, None, &p, veteran, None).lane,
            Lane::AutoMerge
        );
        // A new setup cannot auto-merge yet.
        assert_eq!(classify(&docs, None, &p, None, None).lane, Lane::Judge);
        // One row outside the rule keeps the default.
        let mixed = report(vec![
            row(ChangeKind::Internal, "internal-changes", &[]),
            row(ChangeKind::Additive, "export-added", &[]),
        ]);
        assert_eq!(classify(&mixed, None, &p, veteran, None).lane, Lane::Judge);
        // Policy raises; floors raise further.
        let migration = report(vec![row(ChangeKind::Config, "migration-changed", &[])]);
        assert_eq!(
            classify(&migration, None, &p, veteran, None).lane,
            Lane::Human
        );
        let payments = report(vec![row(
            ChangeKind::Internal,
            "internal-changes",
            &["payments"],
        )]);
        let c = classify(&payments, None, &p, veteran, None);
        assert_eq!(c.lane, Lane::Human);
        assert!(c.applied.iter().any(|a| a.reason.contains("payments")));
        let mut secret = docs.clone();
        secret.summary.secrets = 1;
        assert_eq!(
            classify(&secret, None, &p, veteran, None).lane,
            Lane::Blocked
        );
        let weakened = report(vec![row(ChangeKind::Test, "test-weakened", &[])]);
        assert_eq!(
            classify(&weakened, None, &p, veteran, None).lane,
            Lane::Blocked
        );
    }

    #[test]
    fn audits_are_a_deterministic_share() {
        let n = (0..10_000)
            .filter(|i| audited(&format!("{i:040}"), 0.05))
            .count();
        assert!((400..600).contains(&n), "{n}");
        assert_eq!(audited("abc", 0.05), audited("abc", 0.05));
        assert!(!audited("abc", 0.0));
    }
}
