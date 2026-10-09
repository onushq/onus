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
    assert_eq!(
        say("just a comment", "MEMBER")["command"],
        serde_json::Value::Null
    );

    // The next run shows the approval and no longer offers that row.
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
            out.to_str().unwrap(),
        ],
    );
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
    assert_eq!(lines(&rec2.join("pulls.jsonl")).len(), 3);

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
