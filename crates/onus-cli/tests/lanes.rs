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

const POLICY: &str = "\nlanes:\n  default: judge\n  auditRate: 0\n  minRecord: 0\n  rules:\n    - { lane: auto-merge, match: every, kinds: [internal], components: [logger] }\n    - { lane: human, subkinds: [migration-changed] }\n";

fn repo() -> tempfile::TempDir {
    repo_with(POLICY)
}

/// The shop with `policy` appended to its onus.yaml. Held-out suites are
/// kept outside it, where the author cannot read them.
fn repo_with(policy: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    copy_dir(&fixture().join("base"), r);
    let mut yaml = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    yaml.push_str(policy);
    std::fs::write(r.join("onus.yaml"), yaml).unwrap();
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
    logger_change(r);
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
    // An AWS-shaped key, assembled here so this file holds no secret.
    let key = format!("export const KEY = '{}{}';\n", "AKIA", "IOSFODNN7EXAMPLE");
    change(r, "secret", &[("services/orders/src/key.ts", &key)]);
    let c = lane_of(r, "secret");
    assert_eq!(c["lane"], "blocked", "{c}");

    // The judge: without evidence or intent it rejects with the first thing
    // the agent can act on, and stops there.
    let logger_sub = out(r, "logger.json");
    let (code, verdict, err) = onus(r, &["judge", "--submission", &logger_sub]);
    assert_eq!(code, 2, "{verdict}\n{err}");
    let j: serde_json::Value = serde_json::from_str(&verdict).unwrap();
    let reasons = j["judgment"]["reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), 1, "{verdict}");
    assert!(
        reasons[0].as_str().unwrap().contains("no test evidence"),
        "{verdict}"
    );
    assert!(!verdict.contains("no stated intent"), "{verdict}");

    // With evidence and intent, and a container engine to verify with, it
    // re-runs the evidence and approves.
    if onus_doors::runner::engine().is_err() {
        return;
    }
    submit_with_evidence(r, "logger", &logger_sub);
    let (code, verdict, err) = onus(r, &["judge", "--submission", &logger_sub]);
    assert_eq!(code, 0, "{verdict}\n{err}");
    let j: serde_json::Value = serde_json::from_str(&verdict).unwrap();
    assert_eq!(j["judgment"]["verdict"], "approve", "{j}");
}

/// Submits `branch` as `sub` with an intent about the logger and a test run
/// at its head as evidence. Needs a container engine.
fn submit_with_evidence(r: &Path, branch: &str, sub: &str) {
    let (code, run, err) = onus(
        r,
        &[
            "run-test",
            "--reference",
            branch,
            "--image",
            "node:22-alpine",
            "--",
            "test -f package.json",
        ],
    );
    assert_eq!(code, 0, "{run}\n{err}");
    let run_file = out(r, &format!("{branch}-run.json"));
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
            branch,
            "--intent",
            &intent_file,
            "--evidence",
            &run_file,
            "--agent-tool",
            "test",
            "--out",
            sub,
        ],
    );
    assert_eq!(code, 0, "{err}");
}

/// A logger-only change on the branch `logger`.
fn logger_change(r: &Path) {
    let logger = std::fs::read_to_string(r.join("packages/logger/src/logger.ts")).unwrap();
    let logger = format!(
        "{logger}\nfunction redact(message: string): string {{\n  return message.replace(/[0-9]+/g, '#');\n}}\nvoid redact;\n"
    );
    change(r, "logger", &[("packages/logger/src/logger.ts", &logger)]);
}

/// A suite in a directory of its own: `held-out.yaml` and a marker file.
fn suite(yaml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("held-out.yaml"), yaml).unwrap();
    std::fs::write(dir.path().join("marker"), "").unwrap();
    dir
}

/// Three checks: one about the logger, one about user-preferences (which
/// does not interact with the logger in the shop, while billing does), and
/// one for every change. `logger` is the logger check's command.
fn suite_yaml(logger: &str, setup: Option<&str>) -> String {
    format!(
        "version: 1\nimage: node:22-alpine\n{}checks:\n  - id: logger-format\n    command: {logger:?}\n    covers: [logger]\n  - id: prefs-only\n    command: \"false\"\n    covers: [user-preferences]\n  - id: smoke\n    command: \"test -d /held-out\"\n",
        setup.map(|s| format!("setup: {s:?}\n")).unwrap_or_default()
    )
}

