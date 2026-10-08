//! Phase 4 end to end: a change is submitted, classified into a lane by the
//! policy in onus.yaml and the hard floors, and verified by the judge.
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

const POLICY: &str = "\nlanes:\n  default: judge\n  auditRate: 0\n  minRecord: 0\n  rules:\n    - { lane: auto-merge, match: every, kinds: [internal], components: [logger] }\n    - { lane: human, subkinds: [migration-changed] }\n  heldOut: { command: \"node --test held-out/*.test.mjs\", image: \"node:22-alpine\" }\n";

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    copy_dir(&fixture().join("base"), r);
    let mut yaml = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    yaml.push_str(POLICY);
    std::fs::write(r.join("onus.yaml"), yaml).unwrap();
    std::fs::create_dir_all(r.join("held-out")).unwrap();
    std::fs::write(
        r.join("held-out/check.test.mjs"),
        "import test from 'node:test';\ntest('held out', () => {});\n",
    )
    .unwrap();
    git(r, &["init", "-q", "-b", "main"]);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "base"]);
    dir
}

/// Commits `files` on a new branch and returns its name.
fn change(r: &Path, branch: &str, files: &[(&str, &str)]) {
    git(r, &["checkout", "-q", "-B", branch, "main"]);
    for (path, text) in files {
        let p = r.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", branch]);
}

/// Submissions and runs go outside the repository, so commits never pick
/// them up.
fn out(r: &Path, name: &str) -> String {
    let dir = r
        .parent()
        .unwrap()
        .join(format!("{}-out", r.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_string_lossy().to_string()
}

fn lane_of(r: &Path, branch: &str) -> serde_json::Value {
    let sub = out(r, &format!("{branch}.json"));
    let (code, _, err) = onus(
        r,
        &[
            "submit",
            "--base",
            "main",
            "--head",
            branch,
            "--agent-tool",
            "test",
            "--out",
            &sub,
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = onus(r, &["classify", "--submission", &sub]);
    assert_eq!(code, 0, "{err}");
    serde_json::from_str(&out).unwrap()
}

#[test]
fn changes_take_the_lane_their_rows_and_floors_decide() {
    let dir = repo();
    let r = dir.path();
    // Only logger internals: the policy lets it auto-merge.
    let logger = std::fs::read_to_string(r.join("packages/logger/src/logger.ts")).unwrap();
    let logger = format!(
        "{logger}\nfunction redact(message: string): string {{\n  return message.replace(/[0-9]+/g, '#');\n}}\nvoid redact;\n"
    );
    change(r, "logger", &[("packages/logger/src/logger.ts", &logger)]);
    let c = lane_of(r, "logger");
    assert_eq!(c["lane"], "auto-merge", "{c}");

    // Payments code: a person, whatever the policy says.
    change(
        r,
        "billing",
        &[(
            "services/billing/src/discount.ts",
            "export const NOTE = 'x';\n",
        )],
    );
    let c = lane_of(r, "billing");
    assert_eq!(c["lane"], "human", "{c}");
    assert!(c.to_string().contains("payments"), "{c}");

    // A committed secret is blocked.
    change(
        r,
        "secret",
        &[(
            "services/orders/src/key.ts",
            "export const KEY = 'AKIAIOSFODNN7EXAMPLE';\n",
        )],
    );
    let c = lane_of(r, "secret");
    assert_eq!(c["lane"], "blocked", "{c}");

    // The judge: without evidence or intent it rejects with reasons the
    // agent can act on.
    let logger_sub = out(r, "logger.json");
    let (code, verdict, err) = onus(r, &["judge", "--submission", &logger_sub]);
    assert_eq!(code, 2, "{verdict}\n{err}");
    assert!(
        verdict.contains("no test evidence") && verdict.contains("no stated intent"),
        "{verdict}"
    );

    // With evidence and intent, and a container engine to verify with, it
    // re-runs the evidence and the held-out checks and approves.
    if onus_doors::runner::engine().is_err() {
        return;
    }
    let (code, run, err) = onus(
        r,
        &[
            "run-test",
            "--reference",
            "logger",
            "--image",
            "node:22-alpine",
            "--",
            "node --test held-out/*.test.mjs",
        ],
    );
    assert_eq!(code, 0, "{run}\n{err}");
    let run_file = out(r, "run.json");
    let intent_file = out(r, "intent.yaml");
    std::fs::write(&run_file, &run).unwrap();
    std::fs::write(
        &intent_file,
        "summary: Redact digits in logs\ntouches: [logger]\n",
    )
    .unwrap();
    let (code, _, err) = onus(
        r,
        &[
            "submit",
            "--base",
            "main",
            "--head",
            "logger",
            "--intent",
            &intent_file,
            "--evidence",
            &run_file,
            "--agent-tool",
            "test",
            "--out",
            &logger_sub,
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, verdict, err) = onus(r, &["judge", "--submission", &logger_sub]);
    assert_eq!(code, 0, "{verdict}\n{err}");
    let j: serde_json::Value = serde_json::from_str(&verdict).unwrap();
    assert_eq!(j["judgment"]["verdict"], "approve", "{j}");
}
