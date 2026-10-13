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
    // And what the change looked like, for the learned lane model.
    assert_eq!(pulls[0]["features"]["set"], 1, "{:?}", pulls[0]);
    assert!(
        pulls[0]["features"]["components"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("notifications")),
        "{:?}",
        pulls[0]
    );
    let md = std::fs::read_to_string(out.join("lane.md")).unwrap();
    assert!(
        md.contains("/onus approve new-external-service:external:acme-sms@notifications"),
        "{md}"
    );
    assert!(md.contains("0 merged on record"), "{md}");
    // Its provenance while open: who made it and how Onus knew, the lane and
    // the intent, and what this run could not vouch for.
    let open: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("provenance.json")).unwrap())
            .unwrap();
    assert_eq!(open["stage"], "open", "{open}");
    assert_eq!(open["approval"]["by"], "pending");
    assert_eq!(open["agent"]["key"], "codex//");
    assert_eq!(open["agent"]["detectedFrom"], "branch");
    assert_eq!(open["lane"]["lane"], "human");
    assert!(
        open["intent"]["check"]["stated"]
            .as_str()
            .unwrap()
            .contains("SMS"),
        "{open}"
    );
    assert_eq!(open["intent"]["source"], "pull-request");
    assert!(
        open["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["field"] == "token"),
        "{open}"
    );

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
    // runs in the same second as the approval, on the same pull request, so
    // it writes the same line.
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
            "--author",
            "mike",
            "--body-file",
            body.to_str().unwrap(),
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
                "--merged-by",
                "mike",
                "--records",
                rs,
            ],
        );
    }
    let outcomes = lines(&rec.join("outcomes.jsonl"));
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0]["result"], "merged");
    assert_eq!(outcomes[0]["mergedBy"], "@mike");
    assert_eq!(outcomes[0]["agent"], "codex//");
    assert_eq!(outcomes[0]["lane"], "human");
    assert!(outcomes[0]["judge"].is_null(), "{:?}", outcomes[0]);
    // A person reviewed it, so it is never sampled for an audit.
    assert!(outcomes[0].get("sampled").is_none(), "{:?}", outcomes[0]);
    assert_eq!(
        outcomes[0]["components"],
        serde_json::json!(["notifications", "user-preferences"])
    );
    let last_pull = lines(&rec.join("pulls.jsonl")).pop().unwrap();
    assert_eq!(outcomes[0]["features"], last_pull["features"]);
    assert!(outcomes[0]["features"].is_object());

    // Its provenance once merged, rebuilt from the records: the facts from
    // while it was open are kept.
    let provenance = |file: &str| -> serde_json::Value {
        let to = t.join(file);
        onus(
            &r,
            &[
                "provenance",
                "--change",
                change,
                "--records",
                rs,
                "--from",
                out.join("provenance.json").to_str().unwrap(),
                "--out",
                to.to_str().unwrap(),
            ],
        );
        serde_json::from_str(&std::fs::read_to_string(to).unwrap()).unwrap()
    };
    let from: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("provenance.json")).unwrap())
            .unwrap();
    let p = provenance("merged.json");
    assert_eq!(p["stage"], "merged", "{p}");
    assert_eq!(p["approval"]["by"], "person");
    assert_eq!(p["approval"]["mergedBy"], "@mike");
    assert_eq!(
        p["approval"]["people"],
        serde_json::json!(["@lead", "@mike"])
    );
    assert_eq!(p["rollout"]["commit"], merged.as_str());
    assert_eq!(p["commits"]["merge"], merged.as_str());
    assert_eq!(p["lane"]["applied"], from["lane"]["applied"]);
    assert!(!p["lane"]["applied"].as_array().unwrap().is_empty());
    assert_eq!(p["intent"], from["intent"]);
    assert_eq!(p["agent"]["detectedFrom"], "branch");

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
    assert_eq!(summary["awaitingAudit"], 0);
    assert!(summary["sampledMissRate"].is_null(), "{summary}");
    let outcomes = lines(&rec.join("outcomes.jsonl"));
    let reverted = outcomes.last().unwrap();
    assert_eq!(reverted["result"], "reverted");
    assert_eq!(reverted["components"], outcomes[0]["components"]);
    assert_eq!(reverted["features"], outcomes[0]["features"]);

    // The rollout holds every outcome in order: merged, audited, the
    // incident and the revert.
    let p = provenance("reverted.json");
    assert_eq!(p["stage"], "merged");
    assert_eq!(p["rollout"]["result"], "reverted");
    let history: Vec<&str> = p["rollout"]["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["result"].as_str().unwrap())
        .collect();
    assert_eq!(history, ["merged", "merged", "incident", "reverted"]);
    assert_eq!(
        (&p["rollout"]["audited"], &p["rollout"]["missed"]),
        (&serde_json::json!(true), &serde_json::json!(true))
    );
    assert_eq!(p["approval"]["mergedBy"], "@mike");
    // The digest says whether the record was changed.
    let file = t.join("reverted.json");
    let verify = || {
        Command::new(env!("CARGO_BIN_EXE_onus"))
            .args(["provenance", "--verify", file.to_str().unwrap()])
            .output()
            .unwrap()
            .status
            .code()
    };
    assert_eq!(verify(), Some(0));
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("\"@mike\"", "\"@someone\"")).unwrap();
    assert_eq!(verify(), Some(1));

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

    // `/onus caught` from someone with write access is recorded: on the
    // outcome once there is one, else on the pull record.
    let count = |f: &str| lines(&rec.join(f)).len();
    let (outcomes_before, pulls_before) = (count("outcomes.jsonl"), count("pulls.jsonl"));
    let caught = say("/onus caught the new vendor", "NONE");
    assert_eq!(
        (&caught["ok"], &caught["recorded"]),
        (&serde_json::json!(true), &serde_json::json!(false))
    );
    assert_eq!(count("outcomes.jsonl"), outcomes_before);
    let caught = say("/onus caught the new vendor", "MEMBER");
    assert_eq!(caught["recorded"], true, "{caught}");
    assert_eq!(caught["note"], "the new vendor");
    let outcomes = lines(&rec.join("outcomes.jsonl"));
    assert_eq!(outcomes.len(), outcomes_before + 1);
    let last = outcomes.last().unwrap();
    assert_eq!(last["caught"], true, "{last}");
    assert_eq!(last["result"], "reverted");
    assert_eq!(last["note"], "caught by @lead: the new vendor");
    assert_eq!(last["features"], outcomes[0]["features"]);
    let f = t.join("comment.md");
    std::fs::write(&f, "/onus caught the retry loop").unwrap();
    let open_pr: serde_json::Value = serde_json::from_str(&onus(
        &r,
        &[
            "ci",
            "comment",
            "--change",
            "acme/shop#8",
            "--body-file",
            f.to_str().unwrap(),
            "--author",
            "lead",
            "--association",
            "OWNER",
            "--records",
            rs,
        ],
    ))
    .unwrap();
    assert_eq!(open_pr["recorded"], true, "{open_pr}");
    let pulls = lines(&rec.join("pulls.jsonl"));
    assert_eq!(pulls.len(), pulls_before + 1);
    assert_eq!(pulls.last().unwrap()["change"], "acme/shop#8");
    assert_eq!(pulls.last().unwrap()["caught"], true);
    git(&r, &["checkout", "-q", "main"]);

    // Production signals travel with them.
    let signal = t.join("signal.json");
    std::fs::write(
        &signal,
        r#"{"id":"datadog:1","kind":"metric","components":["notifications"]}"#,
    )
    .unwrap();
    onus(
        &r,
        &[
            "signals",
            "ingest",
            "--records",
            rs,
            "--file",
            signal.to_str().unwrap(),
        ],
    );

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
    let signals = lines(&rec3.join("signals.jsonl"));
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0]["id"], "datadog:1");
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

