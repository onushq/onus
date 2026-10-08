//! Ranking of semantic changes (PLAN.md section 5.3).
//!
//! Intent mismatches come first, then rows that need a person, and within
//! each of those groups: security-sensitive changes, breaking
//! changes, dependency and novelty, rules of the game, config, additive
//! contracts, new relationships, tests and finally internal changes. Ties
//! break on low confidence (unknowns rank as higher risk), then sensitivity
//! labels, then blast radius, then id.

use std::cmp::Reverse;

use crate::change::{ChangeKind, ChangeLevel, SemanticChange};
use crate::map::Confidence;

pub const TIER_SECURITY_SENSITIVE: u8 = 1;
pub const TIER_BREAKING: u8 = 2;
pub const TIER_DEPENDENCY: u8 = 3;
pub const TIER_RULES_OF_THE_GAME: u8 = 4;
pub const TIER_CONFIG: u8 = 5;
pub const TIER_ADDITIVE_CONTRACT: u8 = 6;
pub const TIER_RELATIONSHIP: u8 = 7;
pub const TIER_TEST: u8 = 8;
pub const TIER_INTERNAL_BEHAVIOR: u8 = 9;
pub const TIER_INTERNAL_RENAME: u8 = 10;
pub const TIER_INTERNAL: u8 = 11;

/// Subkind of the per-component bucket of remaining changes.
pub const SUBKIND_INTERNAL_CHANGES: &str = "internal-changes";

/// The tier of a change, lower is more important.
pub fn tier(change: &SemanticChange) -> u8 {
    match change.kind {
        ChangeKind::SecuritySensitive => TIER_SECURITY_SENSITIVE,
        ChangeKind::Breaking => TIER_BREAKING,
        ChangeKind::Dependency => TIER_DEPENDENCY,
        ChangeKind::Config | ChangeKind::Test if change.hints.rules_of_the_game => {
            TIER_RULES_OF_THE_GAME
        }
        ChangeKind::Config => TIER_CONFIG,
        ChangeKind::Additive if change.level == ChangeLevel::Relationship => TIER_RELATIONSHIP,
        ChangeKind::Additive => TIER_ADDITIVE_CONTRACT,
        ChangeKind::Test => TIER_TEST,
        ChangeKind::Internal if change.subkind == SUBKIND_INTERNAL_CHANGES => TIER_INTERNAL,
        ChangeKind::Internal if change.level == ChangeLevel::Behavior => TIER_INTERNAL_BEHAVIOR,
        ChangeKind::Internal => TIER_INTERNAL_RENAME,
    }
}

/// Sort key: smaller sorts first.
pub fn sort_key(change: &SemanticChange) -> impl Ord + '_ {
    (
        !change.hints.intent_mismatch,
        !change.hints.needs_person,
        tier(change),
        change.hints.confidence != Confidence::Low,
        Reverse(change.hints.labels.len()),
        Reverse(change.hints.blast_radius),
        change.id.as_str(),
    )
}

/// Sorts changes into report order.
pub fn rank(changes: &mut [SemanticChange]) {
    changes.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Hints;

    fn change(id: &str, kind: ChangeKind, level: ChangeLevel, subkind: &str) -> SemanticChange {
        SemanticChange {
            id: id.into(),
            kind,
            subkind: subkind.into(),
            level,
            subject: id.into(),
            component: None,
            kind_label: String::new(),
            title: id.into(),
            why_it_matters: String::new(),
            hints: Hints {
                labels: vec![],
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

    fn ids(changes: &[SemanticChange]) -> Vec<&str> {
        changes.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn follows_the_plan_order() {
        let mut rows = vec![
            change(
                "internal",
                ChangeKind::Internal,
                ChangeLevel::Structure,
                SUBKIND_INTERNAL_CHANGES,
            ),
            change(
                "rename",
                ChangeKind::Internal,
                ChangeLevel::Structure,
                "rename",
            ),
            change(
                "test",
                ChangeKind::Test,
                ChangeLevel::Behavior,
                "test-weakened",
            ),
            change(
                "consumer",
                ChangeKind::Additive,
                ChangeLevel::Relationship,
                "new-event-consumer",
            ),
            change(
                "contract",
                ChangeKind::Additive,
                ChangeLevel::Structure,
                "contract-field-added-optional",
            ),
            change(
                "config",
                ChangeKind::Config,
                ChangeLevel::Behavior,
                "config-changed",
            ),
            change(
                "dep",
                ChangeKind::Dependency,
                ChangeLevel::Relationship,
                "new-dependency",
            ),
            change(
                "break",
                ChangeKind::Breaking,
                ChangeLevel::Structure,
                "export-removed",
            ),
            change(
                "secure",
                ChangeKind::SecuritySensitive,
                ChangeLevel::Relationship,
                "new-external-service",
            ),
        ];
        rows[2].hints.rules_of_the_game = true;
        let mut ci = change(
            "ci",
            ChangeKind::Config,
            ChangeLevel::Behavior,
            "ci-changed",
        );
        ci.hints.rules_of_the_game = true;
        rows.push(ci);
        rank(&mut rows);
        assert_eq!(
            ids(&rows),
            [
                "secure", "break", "dep", "ci", "test", "config", "contract", "consumer", "rename",
                "internal"
            ]
        );
    }

    #[test]
    fn intent_mismatches_come_first() {
        let mut internal = change(
            "internal",
            ChangeKind::Internal,
            ChangeLevel::Structure,
            SUBKIND_INTERNAL_CHANGES,
        );
        internal.hints.intent_mismatch = true;
        let secure = change(
            "secure",
            ChangeKind::SecuritySensitive,
            ChangeLevel::Behavior,
            "secret-committed",
        );
        let mut rows = vec![secure, internal];
        rank(&mut rows);
        assert_eq!(ids(&rows), ["internal", "secure"]);
    }

    #[test]
    fn ties_break_on_confidence_then_labels_then_blast_radius() {
        let mut a = change("a", ChangeKind::Breaking, ChangeLevel::Structure, "x");
        let mut b = change("b", ChangeKind::Breaking, ChangeLevel::Structure, "x");
        let mut c = change("c", ChangeKind::Breaking, ChangeLevel::Structure, "x");
        let mut d = change("d", ChangeKind::Breaking, ChangeLevel::Structure, "x");
        a.hints.blast_radius = 10;
        b.hints.labels = vec!["payments".into()];
        c.hints.blast_radius = 20;
        d.hints.confidence = Confidence::Low;
        let mut rows = vec![a, b, c, d];
        rank(&mut rows);
        assert_eq!(ids(&rows), ["d", "b", "c", "a"]);
    }
}
