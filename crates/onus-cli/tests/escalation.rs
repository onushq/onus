//! Evidence-based escalation end to end: a request with a failing test that
//! the test runner reproduces in a container is granted automatically when
//! it is low risk; one that reaches a sensitive component goes to a person.
//! Skipped when no container engine is running.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Command;

use common::{copy_dir, fixture};

fn onus(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn reproduced_low_risk_requests_are_granted_and_sensitive_ones_go_to_a_person() {
    // Without a container engine there is nothing to reproduce with.
    if onus_doors::runner::engine().is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    copy_dir(&fixture().join("base"), &repo);
    // A test that fails at this commit, runnable with Node alone.
    std::fs::write(
        repo.join("services/orders/src/ship.test.mjs"),
        "import test from 'node:test';\nimport assert from 'node:assert';\ntest('ships', () => assert.equal(1, 2));\n",
    )
    .unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let keys = dir.path().join("keys");
    let (code, _, err) = onus(
        dir.path(),
        &["token", "keygen", "--out", keys.to_str().unwrap()],
    );
    assert_eq!(code, 0, "{err}");
    std::fs::write(
        dir.path().join("plan.yaml"),
        "task: sms\nwrites: [\"services/notifications/**\"]\n",
    )
    .unwrap();
    let key = keys.join("root.key");
    let public = keys.join("root.pub");
    let (_, token, _) = onus(
        dir.path(),
        &[
            "token",
            "mint",
            "--plan",
            "plan.yaml",
            "--key",
            key.to_str().unwrap(),
        ],
    );
    let audit = dir.path().join("audit.jsonl");

    // Low risk, grade-1 evidence: granted after the runner reproduces it.
    let (code, _, err) = onus(
        &repo,
        &[
            "escalate",
            "--task",
            "sms",
            "--scope",
            "write:path:services/orders/src/**",
            "--evidence",
            "failing-test:services/orders/src/ship.test.mjs",
            "--reason",
            "the shipped event must carry the phone number",
            "--out",
            "../orders.json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = onus(
        &repo,
        &[
            "escalation",
            "decide",
            "../orders.json",
            "--token",
            &token,
            "--key-public",
            public.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--reproduce-at",
            "HEAD",
            "--image",
            "node:22-alpine",
            "--test-command",
            "node --test {test}",
            "--audit",
            audit.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    let decision: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(decision["decision"], "granted", "{decision}");
    assert_eq!(decision["evidence"][0]["reproduced"], true);
    let granted = decision["token"].as_str().unwrap().to_string();
    let (code, _, _) = onus(
        dir.path(),
        &[
            "token",
            "check",
            "--token",
            &granted,
            "--key-public",
            public.to_str().unwrap(),
            "write:path:services/orders/src/ship.ts",
        ],
    );
    assert_eq!(code, 0);

    // Payments code: a person decides, whatever the evidence.
    let (code, _, err) = onus(
        &repo,
        &[
            "escalate",
            "--task",
            "sms",
            "--scope",
            "write:path:services/billing/src/**",
            "--evidence",
            "failing-test:services/orders/src/ship.test.mjs",
            "--reason",
            "refunds should notify",
            "--out",
            "../billing.json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, _) = onus(
        &repo,
        &[
            "escalation",
            "decide",
            "../billing.json",
            "--token",
            &token,
            "--key-public",
            public.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--audit",
            audit.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 3);
    assert!(out.contains("payments"), "{out}");
    let (code, person_grant, _) = onus(
        &repo,
        &[
            "escalation",
            "grant",
            "../billing.json",
            "--token",
            &token,
            "--key-public",
            public.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--by",
            "@team-payments",
            "--audit",
            audit.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0);
    let (code, _, _) = onus(
        dir.path(),
        &[
            "token",
            "check",
            "--token",
            &person_grant,
            "--key-public",
            public.to_str().unwrap(),
            "write:path:services/billing/src/refund.ts",
        ],
    );
    assert_eq!(code, 0);
    let (code, out, _) = onus(dir.path(), &["audit", audit.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(out.contains("3 entries"), "{out}");
}
