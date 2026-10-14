//! Evidence-based escalation end to end: a request with a failing test that
//! the test runner reproduces in a container is granted automatically when
//! it is low risk; one that reaches a sensitive component goes to a person,
//! with the owners it is routed to. The tests that reproduce are skipped when
//! no container engine is running.
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
    // The trusted test command and environment `escalation assess` uses, and
    // a test it runs with them.
    let config = std::fs::read_to_string(repo.join("onus.yaml")).unwrap();
    std::fs::write(
        repo.join("onus.yaml"),
        format!(
            "{config}tests:\n  command: \"sh {{test}}\"\nenvironment:\n  image: node:22-alpine\n"
        ),
    )
    .unwrap();
    std::fs::create_dir_all(repo.join("services/orders/test")).unwrap();
    std::fs::write(repo.join("services/orders/test/eta.test.sh"), "exit 1\n").unwrap();
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

    // Assessed without minting: onus.yaml's tests.command reproduces the
    // failing test in its declared image, and policy would grant it.
    let (code, _, err) = onus(
        &repo,
        &[
            "escalate",
            "--task",
            "sms",
            "--scope",
            "write:path:services/orders/src/**",
            "--evidence",
            "failing-test:services/orders/test/eta.test.sh",
            "--reason",
            "the eta must use local time",
            "--out",
            "../eta.json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = onus(
        &repo,
        &[
            "escalation",
            "assess",
            "../eta.json",
            "--reproduce-at",
            "HEAD",
        ],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    let assessed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(assessed["decision"], "granted", "{assessed}");
    assert_eq!(assessed["evidence"][0]["reproduced"], true);
    assert_eq!(assessed["evidence"][0]["run"]["image"], "node:22-alpine");
    assert_eq!(
        assessed["request"]["owners"],
        serde_json::json!(["@team-orders"])
    );
    assert!(assessed.get("token").is_none(), "{assessed}");
    assert!(!dir.path().join("eta.token").exists());

    // A reference that is not a plain path is never run, and it sends the
    // request to a person even beside a test that reproduced.
    let assess = |at: &str, evidence: &[&str]| {
        let mut args = vec!["escalate", "--task", "sms", "--scope"];
        args.push("write:path:services/orders/src/**");
        for e in evidence {
            args.extend(["--evidence", e]);
        }
        args.extend([
            "--reason",
            "the eta must use local time",
            "--out",
            "../a.json",
        ]);
        let (code, _, err) = onus(&repo, &args);
        assert_eq!(code, 0, "{err}");
        let (code, out, err) = onus(
            &repo,
            &["escalation", "assess", "../a.json", "--reproduce-at", at],
        );
        assert!(code == 0 || code == 3, "{out}\n{err}");
        let assessed: serde_json::Value = serde_json::from_str(&out).unwrap();
        (code, assessed)
    };
    let (code, assessed) = assess(
        "HEAD",
        &[
            "failing-test:services/orders/test/eta.test.sh",
            "failing-test:x.test.sh;exit",
        ],
    );
    assert_eq!(code, 3, "{assessed}");
    assert_eq!(assessed["decision"], "needs-person");
    assert_eq!(assessed["evidence"][0]["reproduced"], true);
    assert_eq!(assessed["evidence"][1]["reproduced"], false);
    assert!(assessed["evidence"][1].get("run").is_none(), "{assessed}");
    let reasons = assessed["reasons"].to_string();
    assert!(
        reasons.contains("`x.test.sh;exit` is not a path"),
        "{reasons}"
    );

    // The agent's commit is only the tree the test runs in: its own
    // onus.yaml, which makes every command fail and every .sh file a test,
    // is not what assess reads.
    git(&repo, &["checkout", "-q", "-b", "agent"]);
    std::fs::write(
        repo.join("onus.yaml"),
        format!(
            "{config}tests:\n  command: \"false\"\n  globs: [\"**/*.sh\"]\nenvironment:\n  image: node:22-alpine\n"
        ),
    )
    .unwrap();
    std::fs::write(repo.join("services/orders/test/eta.test.sh"), "exit 0\n").unwrap();
    std::fs::write(repo.join("services/orders/src/eta.sh"), "exit 1\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "agent"]);
    git(&repo, &["checkout", "-q", "main"]);
    let (code, assessed) = assess("agent", &["failing-test:services/orders/test/eta.test.sh"]);
    assert_eq!(code, 3, "{assessed}");
    assert_eq!(assessed["evidence"][0]["reproduced"], false);
    assert_eq!(
        assessed["evidence"][0]["run"]["command"],
        "sh services/orders/test/eta.test.sh"
    );
    let (code, assessed) = assess("agent", &["failing-test:services/orders/src/eta.sh"]);
    assert_eq!(code, 3, "{assessed}");
    let reasons = assessed["reasons"].to_string();
    assert!(
        reasons.contains("`services/orders/src/eta.sh` is not a test file"),
        "{reasons}"
    );

    // Assessing is not deciding: the audit log has only the three decisions.
    let (code, out, _) = onus(dir.path(), &["audit", audit.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(out.contains("3 entries"), "{out}");
}

#[test]
fn assess_routes_a_sensitive_request_to_a_person_with_its_owners() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("shop");
    copy_dir(&fixture().join("base"), &repo);
    let (code, _, err) = onus(
        &repo,
        &[
            "escalate",
            "--task",
            "sms",
            "--kind",
            "permission",
            "--scope",
            "write:path:services/billing/src/**",
            "--evidence",
            "rationale:x",
            "--reason",
            "refunds should notify",
            "--out",
            "../r.json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = onus(&repo, &["escalation", "assess", "../r.json"]);
    assert_eq!(code, 3, "{out}\n{err}");
    let assessed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(assessed["decision"], "needs-person");
    assert_eq!(
        assessed["request"]["owners"],
        serde_json::json!(["@team-payments"])
    );
    assert_eq!(
        assessed["request"]["sensitive"],
        serde_json::json!(["payments"])
    );
    let reasons = assessed["reasons"].to_string();
    assert!(reasons.contains("labeled payments"), "{reasons}");
    assert!(reasons.contains("grade 5"), "{reasons}");
    assert!(
        reasons.contains("no test command is configured"),
        "{reasons}"
    );
    assert!(assessed.get("token").is_none());

    // Evidence in another component does not add its owners as deciders:
    // an agent cannot pick who may grant it.
    let (code, _, err) = onus(
        &repo,
        &[
            "escalate",
            "--task",
            "sms",
            "--scope",
            "write:path:services/billing/src/**",
            "--evidence",
            "trace:services/orders/src/shipping/eta.test.ts",
            "--reason",
            "refunds should notify",
            "--out",
            "../traced.json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, _) = onus(&repo, &["escalation", "assess", "../traced.json"]);
    assert_eq!(code, 3, "{out}");
    let assessed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        assessed["request"]["owners"],
        serde_json::json!(["@team-payments"])
    );

    // A broken test with no scopes goes to the owners of the test, and a
    // failing test is not reproduced without a test command.
    let (code, _, err) = onus(
        &repo,
        &[
            "escalate",
            "--task",
            "sms",
            "--kind",
            "broken-test",
            "--evidence",
            "failing-test:services/orders/src/shipping/eta.test.ts",
            "--reason",
            "the test expects UTC but the spec says local time",
            "--out",
            "../blocked.json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, _) = onus(
        &repo,
        &[
            "escalation",
            "assess",
            "../blocked.json",
            "--reproduce-at",
            "HEAD",
        ],
    );
    assert_eq!(code, 3, "{out}");
    let assessed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        assessed["request"]["owners"],
        serde_json::json!(["@team-orders"])
    );
    let reasons = assessed["reasons"].to_string();
    assert!(
        reasons.contains("a broken-test request always goes to a person"),
        "{reasons}"
    );
    assert!(
        reasons.contains("no test command is configured"),
        "{reasons}"
    );
}

#[test]
fn a_run_that_does_not_finish_in_time_is_stopped_and_leaves_nothing_behind() {
    let Ok(engine) = onus_doors::runner::engine() else {
        return;
    };
    // Pulled first, so the limit is spent in the step under test, not the
    // pull.
    let pulled = Command::new(&engine)
        .args(["pull", "-q", "node:22-alpine"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !pulled {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.test.sh"), "sleep 600\n").unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "a"]);
    let out = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let commit = String::from_utf8_lossy(&out.stdout).trim()[..12].to_string();
    let left = |args: &[&str]| -> String {
        let out = Command::new(&engine).args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains(&commit))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let limit = std::time::Duration::from_secs(5);
    // What comes before the test or the setup fits in the limit, so below
    // they are stopped in the test, and then in the setup step.
    let quick = onus_doors::runner::run_within(
        dir.path(),
        "HEAD",
        "node:22-alpine",
        None,
        "true",
        Some(limit),
    )
    .unwrap();
    assert!(matches!(quick, onus_doors::runner::Ran::Done(_)));
    for (setup, command) in [(None, "sh a.test.sh"), (Some("sleep 600"), "true")] {
        let started = std::time::Instant::now();
        let ran = onus_doors::runner::run_within(
            dir.path(),
            "HEAD",
            "node:22-alpine",
            setup,
            command,
            Some(limit),
        )
        .unwrap();
        assert_eq!(ran, onus_doors::runner::Ran::TimedOut, "{setup:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(120));
        // Its containers and the setup's image are removed.
        assert_eq!(
            left(&["ps", "-a", "--format", "{{.Names}}"]),
            "",
            "{setup:?}"
        );
        assert_eq!(
            left(&["images", "--format", "{{.Repository}}:{{.Tag}}"]),
            "",
            "{setup:?}"
        );
    }
}
