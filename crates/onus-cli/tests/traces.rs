//! Runtime traces as map facts (ADR 0010): calls static analysis cannot see,
//! such as an event bus dispatching to a handler, are added from OTLP JSON.

mod common;

use std::process::Command;

use common::{fixture, repo_root};

fn trace() -> String {
    repo_root()
        .join("crates/onus-cli/tests/otlp/order-placed.json")
        .to_string_lossy()
        .to_string()
}

#[test]
fn traced_calls_join_the_map() {
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .arg("map")
        .arg(fixture().join("base"))
        .args(["--json", "--traces", &trace()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("4 spans, 3 resolved, 2 traced calls added, 1 confirmed"),
        "{stderr}"
    );
    let map: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let traced: Vec<(String, String)> = map["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["confidence"] == "traced")
        .map(|e| {
            (
                e["from"].as_str().unwrap().to_string(),
                e["to"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    // The bus dispatch, and a call into another service by its name. The
    // static edge invoiceOrder -> createInvoice is confirmed, not repeated.
    assert_eq!(
        traced,
        [
            (
                "orders:src/orders.ts#placeOrder".to_string(),
                "billing:src/subscriptions.ts#invoiceOrder".to_string()
            ),
            (
                "orders:src/orders.ts#placeOrder".to_string(),
                "notifications".to_string()
            ),
        ]
    );
}

#[test]
fn traces_apply_to_both_trees_and_change_nothing() {
    let base = fixture().join("base");
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .arg("diff")
        .arg(&base)
        .arg(&base)
        .args(["--format", "json", "--traces", &trace()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["changes"].as_array().unwrap().len(), 0, "{report}");
}