#[test]
fn held_out_suites_live_outside_and_follow_the_map() {
    let dir = repo();
    let r = dir.path();
    logger_change(r);
    let sub = out(r, "logger.json");
    let (code, _, err) = onus(
        r,
        &[
            "submit",
            "--base",
            "main",
            "--head",
            "logger",
            "--agent-tool",
            "test",
            "--out",
            &sub,
        ],
    );
    assert_eq!(code, 0, "{err}");
    let passing = suite(&suite_yaml(
        "test -f /held-out/marker && test -f /work/package.json",
        None,
    ));
    let s = passing.path().to_str().unwrap();

    // The suite checks out, with its hash.
    let (code, checked, err) = onus(r, &["held-out", "check", s, "--json"]);
    assert_eq!(code, 0, "{err}");
    let checked: serde_json::Value = serde_json::from_str(&checked).unwrap();
    assert_eq!(checked["suite"].as_str().unwrap().len(), 12, "{checked}");
    assert_eq!(checked["checks"].as_array().unwrap().len(), 3, "{checked}");
    // Only the short hash: the full one is what makes the rotation unpredictable.
    assert!(checked.get("key").is_none(), "{checked}");

    // Inside the repository, the author could read it.
    let inside = r.join("held-out");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::write(inside.join("held-out.yaml"), suite_yaml("true", None)).unwrap();
    let (code, stdout, err) = onus(
        r,
        &[
            "judge",
            "--submission",
            &sub,
            "--held-out",
            inside.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 1, "{stdout}\n{err}");
    assert!(err.contains("inside the repository"), "{err}");
    let (code, _, err) = onus(
        r,
        &["held-out", "check", inside.to_str().unwrap(), "--repo", "."],
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("inside the repository"), "{err}");
    std::fs::remove_dir_all(&inside).unwrap();

    // The map picks the logger's check and the untagged one; user-preferences
    // has nothing to do with the logger.
    let (code, picked, err) = onus(
        r,
        &[
            "held-out",
            "select",
            "--held-out",
            s,
            "--submission",
            &sub,
            "--json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let picked: serde_json::Value = serde_json::from_str(&picked).unwrap();
    let ids: Vec<&str> = picked["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["logger-format", "smoke"], "{picked}");
    assert_eq!(picked["checks"][0]["why"], "touched", "{picked}");
    assert_eq!(picked["notSelected"], serde_json::json!(["prefs-only"]));
    assert_eq!(picked["reach"]["map"], true, "{picked}");

    if onus_doors::runner::engine().is_err() {
        return;
    }
    submit_with_evidence(r, "logger", &sub);
    // Both checks see the suite and the tree.
    let (code, stdout, err) = onus(r, &["judge", "--submission", &sub, "--held-out", s]);
    assert_eq!(code, 0, "{stdout}\n{err}");
    let j: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(j["judgment"]["heldOut"]["ran"], 2, "{j}");
    assert_eq!(j["judgment"]["heldOut"]["passed"], 2, "{j}");
    assert!(j.get("heldOut").is_none(), "{j}");

    // A failing check rejects without naming itself.
    let failing = suite(&suite_yaml("test -f /held-out/missing", None));
    let f = failing.path().to_str().unwrap();
    let (code, stdout, err) = onus(r, &["judge", "--submission", &sub, "--held-out", f]);
    assert_eq!(code, 2, "{stdout}\n{err}");
    let j: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        j["judgment"]["reasons"],
        serde_json::json!(["held-out: a held-out check failed (exit 1)"])
    );
    for id in ["logger-format", "prefs-only", "smoke", "/held-out/missing"] {
        assert!(!stdout.contains(id), "{id} in {stdout}");
    }
    // People can ask what ran.
    let (code, stdout, _) = onus(
        r,
        &[
            "judge",
            "--submission",
            &sub,
            "--held-out",
            f,
            "--show-held-out",
        ],
    );
    assert_eq!(code, 2);
    let j: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(j["heldOut"]["checks"][0]["id"], "logger-format", "{j}");
    assert_eq!(j["heldOut"]["checks"][0]["status"], "failed", "{j}");
    assert_eq!(j["heldOut"]["checks"][1]["status"], "not-run", "{j}");

    // The setup runs with network, and never sees the suite.
    let setup = suite(&suite_yaml(
        "test -f /held-out/marker",
        Some("test ! -e /held-out"),
    ));
    let (code, stdout, err) = onus(
        r,
        &[
            "judge",
            "--submission",
            &sub,
            "--held-out",
            setup.path().to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{stdout}\n{err}");
}

#[test]
fn judges_rotate_and_are_recorded() {
    let dir = repo_with(&format!(
        "{POLICY}  judges:\n    - name: a\n    - name: b\n      heldOutLimit: 1\n"
    ));
    let r = dir.path();
    logger_change(r);
    let sub = out(r, "logger.json");
    let (code, _, err) = onus(
        r,
        &[
            "submit",
            "--base",
            "main",
            "--head",
            "logger",
            "--agent-tool",
            "test",
            "--out",
            &sub,
        ],
    );
    assert_eq!(code, 0, "{err}");
    // Without evidence the judge rejects at once: no container engine needed.
    let judged = |extra: &[&str]| -> serde_json::Value {
        let mut args = vec!["judge", "--submission", sub.as_str()];
        args.extend(extra);
        let (code, stdout, err) = onus(r, &args);
        assert_eq!(code, 2, "{stdout}\n{err}");
        serde_json::from_str::<serde_json::Value>(&stdout).unwrap()["judgment"].clone()
    };
    let first = judged(&[]);
    let again = judged(&[]);
    let config = first["rotation"]["config"].as_str().unwrap().to_string();
    assert!(config == "a" || config == "b", "{first}");
    assert_eq!(first["rotation"], again["rotation"]);
    assert_eq!(first["rotation"]["forced"], false);
    assert_eq!(first["rotation"]["of"], 2);
    assert!(
        first["judge"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{config}@")),
        "{first}"
    );
    let forced = judged(&["--judge-config", "b"]);
    assert_eq!(forced["rotation"]["config"], "b");
    assert_eq!(forced["rotation"]["forced"], true);
    assert!(forced["judge"].as_str().unwrap().starts_with("b@"));
    let (code, _, err) = onus(r, &["judge", "--submission", &sub, "--judge-config", "c"]);
    assert_eq!(code, 1);
    assert!(err.contains("no judge configuration `c`"), "{err}");
}

#[test]
fn reverts_and_incidents_flow_back_into_the_record() {
    let dir = repo();
    let r = dir.path();
    change(
        r,
        "feature",
        &[("services/orders/src/extra.ts", "export const EXTRA = 1;\n")],
    );
    git(r, &["checkout", "-q", "main"]);
    git(r, &["merge", "-q", "--no-ff", "--no-edit", "feature"]);
    let merged = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(r)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let file = out(r, "outcomes.jsonl");
    for (change, commit) in [("acme/shop#1", merged.as_str()), ("acme/shop#2", "0000000")] {
        let (code, _, err) = onus(
            r,
            &[
                "outcomes",
                "record",
                "--file",
                &file,
                "--change",
                change,
                "--agent",
                "test/model/x",
                "--lane",
                "auto-merge",
                "--result",
                "merged",
                "--commit",
                commit,
            ],
        );
        assert_eq!(code, 0, "{err}");
    }
    git(r, &["revert", "--no-edit", "-m", "1", "HEAD"]);
    let (code, said, err) = onus(r, &["outcomes", "ingest-reverts", "--file", &file]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        said,
        "1 reverts found, 1 recorded against changes on record"
    );
    let (code, _, err) = onus(
        r,
        &[
            "outcomes",
            "incident",
            "--file",
            &file,
            "--change",
            "acme/shop#2",
            "--involved",
            "billing",
            "--note",
            "double charge",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (_, summary, _) = onus(r, &["outcomes", "summary", "--file", &file]);
    let s: serde_json::Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(s["total"]["reverted"], 1, "{s}");
    assert_eq!(s["total"]["incidents"], 1, "{s}");
    let (_, backlog, _) = onus(r, &["outcomes", "backlog", "--file", &file]);
    assert_eq!(backlog, "1\tbilling");
}

#[test]
fn submissions_list_only_granted_escalations() {
    let dir = repo();
    let r = dir.path();
    change(
        r,
        "sms",
        &[(
            "services/notifications/src/sms.ts",
            "export const SMS = 1;\n",
        )],
    );
    // Keys, a token, and a request filed outside the repository.
    let keys = out(r, "keys");
    let (code, _, err) = onus(r, &["token", "keygen", "--out", &keys]);
    assert_eq!(code, 0, "{err}");
    let plan = out(r, "plan.yaml");
    std::fs::write(
        &plan,
        "task: sms\nwrites: [\"services/notifications/**\"]\nagent: {tool: claude-code, model: opus}\n",
    )
    .unwrap();
    let key = format!("{keys}/root.key");
    let public = format!("{keys}/root.pub");
    let (code, token, err) = onus(r, &["token", "mint", "--plan", &plan, "--key", &key]);
    assert_eq!(code, 0, "{err}");
    let (code, request, err) = onus(
        r,
        &[
            "escalate",
            "--task",
            "sms",
            "--scope",
            "write:path:services/orders/src/**",
            "--evidence",
            "rationale:the event needs the phone",
            "--reason",
            "the event needs the phone",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let id = serde_json::from_str::<serde_json::Value>(&request).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let escalations = out(r, "escalations");
    std::fs::create_dir_all(&escalations).unwrap();
    let file = format!("{escalations}/{id}.json");
    std::fs::write(&file, &request).unwrap();

    let sub = out(r, "sub.json");
    let submit = |extra: &[&str]| {
        let mut args = vec![
            "submit",
            "--base",
            "main",
            "--head",
            "sms",
            "--token",
            &token,
            "--escalation",
            &id,
            "--escalations-dir",
            &escalations,
            "--out",
            &sub,
        ];
        args.extend_from_slice(extra);
        onus(r, &args)
    };
    // Ids are verified with the root public key.
    let (code, _, err) = submit(&[]);
    assert_ne!(code, 0);
    assert!(err.contains("pass --key-public"), "{err}");
    // Not granted yet.
    let (code, _, err) = submit(&["--key-public", &public]);
    assert_ne!(code, 0);
    assert!(err.contains("is still open"), "{err}");

    let (code, _, err) = onus(
        r,
        &[
            "escalation",
            "grant",
            &file,
            "--token",
            &token,
            "--key-public",
            &public,
            "--key",
            &key,
            "--by",
            "@lead",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = submit(&["--key-public", &public, "--agent-tool", "codex"]);
    assert_eq!(code, 0, "{err}");
    let s: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sub).unwrap()).unwrap();
    assert_eq!(s["escalations"], serde_json::json!([id]));
    // The token names the agent; the flag does not change it.
    assert_eq!(s["agent"]["tool"], "claude-code");
    assert_eq!(s["agent"]["model"], "opus");
}

#[test]
fn a_local_change_has_its_token_escalations_and_doors_on_record() {
    let dir = repo();
    let r = dir.path();
    change(
        r,
        "sms",
        &[(
            "services/notifications/src/sms.ts",
            "export const SMS = 1;\n",
        )],
    );
    let keys = out(r, "keys");
    let (code, _, err) = onus(r, &["token", "keygen", "--out", &keys]);
    assert_eq!(code, 0, "{err}");
    let plan = out(r, "plan.yaml");
    std::fs::write(
        &plan,
        "task: sms\nwrites: [\"services/notifications/**\"]\nagent: {tool: claude-code, model: opus}\n",
    )
    .unwrap();
    let key = format!("{keys}/root.key");
    let public = format!("{keys}/root.pub");
    // The door log sits beside the escalation requests.
    let audit = out(r, "audit.jsonl");
    let (code, token, err) = onus(
        r,
        &[
            "token", "mint", "--plan", &plan, "--key", &key, "--audit", &audit,
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, request, err) = onus(
        r,
        &[
            "escalate",
            "--task",
            "sms",
            "--scope",
            "write:path:services/orders/src/**",
            "--evidence",
            "rationale:the event needs the phone",
            "--reason",
            "the event needs the phone",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let id = serde_json::from_str::<serde_json::Value>(&request).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let escalations = out(r, "escalations");
    std::fs::create_dir_all(&escalations).unwrap();
    let file = format!("{escalations}/{id}.json");
    std::fs::write(&file, &request).unwrap();
    let with_token = |args: &[&str]| {
        let mut all = args.to_vec();
        all.extend_from_slice(&["--token", &token, "--key-public", &public]);
        onus(r, &all)
    };
    // Policy leaves it to a person, who grants it.
    let (code, _, err) = with_token(&[
        "escalation",
        "decide",
        &file,
        "--key",
        &key,
        "--audit",
        &audit,
    ]);
    assert_eq!(code, 3, "{err}");
    let (code, _, err) = with_token(&[
        "escalation",
        "grant",
        &file,
        "--key",
        &key,
        "--by",
        "@lead",
        "--audit",
        &audit,
    ]);
    assert_eq!(code, 0, "{err}");
    let sub = out(r, "sub.json");
    let (code, _, err) = with_token(&[
        "submit",
        "--base",
        "main",
        "--head",
        "sms",
        "--escalation",
        &id,
        "--escalations-dir",
        &escalations,
        "--out",
        &sub,
    ]);
    assert_eq!(code, 0, "{err}");

    let (code, text, err) = onus(
        r,
        &[
            "provenance",
            "--submission",
            &sub,
            "--audit",
            &audit,
            "--escalations",
            &escalations,
        ],
    );
    assert_eq!(code, 0, "{err}");
    let p: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(p["token"]["source"], "submission", "{p}");
    assert_eq!(p["token"]["task"], "sms");
    assert_eq!(
        p["token"]["rights"][0], "write:path:services/notifications/**",
        "{p}"
    );
    assert_eq!(p["token"]["fingerprint"].as_str().unwrap().len(), 16);
    assert!(
        !text.contains(token.trim()),
        "the token itself is never kept"
    );
    assert_eq!(p["escalations"][0]["id"], id.as_str());
    assert_eq!(p["escalations"][0]["decision"], "granted");
    assert_eq!(p["escalations"][0]["by"], "@lead");
    assert_eq!(p["agent"]["key"], "claude-code/opus/");
    assert_eq!(p["agent"]["detectedFrom"], "submission");
    assert_eq!(p["doors"]["intact"], true);
    let actions: Vec<&str> = p["doors"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["mint", "escalate", "grant"]);
    // The change is the submission's head; the record is local, so open.
    assert_eq!(p["stage"], "open");
    assert_eq!(p["change"], p["commits"]["head"]);
    assert!(
        !p["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["field"] == "token" || g["field"] == "doors")
    );

    // The same from what onus ui and onus mcp keep in .onus.
    let dot = r.join(".onus");
    std::fs::create_dir_all(dot.join("submissions")).unwrap();
    std::fs::create_dir_all(dot.join("escalations")).unwrap();
    std::fs::copy(&sub, dot.join("submissions/4fa91c.json")).unwrap();
    std::fs::copy(&audit, dot.join("audit.jsonl")).unwrap();
    std::fs::copy(&file, dot.join(format!("escalations/{id}.json"))).unwrap();
    let (code, text, err) = onus(r, &["provenance", "--submission-id", "4fa91c"]);
    assert_eq!(code, 0, "{err}");
    let kept: serde_json::Value = serde_json::from_str(&text).unwrap();
    for field in ["token", "escalations", "doors", "change"] {
        assert_eq!(kept[field], p[field], "{field}");
    }

    // A token that does not verify is an error, not a gap.
    let (code, _, err) = onus(
        r,
        &[
            "provenance",
            "--submission",
            &sub,
            "--token",
            "not-a-token",
            "--key-public",
            &public,
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("does not verify"), "{err}");
}

#[test]
fn risk_inputs_and_new_floors_raise_lanes() {
    let dir = repo();
    let r = dir.path();
    // Additive changes to logger may auto-merge too, so the inputs are what
    // raise them.
    let yaml = std::fs::read_to_string(r.join("onus.yaml"))
        .unwrap()
        .replace(
            "kinds: [internal], components: [logger]",
            "kinds: [internal, additive], components: [logger]",
        );
    std::fs::write(r.join("onus.yaml"), yaml).unwrap();
    git(r, &["commit", "-q", "-am", "policy"]);
    let input = |c: &serde_json::Value, source: &str, text: &str| {
        c["applied"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["source"] == source && a["reason"].as_str().unwrap().contains(text))
    };
    let logger_ts = std::fs::read_to_string(r.join("packages/logger/src/logger.ts")).unwrap();

    // Internals that a test exercises still auto-merge.
    let logger = format!(
        "{logger_ts}\nfunction redact(message: string): string {{\n  return message.replace(/[0-9]+/g, '#');\n}}\nvoid redact;\n"
    );
    change(r, "logger", &[("packages/logger/src/logger.ts", &logger)]);
    let c = lane_of(r, "logger");
    assert_eq!(c["lane"], "auto-merge", "{c}");
    assert!(!input(&c, "input", ""), "{c}");

    // A file no test imports.
    change(
        r,
        "untested",
        &[(
            "packages/logger/src/redact.ts",
            "export function redact(s: string): string {\n  return s;\n}\n",
        )],
    );
    let c = lane_of(r, "untested");
    assert_eq!(c["lane"], "judge", "{c}");
    assert!(input(&c, "input", "no test exercises"), "{c}");

    // A body-only edit of `createLogger`, which billing, notifications and
    // orders use.
    let body = logger_ts.replace("console.log(", "console.info(");
    change(r, "wider", &[("packages/logger/src/logger.ts", &body)]);
    let c = lane_of(r, "wider");
    assert_eq!(c["lane"], "judge", "{c}");
    assert!(
        input(
            &c,
            "input",
            "is used by 3 other components (billing, notifications and orders); 3 or more go to judge"
        ),
        "{c}"
    );

    // Third-party code new to the repository.
    let pkg = std::fs::read_to_string(r.join("packages/logger/package.json"))
        .unwrap()
        .replace(
            "\"private\": true,",
            "\"private\": true,\n  \"dependencies\": { \"left-pad\": \"1.3.0\" },",
        );
    let format = std::fs::read_to_string(r.join("packages/logger/src/format.ts")).unwrap();
    change(
        r,
        "left-pad",
        &[
            ("packages/logger/package.json", &pkg),
            (
                "packages/logger/src/format.ts",
                &format!("import leftPad from \"left-pad\";\nvoid leftPad;\n{format}"),
            ),
        ],
    );
    let c = lane_of(r, "left-pad");
    assert_eq!(c["lane"], "human", "{c}");
    assert!(
        input(&c, "floor", "third-party code new to this repository"),
        "{c}"
    );

    // The history of the components a change touches.
    let sub = out(r, "logger.json");
    let outcomes = out(r, "outcomes.jsonl");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let classify = |days_ago: u64, change: &str| {
        let line = serde_json::json!({
            "at": now - days_ago * 86_400, "change": "acme/shop#1", "agent": "other//",
            "lane": "judge", "result": "incident", "components": ["logger"],
        });
        std::fs::write(&outcomes, format!("{line}\n")).unwrap();
        let (code, out, err) = onus(
            r,
            &[
                "classify",
                "--submission",
                &sub,
                "--outcomes",
                &outcomes,
                "--change",
                change,
            ],
        );
        assert_eq!(code, 0, "{err}");
        serde_json::from_str::<serde_json::Value>(&out).unwrap()
    };
    let c = classify(10, "acme/shop#2");
    assert_eq!(c["lane"], "human", "{c}");
    assert!(
        input(
            &c,
            "input",
            "`logger` had 1 incident in the last 90 days (acme/shop#1)"
        ),
        "{c}"
    );
    assert_eq!(classify(200, "acme/shop#2")["lane"], "auto-merge");
    assert_eq!(classify(10, "acme/web#2")["lane"], "auto-merge");

    // The same edit, once `createLogger` is a declared contract, is a rule
    // of the game.
    git(r, &["checkout", "-q", "main"]);
    let yaml = std::fs::read_to_string(r.join("onus.yaml"))
        .unwrap()
        .replace(
            "\ncontracts:\n",
            "\ncontracts:\n  Logger:\n    symbol: \"logger:src/logger.ts#createLogger\"\n",
        );
    std::fs::write(r.join("onus.yaml"), yaml).unwrap();
    git(r, &["commit", "-q", "-am", "contract"]);
    change(r, "declared", &[("packages/logger/src/logger.ts", &body)]);
    let report = out(r, "declared.report.json");
    let (code, json, err) = onus(
        r,
        &[
            "report", "--base", "main", "--head", "declared", "--format", "json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    std::fs::write(&report, &json).unwrap();
    assert!(
        json.contains("\"subkind\": \"declared-contract-edited\""),
        "{json}"
    );
    let c = lane_of(r, "declared");
    assert_eq!(c["lane"], "human", "{c}");
    assert!(input(&c, "floor", "changes the rules of the game"), "{c}");

    // Outcomes recorded by hand can name the components.
    let file = out(r, "recorded.jsonl");
    let (code, _, err) = onus(
        r,
        &[
            "outcomes",
            "record",
            "--file",
            &file,
            "--change",
            "acme/shop#3",
            "--agent",
            "test//",
            "--lane",
            "judge",
            "--result",
            "merged",
            "--component",
            "logger",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains(r#""components":["logger"]"#)
    );
}

#[test]
fn a_learned_model_raises_lanes_and_never_lowers() {
    let dir = repo();
    let r = dir.path();
    let yaml = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    std::fs::write(
        r.join("onus.yaml"),
        format!("{yaml}  learned: {{ judge: 0.6, human: 0.85 }}\n"),
    )
    .unwrap();
    git(r, &["commit", "-q", "-am", "learned lanes"]);

    // A logger internal (auto-merge by the policy), and notifications
    // starting to depend on orders (a new dependency: the judge).
    let logger_ts = std::fs::read_to_string(r.join("packages/logger/src/logger.ts")).unwrap();
    let logger = format!(
        "{logger_ts}\nfunction redact(message: string): string {{\n  return message.replace(/[0-9]+/g, '#');\n}}\nvoid redact;\n"
    );
    let logger_files = [("packages/logger/src/logger.ts", logger.as_str())];
    let pkg = std::fs::read_to_string(r.join("services/notifications/package.json"))
        .unwrap()
        .replace(
            "\"@shop/money\": \"workspace:*\",",
            "\"@shop/money\": \"workspace:*\",\n    \"@shop/orders\": \"workspace:*\",",
        );
    let eta = "import { estimateDelivery } from \"@shop/orders\";\n\nexport function etaText(days: number): string {\n  return String(estimateDelivery(new Date(), days));\n}\n";
    let dep_files = [
        ("services/notifications/package.json", pkg.as_str()),
        ("services/notifications/src/eta.ts", eta),
    ];
    change(r, "logger", &logger_files);
    change(r, "dep", &dep_files);

    // Their features, as classify writes them.
    let features = |branch: &str| {
        let (code, json, err) = onus(
            r,
            &[
                "report", "--base", "main", "--head", branch, "--format", "json",
            ],
        );
        assert_eq!(code, 0, "{err}");
        let report = out(r, &format!("{branch}.report.json"));
        std::fs::write(&report, json).unwrap();
        let (code, c, err) = onus(r, &["classify", "--report", &report]);
        assert_eq!(code, 0, "{err}");
        let c: serde_json::Value = serde_json::from_str(&c).unwrap();
        assert_eq!(c["learned"]["status"], "missing", "{c}");
        assert_eq!(c["features"]["set"], 1, "{c}");
        c["features"].clone()
    };
    let (logger_f, dep_f) = (features("logger"), features("dep"));
    assert_ne!(logger_f, dep_f);

    // Settled outcomes: nearly every dependency-shaped change went wrong,
    // nearly no logger one did.
    let base = 1_700_000_000u64;
    let mut lines = Vec::new();
    for i in 0..60u64 {
        let dep = i % 2 == 0;
        let problem = if dep { i != 0 } else { i == 1 };
        let result = match (problem, i % 4) {
            (false, _) => "merged",
            (true, 0 | 1) => "incident",
            (true, _) => "reverted",
        };
        lines.push(serde_json::json!({
            "at": base + i * 3600, "change": format!("acme/shop#{}", i + 1), "agent": "test//",
            "lane": "judge", "result": result,
            "features": if dep { &dep_f } else { &logger_f },
        }));
    }
    // A later record without features: every merge above has settled by it.
    lines.push(serde_json::json!({
        "at": base + 30 * 86_400, "change": "acme/shop#100", "agent": "test//",
        "lane": "judge", "result": "open",
    }));
    let text =
        |lines: &[serde_json::Value]| lines.iter().map(|l| format!("{l}\n")).collect::<String>();
    let outcomes = out(r, "outcomes.jsonl");
    std::fs::write(&outcomes, text(&lines)).unwrap();

    let model_file = r.join("onus.lanes-model.json");
    let (code, dry, err) = onus(r, &["lanes", "train", "--outcomes", &outcomes, "--dry-run"]);
    assert_eq!(code, 0, "{err}");
    assert!(!model_file.exists());
    let dry: serde_json::Value = serde_json::from_str(&dry).unwrap();
    assert_eq!(dry["written"], false);
    let (code, trained, err) = onus(r, &["lanes", "train", "--outcomes", &outcomes]);
    assert_eq!(code, 0, "{err}");
    let trained: serde_json::Value = serde_json::from_str(&trained).unwrap();
    assert_eq!(trained["written"], true, "{trained}");
    assert_eq!(trained["examples"], 60, "{trained}");
    assert_eq!(trained["positives"], 30, "{trained}");
    assert_eq!(trained["skipped"]["noFeatures"], 1, "{trained}");
    assert_eq!(trained["out"], "onus.lanes-model.json");
    assert_eq!(trained["model"], dry["model"]);
    let id = trained["model"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 12);
    let bytes = std::fs::read(&model_file).unwrap();

    // The same records in another order give the same file, byte for byte;
    // so does training outside a repository with --config and --out.
    let elsewhere = tempfile::tempdir().unwrap();
    let e = elsewhere.path();
    let mut reversed = lines.clone();
    reversed.reverse();
    std::fs::write(e.join("outcomes.jsonl"), text(&reversed)).unwrap();
    std::fs::write(
        e.join("onus.yaml"),
        "version: 1\nlanes:\n  learned: { judge: 0.6, human: 0.85 }\n",
    )
    .unwrap();
    let (code, _, err) = onus(
        e,
        &[
            "lanes",
            "train",
            "--outcomes",
            "outcomes.jsonl",
            "--config",
            "onus.yaml",
            "--out",
            "m/lanes-model.json",
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(std::fs::read(e.join("m/lanes-model.json")).unwrap(), bytes);

    // Not committed yet: the base commit has no model, so rules decide.
    let c = lane_of(r, "dep");
    assert_eq!(c["lane"], "judge", "{c}");
    assert_eq!(c["learned"]["status"], "missing", "{c}");
    assert!(
        c["learned"]["note"]
            .as_str()
            .unwrap()
            .contains("the lane comes from rules only"),
        "{c}"
    );

    // A model handed to `onus ci pr` applies without being committed.
    let rec = out(r, "records");
    let ci_out = out(r, "ci");
    let (code, summary, err) = onus(
        r,
        &[
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "dep",
            "--change",
            "acme/shop#70",
            "--records",
            &rec,
            "--out",
            &ci_out,
            "--model",
            model_file.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{err}");
    let summary: serde_json::Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(summary["lane"], "human", "{summary}");
    assert!(summary["risk"].as_f64().unwrap() >= 0.85, "{summary}");
    let c: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&ci_out).join("classification.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(c["learned"]["status"], "applied", "{c}");
    assert_eq!(c["learned"]["model"], id.as_str());
    let md = std::fs::read_to_string(Path::new(&ci_out).join("lane.md")).unwrap();
    assert!(
        md.contains("- model → `human`: the learned model puts"),
        "{md}"
    );
    assert!(md.contains("**Learned risk:** "), "{md}");
    assert!(md.contains(&format!("model `{id}`")), "{md}");
    let pulls = std::fs::read_to_string(Path::new(&rec).join("pulls.jsonl")).unwrap();
    assert!(pulls.contains("\"features\":{\"set\":1"), "{pulls}");
    // A model file that does not parse is ignored, never an error.
    let junk = out(r, "junk-lanes-model.json");
    std::fs::write(&junk, "not json").unwrap();
    let (code, summary, err) = onus(
        r,
        &[
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "dep",
            "--change",
            "acme/shop#71",
            "--records",
            &rec,
            "--out",
            &ci_out,
            "--model",
            &junk,
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(summary.contains("\"lane\":\"judge\""), "{summary}");
    let c: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(Path::new(&ci_out).join("classification.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(c["learned"]["status"], "invalid", "{c}");

    // Committed on main: changes based on it use it.
    git(r, &["checkout", "-q", "main"]);
    git(r, &["add", "onus.lanes-model.json"]);
    git(r, &["commit", "-q", "-m", "lanes model"]);
    change(r, "logger", &logger_files);
    change(r, "dep", &dep_files);
    let c = lane_of(r, "logger");
    assert_eq!(c["lane"], "auto-merge", "{c}");
    assert_eq!(c["learned"]["status"], "applied", "{c}");
    assert_eq!(c["learned"]["raised"], false, "{c}");
    assert!(c["learned"]["risk"].as_f64().unwrap() < 0.6, "{c}");
    let c = lane_of(r, "dep");
    assert_eq!(c["lane"], "human", "{c}");
    assert_eq!(c["learned"]["raised"], true, "{c}");
    let applied = c["applied"].as_array().unwrap();
    let model_step = applied.iter().find(|a| a["source"] == "model").unwrap();
    assert_eq!(model_step["lane"], "human");
    assert!(
        model_step["reason"]
            .as_str()
            .unwrap()
            .contains(&format!("model {id}, trained on 60 outcomes")),
        "{c}"
    );
    // The rules' reasons are all still there.
    assert!(
        applied
            .iter()
            .any(|a| a["source"] == "floor" && a["lane"] == "judge"),
        "{c}"
    );
    // A floor already sends payments code to a person.
    change(
        r,
        "billing",
        &[(
            "services/billing/src/discount.ts",
            "export const NOTE = 'x';\n",
        )],
    );
    assert_eq!(lane_of(r, "billing")["lane"], "human");

    // Deleting the model in the change does not escape it: the model is read
    // from the base, and the deletion is a rule of the game.
    git(r, &["checkout", "-q", "-B", "drop", "dep"]);
    git(r, &["rm", "-q", "onus.lanes-model.json"]);
    git(r, &["commit", "-q", "-m", "drop the model"]);
    let c = lane_of(r, "drop");
    assert_eq!(c["lane"], "human", "{c}");
    assert_eq!(c["learned"]["status"], "applied", "{c}");
    assert_eq!(c["learned"]["model"], id.as_str());
    let sub = std::fs::read_to_string(out(r, "drop.json")).unwrap();
    assert!(
        sub.contains("\"subkind\": \"lanes-model-changed\""),
        "{sub}"
    );

    // Too little data.
    let few = out(r, "few.jsonl");
    std::fs::write(&few, text(&lines[..10])).unwrap();
    let (code, _, err) = onus(r, &["lanes", "train", "--outcomes", &few, "--dry-run"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("too little data") && err.contains("at least 50"),
        "{err}"
    );
}