#[test]
fn a_held_out_report_goes_beside_the_judgment_for_people() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    let mut yaml = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    yaml.push_str(
        "\nlanes:\n  default: judge\n  auditRate: 0\n  minRecord: 0\n  judges:\n    - name: a\n    - name: b\n",
    );
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
    let suite = t.join("suite");
    std::fs::create_dir_all(&suite).unwrap();
    std::fs::write(
        suite.join("held-out.yaml"),
        "version: 1\nchecks:\n  - { id: orders-money, command: \"true\", covers: [orders, money] }\n  - { id: smoke, command: \"true\" }\n",
    )
    .unwrap();
    let rec = t.join("records");
    let out = t.join("out");
    let pr = |held_out: &Path, judge: bool| {
        let mut args = vec![
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/extra",
            "--change",
            "acme/shop#6",
            "--records",
            rec.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--held-out",
            held_out.to_str().unwrap(),
        ];
        if judge {
            args.push("--judge");
        }
        Command::new(env!("CARGO_BIN_EXE_onus"))
            .current_dir(&r)
            .args(args)
            .output()
            .unwrap()
    };
    // Without evidence the judge rejects before the held-out checks: no
    // container engine is needed.
    let done = pr(&suite, true);
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let judgment: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("judgment.json")).unwrap()).unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("held-out.json")).unwrap()).unwrap();
    let config = judgment["rotation"]["config"].as_str().unwrap();
    assert!(
        judgment["judge"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{config}@")),
        "{judgment}"
    );
    assert_eq!(report["judge"], judgment["judge"]);
    assert_eq!(judgment["heldOut"]["selected"], 2, "{judgment}");
    assert_eq!(judgment["heldOut"]["ran"], 0, "{judgment}");
    assert_eq!(report["checks"][0]["id"], "orders-money", "{report}");
    assert_eq!(report["checks"][0]["why"], "combination", "{report}");
    assert_eq!(report["checks"][0]["status"], "not-run", "{report}");
    // The pull record keeps the configuration that decided.
    let pulls = lines(&rec.join("pulls.jsonl"));
    assert_eq!(pulls[0]["judge"], judgment["judge"], "{:?}", pulls[0]);
    // The comment names the configuration, never a check.
    let md = std::fs::read_to_string(out.join("lane.md")).unwrap();
    assert!(md.contains(&format!("configuration `{config}`")), "{md}");
    assert!(
        !md.contains("orders-money") && !md.contains("smoke"),
        "{md}"
    );
    for f in ["judgment.json", "provenance.json", "submission.json"] {
        let text = std::fs::read_to_string(out.join(f)).unwrap();
        assert!(!text.contains("orders-money"), "{f}: {text}");
    }

    // Without --judge nothing decided: no report, no judge on record.
    std::fs::remove_dir_all(&out).unwrap();
    assert!(pr(&suite, false).status.success());
    assert!(!out.join("held-out.json").exists());
    let pulls = lines(&rec.join("pulls.jsonl"));
    assert!(pulls[1].get("judge").is_none(), "{:?}", pulls[1]);

    // A suite inside the repository fails the run.
    std::fs::create_dir_all(r.join("held-out")).unwrap();
    std::fs::copy(
        suite.join("held-out.yaml"),
        r.join("held-out/held-out.yaml"),
    )
    .unwrap();
    let failed = pr(&r.join("held-out"), true);
    assert_eq!(failed.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("inside the repository"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
}

#[test]
fn a_human_lane_change_waits_for_a_persons_review() {
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
    let head = git(&r, &["rev-parse", "HEAD"]);
    let rec = t.join("records");
    let rs = rec.to_str().unwrap();
    let out = t.join("out");
    let change = "acme/shop#8";
    let pr = |reviews: Option<serde_json::Value>| -> serde_json::Value {
        let mut args = vec![
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
            "--records",
            rs,
            "--out",
            out.to_str().unwrap(),
        ];
        let file = t.join("reviews.json");
        if let Some(list) = reviews {
            std::fs::write(&file, list.to_string()).unwrap();
            args.extend(["--reviews", file.to_str().unwrap()]);
        }
        serde_json::from_str(&onus(&r, &args)).unwrap()
    };
    let md = || std::fs::read_to_string(out.join("lane.md")).unwrap();
    let last_pull = || lines(&rec.join("pulls.jsonl")).pop().unwrap();

    // No reviews yet: the owners of what needs a person are asked.
    let s = pr(None);
    assert_eq!(s["lane"], "human", "{s}");
    assert_eq!(s["review"]["required"], true, "{s}");
    assert_eq!(s["review"]["satisfied"], false, "{s}");
    assert_eq!(s["review"]["owners"], serde_json::json!(["@team-growth"]));
    assert_eq!(s["review"]["request"], serde_json::json!(["@team-growth"]));
    assert_eq!(
        last_pull()["requested"],
        serde_json::json!(["@team-growth"])
    );
    assert!(md().contains("waiting for an approving review"), "{}", md());
    assert!(md().contains("Asked: @team-growth."), "{}", md());

    // Pushed again: nobody is asked twice.
    let s = pr(None);
    assert_eq!(s["review"]["request"], serde_json::json!([]), "{s}");
    assert_eq!(
        last_pull()["requested"],
        serde_json::json!(["@team-growth"])
    );

    // The author, someone without write access and an approval of an
    // earlier commit do not count.
    let review = |by: &str, association: &str, commit: &str| serde_json::json!({ "by": by, "state": "APPROVED", "commit": commit, "association": association, "at": 1 });
    let s = pr(Some(serde_json::json!([
        review("mike", "OWNER", &head),
        review("eve", "CONTRIBUTOR", &head),
        review("grace", "MEMBER", &"0".repeat(40)),
    ])));
    assert_eq!(s["review"]["satisfied"], false, "{s}");
    assert_eq!(
        s["review"]["ignored"],
        serde_json::json!([
            { "by": "@eve", "why": "no write access (CONTRIBUTOR)" },
            { "by": "@grace", "why": "approved an earlier commit" },
            { "by": "@mike", "why": "opened it" },
        ])
    );
    assert!(
        md().contains("Not counted: @eve (no write access, CONTRIBUTOR)"),
        "{}",
        md()
    );

    // A member approves the head, but the change has owners, so only an
    // owner's approval counts.
    let s = pr(Some(serde_json::json!([review("grace", "MEMBER", &head)])));
    assert_eq!(s["review"]["satisfied"], false, "{s}");
    assert_eq!(s["review"]["approval"], "owner", "{s}");
    assert_eq!(
        s["review"]["ignored"],
        serde_json::json!([{ "by": "@grace", "why": "not an owner" }])
    );
    assert!(
        md().contains("from an owner of what changed") && md().contains("@grace (not an owner)"),
        "{}",
        md()
    );

    // The host says grace is in @team-growth: the lane is satisfied.
    let s = pr(Some(serde_json::json!({
        "reviews": [review("grace", "MEMBER", &head)],
        "teams": { "@team-growth": ["grace", "lin"] },
    })));
    assert_eq!(s["review"]["satisfied"], true, "{s}");
    assert_eq!(s["review"]["approvedBy"], serde_json::json!(["@grace"]));
    assert!(s["review"].get("note").is_none(), "{s}");
    assert!(md().contains("**Review:** approved by @grace."), "{}", md());
    assert_eq!(last_pull()["reviewedBy"], serde_json::json!(["@grace"]));
    assert_eq!(last_pull()["approval"], "owner");
    assert_eq!(
        last_pull()["requested"],
        serde_json::json!(["@team-growth"])
    );

    // Merged: the outcome says who approved it.
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
    assert_eq!(
        outcomes.last().unwrap()["reviewedBy"],
        serde_json::json!(["@grace"])
    );
    assert_eq!(outcomes.last().unwrap()["approval"], "owner");
}

#[test]
fn lanes_approval_at_the_base_commit_says_whose_approval_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    // A GitHub team owns what the change touches.
    let config = std::fs::read_to_string(r.join("onus.yaml"))
        .unwrap()
        .replace("@team-growth", "@acme/growth");
    std::fs::write(r.join("onus.yaml"), &config).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    git(&r, &["checkout", "-q", "-b", "codex/sms"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), &r);
    // The change tries to loosen the rule for itself.
    std::fs::write(
        r.join("onus.yaml"),
        format!("{config}\nlanes:\n  approval: member\n"),
    )
    .unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "Text customers"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let member = t.join("member.yaml");
    std::fs::write(&member, format!("{config}\nlanes:\n  approval: member\n")).unwrap();
    let rec = t.join("records");
    let out = t.join("out");
    let file = t.join("reviews.json");
    let pr = |reviews: serde_json::Value, config: Option<&Path>| -> serde_json::Value {
        std::fs::write(&file, reviews.to_string()).unwrap();
        let mut args = vec![
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/sms",
            "--change",
            "acme/shop#9",
            "--author",
            "mike",
            "--records",
            rec.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--reviews",
            file.to_str().unwrap(),
        ];
        if let Some(c) = config {
            args.extend(["--config", c.to_str().unwrap()]);
        }
        let s: serde_json::Value = serde_json::from_str(&onus(&r, &args)).unwrap();
        assert_eq!(s["lane"], "human", "{s}");
        s["review"].clone()
    };
    let md = || std::fs::read_to_string(out.join("lane.md")).unwrap();
    let grace = serde_json::json!({ "by": "grace", "state": "APPROVED", "commit": head, "association": "MEMBER", "at": 1 });

    // The base commit asks for an owner (by default), and the host says
    // who is in the team: grace is not.
    let s = pr(
        serde_json::json!({ "reviews": [grace], "teams": { "@acme/growth": ["ada"] } }),
        None,
    );
    assert_eq!(s["owners"], serde_json::json!(["@acme/growth"]), "{s}");
    assert_eq!(s["approval"], "owner", "{s}");
    assert_eq!(s["satisfied"], false, "{s}");
    assert_eq!(
        s["ignored"],
        serde_json::json!([{ "by": "@grace", "why": "not an owner" }])
    );

    // Without the team's members, anyone with write access counts, and the
    // answer says why.
    let s = pr(serde_json::json!([grace]), None);
    assert_eq!(s["satisfied"], true, "{s}");
    assert_eq!(s["approval"], "member", "{s}");
    let note = "The members of @acme/growth are not known, so an approval from any person with write access counts";
    assert_eq!(s["note"], note, "{s}");
    assert!(md().contains(note), "{}", md());
    let pulls = lines(&rec.join("pulls.jsonl"));
    assert_eq!(pulls.last().unwrap()["approval"], "member");

    // lanes.approval: member, given explicitly: anyone with write access.
    let s = pr(
        serde_json::json!({ "reviews": [grace], "teams": { "@acme/growth": ["ada"] } }),
        Some(&member),
    );
    assert_eq!(s["satisfied"], true, "{s}");
    assert_eq!(s["approval"], "member", "{s}");
    assert!(s.get("note").is_none(), "{s}");
}

#[test]
fn a_change_cannot_name_who_approves_it() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    let config = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    std::fs::create_dir_all(r.join(".github")).unwrap();
    std::fs::write(r.join(".github/CODEOWNERS"), "* @grace\n").unwrap();
    std::fs::create_dir_all(r.join("ci")).unwrap();
    std::fs::write(r.join("ci/onus.yaml"), &config).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    git(&r, &["checkout", "-q", "-b", "codex/sms"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), &r);
    // The change makes ada an owner, and loosens the config it is run with.
    std::fs::write(r.join(".github/CODEOWNERS"), "* @ada\n").unwrap();
    std::fs::write(
        r.join("ci/onus.yaml"),
        format!("{config}\nlanes:\n  approval: member\n"),
    )
    .unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "Text customers"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let rec = t.join("records");
    let out = t.join("out");
    let file = t.join("reviews.json");
    let in_repo = r.join("ci/onus.yaml");
    let pr = |by: &str, config: Option<&Path>| -> serde_json::Value {
        let review = serde_json::json!([
            { "by": by, "state": "APPROVED", "commit": head, "association": "MEMBER", "at": 1 }
        ]);
        std::fs::write(&file, review.to_string()).unwrap();
        let mut args = vec![
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/sms",
            "--change",
            "acme/shop#10",
            "--author",
            "mike",
            "--records",
            rec.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--reviews",
            file.to_str().unwrap(),
        ];
        if let Some(c) = config {
            args.extend(["--config", c.to_str().unwrap()]);
        }
        let s: serde_json::Value = serde_json::from_str(&onus(&r, &args)).unwrap();
        assert_eq!(s["lane"], "human", "{s}");
        s["review"].clone()
    };

    // CODEOWNERS is read at the base commit: grace owns it, ada does not.
    let s = pr("ada", None);
    assert_eq!(
        s["owners"],
        serde_json::json!(["@grace", "@team-growth"]),
        "{s}"
    );
    assert_eq!(s["approval"], "owner", "{s}");
    assert_eq!(s["satisfied"], false, "{s}");
    assert_eq!(
        s["ignored"],
        serde_json::json!([{ "by": "@ada", "why": "not an owner" }])
    );
    let s = pr("grace", None);
    assert_eq!(s["satisfied"], true, "{s}");
    assert_eq!(s["approvedBy"], serde_json::json!(["@grace"]));

    // A config inside the repository is read at the base commit too, so
    // the head's `approval: member` does not count.
    let s = pr("ada", Some(&in_repo));
    assert_eq!(s["approval"], "owner", "{s}");
    assert_eq!(s["satisfied"], false, "{s}");
}

