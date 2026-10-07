//! The golden scenarios S1–S9 (PLAN.md section 5.8). Each scenario's report
//! is snapshotted as Markdown and JSON, and the facts the acceptance
//! criteria require are asserted directly as well.

mod common;

use common::Scenario;
use onus_core::{ChangeKind, SemanticChange, SemanticReport};

struct Run {
    report: SemanticReport,
    md: String,
    json: String,
}

fn run(id: &str) -> Run {
    let s = Scenario::new(id);
    let outcome = s.run();
    let md = outcome.render(onus_cli::Format::Md);
    let json = outcome.render(onus_cli::Format::Json);
    insta::assert_snapshot!(format!("{id}.md"), md);
    insta::assert_snapshot!(format!("{id}.json"), json);
    Run {
        report: outcome.report,
        md,
        json,
    }
}

fn subkinds(r: &SemanticReport) -> Vec<&str> {
    r.changes.iter().map(|c| c.subkind.as_str()).collect()
}

fn row<'a>(r: &'a SemanticReport, subkind: &str) -> &'a SemanticChange {
    r.changes
        .iter()
        .find(|c| c.subkind == subkind)
        .unwrap_or_else(|| panic!("no {subkind} row"))
}

#[test]
fn s1_sms_alerts_has_exactly_the_manifesto_rows() {
    let r = run("s1-sms-alerts").report;
    assert_eq!(
        subkinds(&r),
        [
            "new-external-service",
            "contract-field-added-optional",
            "new-event-consumer",
            "internal-changes"
        ]
    );
    let sms = &r.changes[0];
    assert_eq!(sms.kind, ChangeKind::SecuritySensitive);
    assert_eq!(
        sms.title,
        "`notifications` now calls an external SMS provider (Acme SMS)"
    );
    assert!(
        sms.hints
            .novelty
            .contains(&"new-vendor:Acme SMS".to_string())
    );
    assert!(
        sms.hints
            .novelty
            .contains(&"new-data-egress:phone".to_string())
    );
    assert!(sms.hints.needs_person);
    assert!(
        sms.why_it_matters
            .starts_with("Customer phone numbers leave the system")
    );

    let contract = &r.changes[1];
    assert_eq!(contract.kind, ChangeKind::Additive);
    assert_eq!(
        contract.title,
        "`UserPreferences` gains an optional `phoneVerified` field"
    );
    assert!(contract.hints.blast_radius > 0);
    assert!(
        contract
            .why_it_matters
            .contains("4 files across 2 components")
    );

    let consumer = &r.changes[2];
    assert_eq!(
        consumer.title,
        "`notifications` subscribes to the `OrderShipped` event"
    );
    assert_eq!(consumer.why_it_matters, "Additive; `orders` is unchanged");

    let internal = &r.changes[3];
    assert_eq!(internal.component.as_deref(), Some("notifications"));
    let stats = internal.stats.unwrap();
    assert_eq!(stats.lines_added + stats.lines_removed, 1_402);
    // The call that sends phone numbers out is cited, not just the import.
    assert!(
        sms.locations
            .iter()
            .any(|l| l.file.ends_with("sms/client.ts") && l.lines == [51, 51])
    );
    assert_eq!(r.summary.needs_attention, 1);
    assert_eq!(r.text_stats.files, 23);
    assert_eq!(r.text_stats.lines_added + r.text_stats.lines_removed, 1_408);
}

#[test]
fn s2_rename_is_one_row_with_call_sites() {
    let r = run("s2-rename").report;
    assert_eq!(subkinds(&r), ["rename"]);
    let rename = &r.changes[0];
    assert_eq!(rename.kind, ChangeKind::Internal);
    assert_eq!(rename.title, "`formatMoney` renamed to `formatCurrency`");
    assert!(
        rename
            .why_it_matters
            .starts_with("No behavior change (identical body); 40 call sites in 40 files updated"),
        "{}",
        rename.why_it_matters
    );
}

#[test]
fn s3_boundary_condition_in_payments_ranks_first() {
    let r = run("s3-boundary").report;
    let first = &r.changes[0];
    assert_eq!(first.subkind, "boundary-condition-changed");
    assert_eq!(first.kind, ChangeKind::SecuritySensitive);
    assert_eq!(first.hints.labels, ["payments"]);
    assert!(first.title.contains("`order.subtotalCents > DISCOUNT_THRESHOLD_CENTS` is now `order.subtotalCents >= DISCOUNT_THRESHOLD_CENTS`"));
    assert_eq!(
        subkinds(&r),
        ["boundary-condition-changed", "internal-changes"]
    );
}

