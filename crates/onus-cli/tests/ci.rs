//! `onus ci`: a pull request's life recorded without anyone typing an
//! outcome. Classified while open, approved by comment, merged, audited,
//! reverted; the records travel through a branch of the repository.

mod common;

use std::path::Path;
use std::process::Command;

use common::{apply_overlay, copy_dir, fixture};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
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
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn onus(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "onus {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_pull_request_is_recorded_from_open_to_revert() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let origin = t.join("origin.git");
    git(
        t,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    git(&r, &["push", "-q", "origin", "main"]);
    git(&r, &["checkout", "-q", "-b", "codex/sms"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), &r);
    git(&r, &["add", "-A"]);
    git(
        &r,
        &[
            "commit",
            "-q",
            "-m",
            "Text customers when their order ships",
        ],
    );

    let rec = t.join("records");
    let out = t.join("out");
    let rs = rec.to_str().unwrap();
    onus(&r, &["ci", "records", "pull", "--dir", rs]);

    // Open: the agent comes from the branch, the lane from the policy and floors.
    let body = t.join("body.md");
    std::fs::write(&body, "Texts customers.\n\n```onus-intent\nsummary: SMS on shipping\ntouches: [notifications]\n```\n").unwrap();
    let change = "acme/shop#7";
    let summary: serde_json::Value = serde_json::from_str(&onus(
        &r,
        &[
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/sms",
            "--change",
            change,
            "--branch",
            "codex/sms",
            "--author",
            "mike",
            "--body-file",
            body.to_str().unwrap(),
            "--records",
            rs,
            "--out",
            out.to_str().unwrap(),
        ],
    ))
    .unwrap();
    assert_eq!(summary["agent"], "codex//");
    assert_eq!(summary["lane"], "human", "{summary}");
    // The record names the components the change touched.
    let pulls = lines(&rec.join("pulls.jsonl"));
    assert_eq!(
        pulls[0]["components"],
        serde_json::json!(["notifications", "user-preferences"])
    );
    let md = std::fs::read_to_string(out.join("lane.md")).unwrap();
    assert!(
        md.contains("/onus approve new-external-service:external:acme-sms@notifications"),
        "{md}"
    );
    assert!(md.contains("0 merged on record"), "{md}");

    // Approving a row needs write access.
    let say = |text: &str, association: &str| -> serde_json::Value {
        let f = t.join("comment.md");
        std::fs::write(&f, text).unwrap();
        serde_json::from_str(&onus(
            &r,
            &[
                "ci",
                "comment",
                "--change",
                change,
                "--body-file",
                f.to_str().unwrap(),
                "--author",
                "lead",
                "--association",
                association,
                "--records",
                rs,
            ],
        ))
        .unwrap()
    };
    let refused = say(
        "/onus approve new-external-service:external:acme-sms@notifications",
        "CONTRIBUTOR",
    );
    assert_eq!(refused["ok"], false);
    let approved = say(
        "/onus approve new-external-service:external:acme-sms@notifications",
        "MEMBER",
    );
    assert_eq!(approved["ok"], true, "{approved}");
    assert_eq!(approved["rerun"], true);
    let pulls = lines(&rec.join("pulls.jsonl"));
    assert_eq!(pulls.last().unwrap()["approvals"][0]["by"], "@lead");
    // No judge decided this change, so no judge configuration is recorded.
    assert!(pulls[0].get("judge").is_none(), "{:?}", pulls[0]);
    assert_eq!(
        say("just a comment", "MEMBER")["command"],
        serde_json::Value::Null
    );

    // The next run shows the approval and no longer offers that row. It
    // runs in the same second as the approval, so it writes the same line.
    let approved_at = pulls.last().unwrap()["at"].to_string();
    onus(
        &r,
        &[
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/sms",
            "--change",
            change,
            "--branch",
            "codex/sms",
            "--at",
            &approved_at,
            "--records",
            rs,
            "--out",
            out.to_str().unwrap(),
        ],
    );
    let pulls = std::fs::read_to_string(rec.join("pulls.jsonl")).unwrap();
    let pulls: Vec<&str> = pulls.lines().collect();
    assert_eq!(pulls.len(), 3);
    assert_eq!(pulls[1], pulls[2]);
    let md = std::fs::read_to_string(out.join("lane.md")).unwrap();
    assert!(
        md.contains("**Approved:**") && !md.contains("/onus approve new-external-service"),
        "{md}"
    );

    // The records travel through a branch: another clone sees them.
    onus(
        &r,
        &[
            "ci",
            "records",
            "push",
            "--dir",
            rs,
            "--message",
            "acme/shop#7 classified",
        ],
    );
    let other = t.join("other");
    git(
        t,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    let rec2 = t.join("records2");
    onus(
        &other,
        &["ci", "records", "pull", "--dir", rec2.to_str().unwrap()],
    );
    // Every record arrives, the approval and the run it triggered too,
    // though they are the same line.
    assert_eq!(lines(&rec2.join("pulls.jsonl")).len(), 3);
    assert_eq!(
        std::fs::read_to_string(rec2.join("pulls.jsonl")).unwrap(),
        std::fs::read_to_string(rec.join("pulls.jsonl")).unwrap()
    );

    // Merged: the last record becomes an outcome, once.
    git(&r, &["checkout", "-q", "main"]);
    git(&r, &["merge", "-q", "--squash", "codex/sms"]);
    git(
        &r,
        &[
            "commit",
            "-q",
            "-m",
            "Text customers when their order ships (#7)",
        ],
    );
    let merged = git(&r, &["rev-parse", "HEAD"]);
    for _ in 0..2 {
        onus(
            &r,
            &[
                "ci",
                "closed",
                "--change",
                change,
                "--merged",
                "--commit",
                &merged,
                "--records",
                rs,
            ],
        );
    }
    let outcomes = lines(&rec.join("outcomes.jsonl"));
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0]["result"], "merged");
    assert_eq!(outcomes[0]["agent"], "codex//");
    assert_eq!(outcomes[0]["lane"], "human");
    assert!(outcomes[0]["judge"].is_null(), "{:?}", outcomes[0]);
    assert_eq!(
        outcomes[0]["components"],
        serde_json::json!(["notifications", "user-preferences"])
    );

    // Audited after the fact, and an incident.
    assert_eq!(
        say("/onus audit miss the opt-out was ignored", "OWNER")["ok"],
        true
    );
    assert_eq!(
        say(
            "/onus incident SMS sent twice involved: notifications",
            "OWNER"
        )["ok"],
        true
    );

    // Reverted on the default branch.
    git(&r, &["revert", "--no-edit", &merged]);
    assert_eq!(
        onus(&r, &["ci", "push", "--records", rs]),
        "1 reverts found, 1 recorded"
    );
    let summary: serde_json::Value = serde_json::from_str(&onus(
        &r,
        &[
            "outcomes",
            "summary",
            "--file",
            rec.join("outcomes.jsonl").to_str().unwrap(),
        ],
    ))
    .unwrap();
    let codex = &summary["byAgent"]["codex//"];
    assert_eq!(codex["reverted"], 1, "{summary}");
    // The incident still counts after the revert.
    assert_eq!(codex["incidents"], 1, "{summary}");
    assert_eq!(summary["total"]["audited"], 1);
    let outcomes = lines(&rec.join("outcomes.jsonl"));
    let reverted = outcomes.last().unwrap();
    assert_eq!(reverted["result"], "reverted");
    assert_eq!(reverted["components"], outcomes[0]["components"]);

    // The next change to notifications carries that history.
    git(&r, &["checkout", "-q", "-b", "codex/sms-again"]);
    std::fs::write(
        r.join("services/notifications/src/again.ts"),
        "export const AGAIN = 1;\n",
    )
    .unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "Again"]);
    onus(
        &r,
        &[
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/sms-again",
            "--change",
            "acme/shop#8",
            "--records",
            rs,
            "--out",
            out.to_str().unwrap(),
        ],
    );
    let md = std::fs::read_to_string(out.join("lane.md")).unwrap();
    assert!(
        md.contains(
            "- input → `human`: `notifications` had 1 incident and 1 revert in the last 90 days (acme/shop#7)"
        ),
        "{md}"
    );
    git(&r, &["checkout", "-q", "main"]);

    // Two runs pushing at once: both sets of lines end up on the branch.
    std::fs::write(
        rec2.join("outcomes.jsonl"),
        r#"{"at":1,"change":"acme/shop#9","agent":"person/x/","lane":"judge","result":"merged"}"#
            .to_string()
            + "\n",
    )
    .unwrap();
    onus(
        &other,
        &["ci", "records", "push", "--dir", rec2.to_str().unwrap()],
    );
    onus(&r, &["ci", "records", "push", "--dir", rs]);
    let rec3 = t.join("records3");
    onus(
        &other,
        &["ci", "records", "pull", "--dir", rec3.to_str().unwrap()],
    );
    let all = std::fs::read_to_string(rec3.join("outcomes.jsonl")).unwrap();
    assert!(
        all.contains("acme/shop#9") && all.contains("reverted by"),
        "{all}"
    );
}