#[test]
fn the_owners_who_may_approve_come_from_the_base_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    // onus.yaml names no owners for what the change touches, so the map
    // takes them from CODEOWNERS, in whichever tree it maps.
    let declared = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    let unowned: String = declared
        .lines()
        .map(|l| match l.split_once("owners: [\"@team-growth\"]") {
            Some((a, b)) => format!("{}{b}\n", a.trim_end().trim_end_matches(',')),
            None => format!("{l}\n"),
        })
        .collect();
    std::fs::write(r.join("onus.yaml"), &unowned).unwrap();
    std::fs::create_dir_all(r.join(".github")).unwrap();
    std::fs::write(r.join(".github/CODEOWNERS"), "* @grace\n").unwrap();
    // A config kept in the repository and given with --config.
    std::fs::create_dir_all(r.join("ci")).unwrap();
    std::fs::write(r.join("ci/onus.yaml"), &declared).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    git(&r, &["checkout", "-q", "-b", "codex/sms"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), &r);
    // The change makes ada the owner, in CODEOWNERS and in the config.
    std::fs::write(r.join(".github/CODEOWNERS"), "* @ada\n").unwrap();
    std::fs::write(
        r.join("ci/onus.yaml"),
        declared.replace("@team-growth", "@ada"),
    )
    .unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "Text customers"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let in_repo = r.join("ci/onus.yaml");
    let rec = t.join("records");
    let out = t.join("out");
    let file = t.join("reviews.json");
    let review = serde_json::json!([
        { "by": "ada", "state": "APPROVED", "commit": head, "association": "MEMBER", "at": 1 }
    ]);
    std::fs::write(&file, review.to_string()).unwrap();
    // As the action runs it: the report first, then ci owners and ci pr.
    let run = |config: Option<&Path>| -> (serde_json::Value, serde_json::Value) {
        let mut report = vec![
            "report",
            "--base",
            "main",
            "--head",
            "codex/sms",
            "--format",
            "json",
        ];
        let mut extra = vec![];
        if let Some(c) = config {
            extra = vec!["--config", c.to_str().unwrap()];
        }
        report.extend(&extra);
        let report_file = t.join("report.json");
        std::fs::write(&report_file, onus(&r, &report)).unwrap();
        let mut owners = vec![
            "ci",
            "owners",
            "--base",
            "main",
            "--report",
            report_file.to_str().unwrap(),
        ];
        owners.extend(&extra);
        let owners: serde_json::Value = serde_json::from_str(&onus(&r, &owners)).unwrap();
        let mut args = vec![
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            "codex/sms",
            "--change",
            "acme/shop#11",
            "--author",
            "mike",
            "--report",
            report_file.to_str().unwrap(),
            "--records",
            rec.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--reviews",
            file.to_str().unwrap(),
        ];
        args.extend(&extra);
        let s: serde_json::Value = serde_json::from_str(&onus(&r, &args)).unwrap();
        assert_eq!(s["lane"], "human", "{s}");
        (owners, s["review"].clone())
    };

    // The head's map names ada from its own CODEOWNERS; the review reads
    // CODEOWNERS at the base, where grace owns it.
    let names_ada = || {
        std::fs::read_to_string(t.join("report.json"))
            .unwrap()
            .contains("\"@ada\"")
    };
    let (owners, s) = run(None);
    assert!(names_ada(), "the head's map names ada");
    assert_eq!(owners, serde_json::json!({ "owners": ["@grace"] }));
    assert_eq!(s["owners"], serde_json::json!(["@grace"]), "{s}");
    assert_eq!(s["approval"], "owner", "{s}");
    assert_eq!(s["satisfied"], false, "{s}");
    assert_eq!(
        s["ignored"],
        serde_json::json!([{ "by": "@ada", "why": "not an owner" }])
    );

    // The head's copy of an in-repository config names ada; the review
    // reads the base copy, where @team-growth owns it.
    let (owners, s) = run(Some(&in_repo));
    assert!(names_ada(), "the head's map names ada");
    assert_eq!(owners["owners"], s["owners"], "{s}");
    assert!(
        s["owners"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("@team-growth")),
        "{s}"
    );
    assert!(
        !s["owners"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("@ada")),
        "{s}"
    );
    assert_eq!(s["satisfied"], false, "{s}");
    assert_eq!(
        s["ignored"],
        serde_json::json!([{ "by": "@ada", "why": "not an owner" }])
    );
}