#[test]
fn s4_intent_mismatch_ranks_first_with_the_data_write() {
    let r = run("s4-intent-mismatch").report;
    let first = &r.changes[0];
    assert!(first.hints.intent_mismatch);
    assert_eq!(first.subkind, "new-data-write");
    assert_eq!(first.title, "`billing` now writes to the `payment` table");
    let check = r.intent_check.as_ref().unwrap();
    assert!(check.mismatches.iter().any(|m| m.change_id == first.id));
    // Everything inside `logger` matches the stated intent.
    assert!(
        r.changes
            .iter()
            .filter(|c| c.component.as_deref() == Some("logger"))
            .all(|c| !c.hints.intent_mismatch)
    );
}

#[test]
fn s5_weakened_tests_are_rules_of_the_game() {
    let r = run("s5-test-weakened").report;
    let t = row(&r, "test-weakened");
    assert_eq!(t.kind, ChangeKind::Test);
    assert!(t.hints.rules_of_the_game);
    assert!(t.hints.needs_person);
    assert!(t.why_it_matters.contains("1 assertion removed"));
    assert!(t.why_it_matters.contains("is now skipped"));
    assert!(
        t.why_it_matters
            .contains("source in `billing` changed in the same pull request")
    );
    // The source change behind it: a payments rate moves from 0.1 to 0.15.
    let rate = row(&r, "constant-changed");
    assert_eq!(rate.title, "`LOYALTY_RATE` changes from `0.1` to `0.15`");
    assert_eq!(rate.kind, ChangeKind::SecuritySensitive);
    assert_eq!(subkinds(&r), ["constant-changed", "test-weakened"]);
}

#[test]
fn s6_new_boundary_rule_violation() {
    let r = run("s6-boundary-rule").report;
    assert_eq!(subkinds(&r), ["rule-violation"]);
    assert_eq!(r.rule_violations.len(), 1);
    assert_eq!(
        r.changes[0].title,
        "`billing` reaches into `notifications` internals (`renderTemplate`)"
    );
}

#[test]
fn s7_new_dependency_row() {
    let r = run("s7-new-dependency").report;
    let d = row(&r, "new-dependency");
    assert_eq!(d.kind, ChangeKind::Dependency);
    assert_eq!(d.title, "`orders` adds the npm package `date-fns` (^4.1.0)");
    assert_eq!(d.hints.novelty, ["new-package:date-fns"]);
    assert_eq!(subkinds(&r)[0], "new-dependency");
}

#[test]
fn s8_secret_is_reported_without_its_value() {
    let run = run("s8-secret");
    let r = &run.report;
    let s = row(r, "secret-committed");
    assert_eq!(s.kind, ChangeKind::SecuritySensitive);
    assert_eq!(subkinds(r)[0], "secret-committed");
    assert_eq!(r.summary.secrets, 1);
    for out in [&run.md, &run.json] {
        // Split so this file does not hold a secret-shaped literal itself.
        assert!(!out.contains(concat!("AKIA", "IOSFODNN7EXAMPLE")));
        assert!(!out.contains(concat!("wJalrXUtnFEMI", "/K7MDENG")));
    }
}

#[test]
fn s9_formatting_and_moves_have_no_meaning_rows() {
    let run = run("s9-formatting-move");
    assert!(run.report.changes.is_empty());
    assert!(run.md.contains("no changes in meaning"));
    assert_eq!(run.report.structure.moved_files.len(), 1);
    assert_eq!(run.report.structure.formatting_only.len(), 3);
}

#[test]
fn reports_are_deterministic() {
    // Different temporary directories, same bytes.
    for id in ["s1-sms-alerts", "s9-formatting-move"] {
        let a = Scenario::new(id).run();
        let b = Scenario::new(id).run();
        assert_eq!(
            a.render(onus_cli::Format::Json),
            b.render(onus_cli::Format::Json)
        );
        assert_eq!(
            a.render(onus_cli::Format::Md),
            b.render(onus_cli::Format::Md)
        );
    }
}

#[test]
fn secrets_in_declared_test_data_are_noted_not_counted() {
    let s = Scenario::new("s8-secret");
    for side in ["base", "head"] {
        let path = s.dir.path().join(side).join("onus.yaml");
        let mut yaml = std::fs::read_to_string(&path).unwrap();
        yaml.push_str("\ntestData:\n  - services/billing/src/payments.ts\n");
        std::fs::write(&path, yaml).unwrap();
    }
    let outcome = s.run();
    let r = &outcome.report;
    assert!(!subkinds(r).contains(&"secret-committed"));
    assert_eq!(r.summary.secrets, 0);
    let note = row(r, "secret-in-test-data");
    assert_eq!(note.kind, ChangeKind::Internal);
    assert!(!note.hints.needs_person);
    assert!(
        note.why_it_matters
            .contains("services/billing/src/payments.ts")
    );
    // Test data is not part of the map.
    assert!(
        !outcome
            .head_map
            .files
            .iter()
            .any(|f| f.path == "services/billing/src/payments.ts")
    );
    let md = outcome.render(onus_cli::Format::Md);
    assert!(!md.contains(concat!("AKIA", "IOSFODNN7EXAMPLE")));
}
