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
    // An AWS-shaped key, assembled here so this file holds no secret.
    let key = format!("export const KEY = '{}{}';\n", "AKIA", "IOSFODNN7EXAMPLE");
    change(r, "secret", &[("services/orders/src/key.ts", &key)]);
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