#[test]
fn an_automatic_merge_is_sampled_and_audited() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    let base = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    let policy = |share: u32| {
        format!(
            "{base}\nlanes:\n  minRecord: 0\n  auditRate: 0\n  auditAfterMerge: {share}\n  rules:\n    - {{ lane: auto-merge, match: every, kinds: [internal], components: [root] }}\n"
        )
    };
    std::fs::write(r.join("onus.yaml"), policy(1)).unwrap();
    let never = t.join("never.yaml");
    std::fs::write(&never, policy(0)).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    let rec = t.join("records");
    let rs = rec.to_str().unwrap();
    let out = t.join("out");
    let file = rec.join("outcomes.jsonl");
    let fs = file.to_str().unwrap();
    // A docs-only change, opened, classified and merged.
    let merge = |change: &str, branch: &str, text: &str, config: Option<&Path>| -> String {
        git(&r, &["checkout", "-q", "-b", branch, "main"]);
        std::fs::create_dir_all(r.join("docs")).unwrap();
        std::fs::write(r.join("docs/ON_CALL.md"), text).unwrap();
        git(&r, &["add", "-A"]);
        git(&r, &["commit", "-q", "-m", "On-call notes"]);
        let mut args = vec![
            "ci",
            "pr",
            "--base",
            "main",
            "--head",
            branch,
            "--change",
            change,
            "--branch",
            branch,
            "--author",
            "mike",
            "--records",
            rs,
            "--out",
        ];
        args.push(out.to_str().unwrap());
        if let Some(c) = config {
            args.extend(["--config", c.to_str().unwrap()]);
        }
        let s: serde_json::Value = serde_json::from_str(&onus(&r, &args)).unwrap();
        assert_eq!(s["lane"], "auto-merge", "{s}");
        git(&r, &["checkout", "-q", "main"]);
        git(&r, &["merge", "-q", "--squash", branch]);
        git(
            &r,
            &["commit", "-q", "-m", &format!("On-call notes ({change})")],
        );
        let commit = git(&r, &["rev-parse", "HEAD"]);
        onus(
            &r,
            &[
                "ci",
                "closed",
                "--change",
                change,
                "--merged",
                "--commit",
                &commit,
                "--records",
                rs,
            ],
        )
    };
    let json =
        |args: &[&str]| -> serde_json::Value { serde_json::from_str(&onus(&r, args)).unwrap() };
    let audits = || json(&["outcomes", "audits", "--file", fs]);
    let summary = || json(&["outcomes", "summary", "--file", fs]);

    // Nothing before the merge says whether it will be audited.
    let printed = merge("acme/shop#12", "codex/on-call", "# On call\n", None);
    let pull = lines(&rec.join("pulls.jsonl")).pop().unwrap();
    assert_eq!(pull["auditAfterMerge"], 1.0, "{pull}");
    let md = std::fs::read_to_string(out.join("lane.md")).unwrap();
    assert!(!md.contains("sampl") && !md.contains("after merg"), "{md}");
    assert_eq!(
        printed,
        "acme/shop#12: merged in the auto-merge lane, by codex//"
    );
    let outcome = lines(&file).pop().unwrap();
    assert_eq!(outcome["sampled"], true, "{outcome}");
    assert!(outcome.get("audit").is_none(), "{outcome}");

    // It waits for a person.
    let q = audits();
    assert_eq!(q["awaiting"].as_array().unwrap().len(), 1, "{q}");
    assert_eq!(q["awaiting"][0]["change"], "acme/shop#12");
    assert_eq!(q["audited"], serde_json::json!([]));
    assert_eq!(summary()["awaitingAudit"], 1);

    // A person audits it.
    let reply = json(&[
        "outcomes",
        "audit",
        "--file",
        fs,
        "--change",
        "acme/shop#12",
        "--by",
        "@alice",
        "--result",
        "miss",
        "--note",
        "names a person who left",
    ]);
    assert_eq!(
        reply,
        serde_json::json!({
            "change": "acme/shop#12",
            "missed": true,
            "sampled": true,
            "reply": "Audit recorded as a miss for @alice.",
        })
    );
    let q = audits();
    assert_eq!(q["awaiting"], serde_json::json!([]), "{q}");
    let audit = &q["audited"][0]["audit"];
    assert_eq!(audit["by"], "@alice", "{q}");
    assert_eq!(audit["missed"], true);
    assert_eq!(audit["note"], "names a person who left");
    assert_eq!(
        q["audited"][0]["note"],
        "audited by @alice: names a person who left"
    );
    let s = summary();
    assert_eq!(s["sampledMissRate"], 1.0, "{s}");
    assert_eq!(s["total"]["sampled"], 1, "{s}");
    assert_eq!(s["awaitingAudit"], 0, "{s}");

    // With no share after merge, nothing is sampled.
    merge(
        "acme/shop#13",
        "codex/on-call-2",
        "# On call\n\nPage twice.\n",
        Some(&never),
    );
    let outcome = lines(&file).pop().unwrap();
    assert_eq!(outcome["change"], "acme/shop#13");
    assert!(outcome.get("sampled").is_none(), "{outcome}");
    assert_eq!(
        lines(&rec.join("pulls.jsonl")).pop().unwrap()["auditAfterMerge"],
        0.0
    );

    // `/onus audit` writes the same record and clears the queue.
    merge(
        "acme/shop#14",
        "codex/on-call-3",
        "# On call\n\nPage once.\n",
        None,
    );
    assert_eq!(audits()["awaiting"][0]["change"], "acme/shop#14");
    let body = t.join("comment.md");
    std::fs::write(&body, "/onus audit ok").unwrap();
    let said = json(&[
        "ci",
        "comment",
        "--change",
        "acme/shop#14",
        "--body-file",
        body.to_str().unwrap(),
        "--author",
        "lead",
        "--association",
        "MEMBER",
        "--records",
        rs,
    ]);
    assert_eq!(said["reply"], "Audit recorded for @lead.", "{said}");
    let q = audits();
    assert_eq!(q["awaiting"], serde_json::json!([]), "{q}");
    assert_eq!(q["audited"][0]["change"], "acme/shop#14", "{q}");
    assert_eq!(q["audited"][0]["audit"]["by"], "@lead");
    assert_eq!(q["audited"][0]["audit"]["missed"], false);

    // A change with no outcome cannot be audited.
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .current_dir(&r)
        .args([
            "outcomes",
            "audit",
            "--file",
            fs,
            "--change",
            "acme/shop#99",
            "--by",
            "@alice",
            "--result",
            "ok",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has no outcome yet"), "{err}");
}

#[test]
fn comment_feedback_says_what_was_said() {
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
    let rs = rec.to_str().unwrap();
    let change = "acme/shop#7";
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
        ],
    );
    let report_file = t.join("report.json");
    std::fs::write(
        &report_file,
        onus(
            &r,
            &[
                "report",
                "--base",
                "main",
                "--head",
                "codex/sms",
                "--format",
                "json",
            ],
        ),
    )
    .unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report_file).unwrap()).unwrap();
    let first = report["changes"][0]["id"].as_str().unwrap().to_string();
    let rows = report["changes"].as_array().unwrap().len();
    let snapshot = || {
        ["pulls.jsonl", "outcomes.jsonl"].map(|f| std::fs::read(rec.join(f)).unwrap_or_default())
    };
    let before = snapshot();

    let say = |text: &str, association: &str, with_report: bool| -> serde_json::Value {
        let f = t.join("comment.md");
        std::fs::write(&f, text).unwrap();
        let mut args = vec![
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
        ];
        if with_report {
            args.extend(["--report", report_file.to_str().unwrap()]);
        }
        serde_json::from_str(&onus(&r, &args)).unwrap()
    };

    // Anyone may say the report caught something; the note and who said it come back.
    let caught = say("/onus caught the new SMS vendor", "NONE", false);
    assert_eq!(caught["ok"], true, "{caught}");
    assert_eq!(caught["note"], "the new SMS vendor");
    assert_eq!(caught["by"], "@lead");
    assert_eq!(
        say("/onus caught", "NONE", false)["note"],
        serde_json::Value::Null
    );

    // An accuracy check needs write access.
    let ok = say("/onus accuracy ok", "MEMBER", true);
    assert_eq!(ok["ok"], true, "{ok}");
    assert_eq!(ok["verdict"], "ok");
    assert_eq!(ok["row"], serde_json::Value::Null);
    assert_eq!(ok["by"], "@lead");
    let refused = say("/onus accuracy ok", "CONTRIBUTOR", true);
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["command"], "accuracy");
    assert!(
        refused["reply"].as_str().unwrap().contains("write access"),
        "{refused}"
    );

    // A row number is the row's id in the table; a row id is kept.
    let wrong = say("/onus accuracy wrong 1 not new", "MEMBER", true);
    assert_eq!(wrong["verdict"], "wrong", "{wrong}");
    assert_eq!(wrong["row"], first.as_str());
    assert_eq!(wrong["note"], "not new");
    assert_eq!(
        say("/onus accuracy wrong #1", "MEMBER", true)["row"],
        first.as_str()
    );
    let unknown = say("/onus accuracy wrong 99 x", "MEMBER", true);
    assert_eq!(unknown["ok"], false);
    let text = unknown["reply"].as_str().unwrap();
    assert!(text.contains("has no row `99`"), "{text}");
    assert!(text.contains(&format!("it has {rows}")), "{text}");
    let by_id = say(&format!("/onus accuracy wrong {first} x"), "MEMBER", true);
    assert_eq!(by_id["row"], first.as_str(), "{by_id}");
    assert_eq!(by_id["note"], "x");
    // Without the report, the row is kept as written.
    assert_eq!(say("/onus accuracy wrong 7 x", "MEMBER", false)["row"], "7");
    assert_eq!(say("/onus accuracy wrong", "MEMBER", true)["ok"], false);

    // A miss needs to say what was left out.
    assert_eq!(say("/onus accuracy missed", "MEMBER", true)["ok"], false);
    let missed = say("/onus accuracy missed the cron job", "MEMBER", true);
    assert_eq!(missed["verdict"], "missed", "{missed}");
    assert_eq!(missed["note"], "the cron job");
    assert_eq!(missed["row"], serde_json::Value::Null);
    assert_eq!(say("/onus accuracy maybe", "MEMBER", true)["ok"], false);

    // None of it is a record.
    assert_eq!(snapshot(), before);
}
