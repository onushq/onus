//! Queries over the map of the `fixtures/shop` monorepo.

use std::path::PathBuf;
use std::sync::Arc;

use onus_index::{MapIndex, Query};
use onus_map::{BuildOptions, FactsCache, build_map};

fn shop() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("../../fixtures/shop/base")
}

fn index() -> MapIndex {
    MapIndex::new(build_map(&shop(), &BuildOptions::default()).unwrap())
}

#[test]
fn find_ranks_exact_names_first() {
    let idx = index();
    let found = idx.find("UserPreferences", 5);
    assert_eq!(
        found.symbols[0].id,
        "user-preferences:src/types.ts#UserPreferences"
    );
    // Words match name parts in any case.
    let found = idx.find("apply discount", 5);
    assert_eq!(found.symbols[0].name, "applyDiscount");
    assert!(idx.find("no such thing anywhere", 5).symbols.is_empty());
}

#[test]
fn dependents_cross_components_with_evidence() {
    let idx = index();
    let walk = idx.dependents("UserPreferences", 2, 50).unwrap();
    assert_eq!(
        walk.resolved,
        ["user-preferences:src/types.ts#UserPreferences"]
    );
    assert!(walk.components.contains_key("notifications"));
    for link in &walk.links {
        assert!(link.file.is_some() && link.line.is_some(), "{link:?}");
        assert!(link.depth >= 1 && link.depth <= 2);
    }
    // Bounded: a limit truncates and says so.
    let small = idx.dependents("UserPreferences", 2, 1).unwrap();
    assert_eq!(small.links.len(), 1);
    assert!(small.truncated);
    assert_eq!(small.total, walk.total);
    // Unknown targets explain what is accepted.
    let err = idx.dependents("nothing-like-this", 1, 10).unwrap_err();
    assert!(err.contains("find"), "{err}");
}

#[test]
fn dependencies_reach_events_and_services() {
    let idx = index();
    let walk = idx.dependencies("notifications", 1, 100).unwrap();
    let kinds: Vec<&str> = walk.links.iter().map(|l| l.kind.as_str()).collect();
    assert!(kinds.contains(&"consumes"), "{kinds:?}");
    assert!(kinds.contains(&"calls-external"), "{kinds:?}");
}

#[test]
fn tests_owners_components_and_files() {
    let idx = index();
    let tests = idx.tests_for("applyDiscount", 10).unwrap();
    assert_eq!(tests.tests[0].file, "services/billing/src/discount.test.ts");
    assert!(tests.tests[0].cases > 0);

    let owners = idx.owners("services/billing/src/payments.ts").unwrap();
    assert_eq!(owners.component, "billing");
    assert_eq!(owners.owners, ["@team-payments"]);
    assert_eq!(owners.labels, ["payments"]);

    let billing = idx.component("billing").unwrap();
    assert!(billing.uses.contains_key("money"));

    let file = idx.file("services/orders/src/orders.ts").unwrap();
    assert_eq!(file.component.as_deref(), Some("orders"));
    assert!(
        file.imported_by
            .contains(&"services/orders/src/index.ts".to_string())
    );
    assert!(file.symbols.iter().any(|s| s.name == "placeOrder"));
}

#[test]
fn answers_are_deterministic_json() {
    let (a, b) = (index(), index());
    for q in [
        Query::Status,
        Query::Find {
            text: "order".into(),
            limit: 0,
        },
        Query::Dependents {
            target: "money".into(),
            depth: 3,
            limit: 0,
        },
        Query::Component {
            id: "billing".into(),
        },
    ] {
        assert_eq!(a.answer(&q).unwrap(), b.answer(&q).unwrap(), "{q:?}");
    }
}

#[test]
fn a_facts_cache_gives_the_same_map() {
    let dir = tempfile::tempdir().unwrap();
    let plain = build_map(&shop(), &BuildOptions::default()).unwrap();
    let cache = Arc::new(FactsCache::on_disk(dir.path()));
    let opts = BuildOptions {
        facts_cache: Some(cache.clone()),
        ..BuildOptions::default()
    };
    let cold = build_map(&shop(), &opts).unwrap();
    let misses = cache.stats().misses;
    assert!(misses > 0);
    let warm = build_map(&shop(), &opts).unwrap();
    assert_eq!(
        cache.stats().misses,
        misses,
        "the second build parses nothing"
    );
    // A new process with the same folder reads the facts back from disk.
    let reloaded = Arc::new(FactsCache::on_disk(dir.path()));
    let from_disk = build_map(
        &shop(),
        &BuildOptions {
            facts_cache: Some(reloaded.clone()),
            ..BuildOptions::default()
        },
    )
    .unwrap();
    assert_eq!(reloaded.stats().misses, 0);
    assert!(reloaded.stats().disk_hits > 0);
    let json = |m| serde_json::to_string(&m).unwrap();
    let expected = json(plain);
    assert_eq!(json(cold), expected);
    assert_eq!(json(warm), expected);
    assert_eq!(json(from_disk), expected);
}