#[test]
fn records_written_after_the_fact_keep_their_time() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    git(&r, &["checkout", "-q", "-b", "codex/sms"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), &r);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "Text customers"]);
    let rec = t.join("records");
    let rs = rec.to_str().unwrap();
    let change = "acme/shop#3";
    // A pull request merged last spring, recorded today.
    onus(
        &r,
        &[
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/sms",
            "--change",
            change,
            "--branch",
            "codex/sms",
            "--records",
            rs,
            "--out",
            t.join("out").to_str().unwrap(),
            "--at",
            "1745000000",
        ],
    );
    onus(
        &r,
        &[
            "ci",
            "closed",
            "--change",
            change,
            "--merged",
            "--records",
            rs,
            "--at",
            "1745090000",
        ],
    );
    assert_eq!(lines(&rec.join("pulls.jsonl"))[0]["at"], 1_745_000_000);
    assert_eq!(lines(&rec.join("outcomes.jsonl"))[0]["at"], 1_745_090_000);
}

#[test]
fn a_judged_pull_request_records_the_judge_that_decided() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    let mut yaml = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    yaml.push_str("\nlanes:\n  default: judge\n  auditRate: 0\n  minRecord: 0\n");
    std::fs::write(r.join("onus.yaml"), yaml).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    git(&r, &["checkout", "-q", "-b", "codex/extra"]);
    std::fs::write(
        r.join("services/orders/src/extra.ts"),
        "export const EXTRA = 1;\n",
    )
    .unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "An extra constant"]);
    let rec = t.join("records");
    let rs = rec.to_str().unwrap();
    let out = t.join("out");
    let change = "acme/shop#5";
    // Without evidence the judge rejects, so no container engine is needed.
    let summary: serde_json::Value = serde_json::from_str(&onus(
        &r,
        &[
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/extra",
            "--change",
            change,
            "--branch",
            "codex/extra",
            "--judge",
            "--records",
            rs,
            "--out",
            out.to_str().unwrap(),
        ],
    ))
    .unwrap();
    assert_eq!(summary["lane"], "judge", "{summary}");
    assert_eq!(summary["verdict"], "reject", "{summary}");
    let judgment: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("judgment.json")).unwrap()).unwrap();
    let pulls = lines(&rec.join("pulls.jsonl"));
    assert_eq!(pulls[0]["verdict"], "reject");
    assert_eq!(pulls[0]["judge"], judgment["judge"], "{:?}", pulls[0]);

    // The outcome keeps it, and the judge configuration has a record.
    onus(
        &r,
        &[
            "ci",
            "closed",
            "--change",
            change,
            "--merged",
            "--records",
            rs,
        ],
    );
    let outcomes = lines(&rec.join("outcomes.jsonl"));
    assert_eq!(outcomes[0]["judge"], judgment["judge"]);
    let s: serde_json::Value = serde_json::from_str(&onus(
        &r,
        &[
            "outcomes",
            "summary",
            "--file",
            rec.join("outcomes.jsonl").to_str().unwrap(),
        ],
    ))
    .unwrap();
    let id = judgment["judge"].as_str().unwrap();
    assert_eq!(s["byJudge"][id]["changes"], 1, "{s}");
}
