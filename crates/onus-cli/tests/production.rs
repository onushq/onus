//! Production is the final judge (ADR 0016): a change gets a rollout plan
//! for its lane, a deploy tool reports stages and Onus decides, a rollback
//! is recorded against the change, its agent setup and its components, and
//! the next change there gets a stricter plan. Signals from monitoring are
//! tied to changes; production traces fold into the map.

mod common;

use std::path::Path;
use std::process::Command;

use common::{copy_dir, fixture, repo_root};

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

/// Runs onus; returns its exit code, stdout and stderr.
fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    )
}

fn onus(dir: &Path, args: &[&str]) -> String {
    let (code, stdout, stderr) = run(dir, args);
    assert_eq!(code, 0, "onus {args:?}: {stderr}");
    stdout
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The shop at `main`, with billing's `payments` label left out so a change
/// there lands in the judge lane rather than with a person.
fn shop(t: &Path) -> std::path::PathBuf {
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    let config = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    let config = config.replace(
        r#"owners: ["@team-payments"], labels: [payments] }"#,
        r#"owners: ["@team-payments"] }"#,
    );
    assert!(!config.contains("labels: [payments]"));
    std::fs::write(r.join("onus.yaml"), config).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    r
}

/// A change to billing on `branch`, merged into main; returns its commit.
fn billing_change(r: &Path, branch: &str, file: &str) -> String {
    git(r, &["checkout", "-q", "-b", branch, "main"]);
    std::fs::write(
        r.join("services/billing/src").join(file),
        "export function roundCents(n: number): number {\n  return Math.round(n * 100) / 100;\n}\n",
    )
    .unwrap();
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "Round cents"]);
    git(r, &["rev-parse", "HEAD"])
}

fn pr(r: &Path, branch: &str, change: &str, rec: &Path, out: &Path) -> serde_json::Value {
    json(&onus(
        r,
        &[
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
            "--records",
            rec.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
    ))
}

fn merge(r: &Path, branch: &str, change: &str, rec: &Path) -> String {
    git(r, &["checkout", "-q", "main"]);
    git(r, &["merge", "-q", "--ff-only", branch]);
    let sha = git(r, &["rev-parse", "HEAD"]);
    onus(
        r,
        &[
            "ci",
            "closed",
            "--change",
            change,
            "--merged",
            "--commit",
            &sha,
            "--records",
            rec.to_str().unwrap(),
        ],
    );
    sha
}

#[test]
fn a_change_meets_production_and_is_rolled_back() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = shop(t);
    let (rec, out) = (t.join("records"), t.join("out"));
    let rs = rec.to_str().unwrap();

    // Open: the judge lane's plan, in the comment and on the record.
    let sha = billing_change(&r, "codex/round", "round.ts");
    let summary = pr(&r, "codex/round", "acme/shop#1", &rec, &out);
    assert_eq!(summary["lane"], "judge", "{summary}");
    assert_eq!(summary["rollout"]["strategy"], "canary");
    assert_eq!(summary["record"]["recentRollbacks"], 0);
    let plan = json(&std::fs::read_to_string(out.join("rollout.json")).unwrap());
    assert_eq!(plan["strategy"], "canary", "{plan}");
    assert_eq!(plan["steps"], serde_json::json!([5, 25, 100]));
    assert_eq!(plan["bakeMinutes"], 30);
    assert_eq!(plan["automaticRollback"], true);
    let md = std::fs::read_to_string(out.join("lane.md")).unwrap();
    assert!(
        md.contains("**Rollout:** `canary` 5% → 25% → 100%, 30 min at each step; rolls back automatically when error_rate > 1.5× baseline or latency_p95_ms > 1.3× baseline."),
        "{md}"
    );
    let pulls = lines(&rec.join("pulls.jsonl"));
    assert!(
        pulls[0]["components"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("billing")),
        "{:?}",
        pulls[0]
    );
    assert_eq!(pulls[0]["rollout"], plan);
    let classification = json(&std::fs::read_to_string(out.join("classification.json")).unwrap());
    assert_eq!(classification["rollout"], plan);
    // Its provenance while open: pending, with the plan.
    let open = json(&std::fs::read_to_string(out.join("provenance.json")).unwrap());
    assert_eq!(open["rollout"]["status"], "pending");
    assert_eq!(open["rollout"]["plan"]["strategy"], "canary");

    assert_eq!(merge(&r, "codex/round", "acme/shop#1", &rec), sha);

    // The plan, asked for by the change: nothing is recorded.
    let asked = json(&onus(
        &r,
        &["ci", "rollout", "--change", "acme/shop#1", "--records", rs],
    ));
    assert_eq!(asked, plan);
    assert!(!rec.join("signals.jsonl").exists());

    // The deploy tool reports stages; Onus decides.
    let metrics = |name: &str, error_rate: f64| {
        let f = t.join(name);
        std::fs::write(
            &f,
            serde_json::json!({ "metrics": [
                { "name": "error_rate", "value": error_rate, "baseline": 0.01 },
                { "name": "latency_p95_ms", "value": 200, "baseline": 190 },
            ]})
            .to_string(),
        )
        .unwrap();
        f
    };
    let (ok, bad) = (metrics("ok.json", 0.011), metrics("bad.json", 0.03));
    let stage = |percent: &str, m: &Path, at: u64| {
        let (code, stdout, stderr) = run(
            &r,
            &[
                "ci",
                "rollout",
                "--commit",
                &sha[..7],
                "--percent",
                percent,
                "--metrics",
                m.to_str().unwrap(),
                "--records",
                rs,
                "--at",
                &at.to_string(),
            ],
        );
        assert!(stderr.is_empty(), "{stderr}");
        (code, json(&stdout))
    };
    let start = now() - 7200;
    let (code, d) = stage("5", &ok, start);
    assert_eq!(
        (code, &d["decision"]),
        (3, &serde_json::json!("hold")),
        "{d}"
    );
    assert_eq!(d["reasons"], serde_json::json!(["baked 0 of 30 min"]));
    let (code, d) = stage("5", &ok, start + 1800);
    assert_eq!(
        (code, &d["decision"]),
        (0, &serde_json::json!("promote")),
        "{d}"
    );
    assert_eq!(
        (&d["next"], &d["change"]),
        (&serde_json::json!(25), &serde_json::json!("acme/shop#1"))
    );
    let (code, d) = stage("25", &bad, start + 1800);
    assert_eq!(
        (code, &d["decision"]),
        (2, &serde_json::json!("rollback")),
        "{d}"
    );
    assert_eq!(
        d["reasons"],
        serde_json::json!(["error_rate 0.03 is 3× its baseline 0.01 (limit 1.5×)"])
    );
    // Each stage is a signal; the rollback is an outcome, once.
    let signals = lines(&rec.join("signals.jsonl"));
    assert_eq!(signals.len(), 3);
    assert!(
        signals
            .iter()
            .all(|s| s["kind"] == "rollout" && s["change"] == "acme/shop#1")
    );
    assert_eq!(signals[2]["rollout"]["decision"], "rollback");
    assert_eq!(signals[2]["attributedBy"], "commit");
    // Reported again: the same decision, recorded once.
    let (code, _) = stage("25", &bad, start + 1800);
    assert_eq!(code, 2);
    assert_eq!(lines(&rec.join("signals.jsonl")).len(), 3);
    let outcomes = lines(&rec.join("outcomes.jsonl"));
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    let last = outcomes.last().unwrap();
    assert_eq!(last["result"], "rolled-back");
    assert!(
        last["signal"]
            .as_str()
            .unwrap()
            .starts_with("rollout:acme/shop#1:25:"),
        "{last}"
    );
    assert_eq!(last["involved"], serde_json::json!(["billing"]));
    assert!(last.get("planned").is_none());
    let summary = json(&onus(
        &r,
        &[
            "outcomes",
            "summary",
            "--file",
            rec.join("outcomes.jsonl").to_str().unwrap(),
        ],
    ));
    assert_eq!(summary["total"]["rolledBack"], 1, "{summary}");
    assert_eq!(summary["total"]["reverted"], 1);
    // Already rolled back.
    assert_eq!(
        onus(
            &r,
            &[
                "ci",
                "rollback",
                "--change",
                "acme/shop#1",
                "--records",
                rs,
                "--reason",
                "again"
            ]
        ),
        "acme/shop#1: already rolled back"
    );

    // The provenance says so, with every stage.
    let p = json(&onus(
        &r,
        &["provenance", "--change", "acme/shop#1", "--records", rs],
    ));
    assert_eq!(p["rollout"]["status"], "rolled-back", "{p}");
    assert_eq!(p["rollout"]["percent"], 25);
    let stages: Vec<(u64, &str)> = p["rollout"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["percent"].as_u64().unwrap(),
                s["decision"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(stages, [(5, "hold"), (5, "promote"), (25, "rollback")]);
    assert_eq!(
        p["rollout"]["plan"]["steps"],
        serde_json::json!([5, 25, 100])
    );

    // The next change to billing from the same agent setup: the history
    // input raises it, and it gets the human lane's plan.
    billing_change(&r, "codex/round-again", "round-again.ts");
    let summary = pr(&r, "codex/round-again", "acme/shop#2", &rec, &out);
    assert_eq!(summary["record"]["recentRollbacks"], 1, "{summary}");
    let c = json(&std::fs::read_to_string(out.join("classification.json")).unwrap());
    assert!(
        c["applied"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["source"] == "input"
                && a["reason"]
                    .as_str()
                    .unwrap()
                    .contains("`billing` had 1 revert")),
        "{c}"
    );
    let plan = json(&std::fs::read_to_string(out.join("rollout.json")).unwrap());
    assert_eq!(plan["strategy"], "flag", "{plan}");
    assert_eq!(plan["steps"], serde_json::json!([1, 5, 25, 100]));
    assert_eq!(plan["flag"], "onus-acme-shop-2");
    assert!(
        plan["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().contains("billing")),
        "{plan}"
    );
    let md = std::fs::read_to_string(out.join("lane.md")).unwrap();
    assert!(md.contains("· 1 recent rollbacks"), "{md}");
    assert!(
        md.contains("**Rollout:** `flag` behind `onus-acme-shop-2` 1% → 5% → 25% → 100%"),
        "{md}"
    );
    assert!(md.contains("- `billing` had 1 revert or rollback in the last 90 days: the human lane's plan instead"), "{md}");

    // A planned rollback counts against no one.
    let sha2 = merge(&r, "codex/round-again", "acme/shop#2", &rec);
    assert_eq!(
        onus(
            &r,
            &[
                "ci",
                "rollback",
                "--commit",
                &sha2,
                "--records",
                rs,
                "--reason",
                "flag off as planned",
                "--planned",
            ]
        ),
        "acme/shop#2: rolled back"
    );
    let last = lines(&rec.join("outcomes.jsonl")).pop().unwrap();
    assert_eq!(
        (&last["result"], &last["planned"]),
        (&serde_json::json!("rolled-back"), &serde_json::json!(true))
    );
    let areas = json(&onus(&r, &["signals", "areas", "--records", rs]));
    assert_eq!(areas["areas"]["billing"]["reverts"], 1, "{areas}");
    assert_eq!(
        areas["areas"]["billing"]["changes"],
        serde_json::json!(["acme/shop#1"])
    );
    // Scoped to one repository, as a classification of its changes reads it.
    let shop = json(&onus(
        &r,
        &[
            "signals",
            "areas",
            "--records",
            rs,
            "--repository",
            "acme/shop",
        ],
    ));
    assert_eq!(shop["areas"]["billing"]["reverts"], 1, "{shop}");
    let other = json(&onus(
        &r,
        &[
            "signals",
            "areas",
            "--records",
            rs,
            "--repository",
            "acme/other",
        ],
    ));
    assert_eq!(
        other["areas"]["billing"]["reverts"].as_u64().unwrap_or(0),
        0,
        "{other}"
    );
    // A rollback that was not planned, after the planned one, counts.
    assert_eq!(
        onus(
            &r,
            &[
                "ci",
                "rollback",
                "--change",
                "acme/shop#2",
                "--records",
                rs,
                "--reason",
                "checkout 500s",
            ]
        ),
        "acme/shop#2: rolled back"
    );
    let last = lines(&rec.join("outcomes.jsonl")).pop().unwrap();
    assert_eq!(last["result"], "rolled-back");
    assert!(last.get("planned").is_none(), "{last}");
    let areas = json(&onus(&r, &["signals", "areas", "--records", rs]));
    assert_eq!(areas["areas"]["billing"]["reverts"], 2, "{areas}");

    // A change recorded before rollout plans: the plan its lane gets now,
    // from the repository's policy, tightened by billing's history.
    let mut old = lines(&rec.join("pulls.jsonl"))[0].clone();
    let o = old.as_object_mut().unwrap();
    o.remove("rollout");
    o.insert("change".into(), "acme/shop#3".into());
    o.insert("lane".into(), "auto-merge".into());
    o.insert("head".into(), "0123456789abcdef".into());
    let mut text = std::fs::read_to_string(rec.join("pulls.jsonl")).unwrap();
    text.push_str(&format!("{old}\n"));
    std::fs::write(rec.join("pulls.jsonl"), text).unwrap();
    let policy = t.join("policy.yaml");
    std::fs::write(
        &policy,
        "version: 1\nlanes:\n  rollout:\n    judge:\n      strategy: canary\n      steps: [50, 100]\n      bakeMinutes: 5\n",
    )
    .unwrap();
    let plan = json(&onus(
        &r,
        &[
            "ci",
            "rollout",
            "--change",
            "acme/shop#3",
            "--records",
            rs,
            "--config",
            policy.to_str().unwrap(),
        ],
    ));
    assert_eq!(
        (&plan["lane"], &plan["steps"], &plan["bakeMinutes"]),
        (
            &serde_json::json!("auto-merge"),
            &serde_json::json!([50, 100]),
            &serde_json::json!(5)
        ),
        "{plan}"
    );
    assert!(
        plan["reasons"][1]
            .as_str()
            .unwrap()
            .contains("the judge lane's plan instead"),
        "{plan}"
    );

    // A change Onus has no record of has no rollout.
    let (code, _, stderr) = run(
        &r,
        &[
            "ci",
            "rollout",
            "--change",
            "acme/shop#9",
            "--percent",
            "5",
            "--records",
            rs,
        ],
    );
    assert_eq!(code, 1);
    assert!(stderr.contains("acme/shop#9 is not on record"), "{stderr}");
}

#[test]
fn signals_ingest_attributes_and_dedupes() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let r = shop(t);
    let (rec, out) = (t.join("records"), t.join("out"));
    let rs = rec.to_str().unwrap();
    billing_change(&r, "codex/round", "round.ts");
    pr(&r, "codex/round", "acme/shop#1", &rec, &out);
    let sha = merge(&r, "codex/round", "acme/shop#1", &rec);

    let file = t.join("export.json");
    let incident = serde_json::json!({ "id": "pagerduty:Q1ABC", "kind": "incident", "source": "pagerduty",
        "severity": "high", "commit": &sha[..7], "components": ["billing"], "note": "checkout 500s" });
    std::fs::write(
        &file,
        serde_json::json!([
            incident,
            { "id": "datadog:conv-1", "kind": "metric", "components": ["billing"],
              "metrics": [{ "name": "checkout_conversion", "value": 0.02, "baseline": 0.03 }] },
            incident,
        ])
        .to_string(),
    )
    .unwrap();
    let ingest = || {
        json(&onus(
            &r,
            &[
                "signals",
                "ingest",
                "--records",
                rs,
                "--file",
                file.to_str().unwrap(),
            ],
        ))
    };
    let s = ingest();
    assert_eq!(
        (&s["recorded"], &s["duplicates"]),
        (&serde_json::json!(2), &serde_json::json!(1)),
        "{s}"
    );
    assert_eq!(
        s["attributed"],
        serde_json::json!([{ "id": "pagerduty:Q1ABC", "change": "acme/shop#1", "by": "commit" }])
    );
    assert_eq!(
        s["outcomes"],
        serde_json::json!([{ "change": "acme/shop#1", "result": "incident" }])
    );
    let last = lines(&rec.join("outcomes.jsonl")).pop().unwrap();
    assert_eq!(
        (&last["result"], &last["signal"]),
        (
            &serde_json::json!("incident"),
            &serde_json::json!("pagerduty:Q1ABC")
        )
    );
    assert_eq!(last["note"], "pagerduty pagerduty:Q1ABC: checkout 500s");
    let areas = json(&onus(
        &r,
        &["signals", "areas", "--records", rs, "--days", "30"],
    ));
    let billing = &areas["areas"]["billing"];
    assert_eq!(
        (&billing["incidents"], &billing["errorSpikes"]),
        (&serde_json::json!(1), &serde_json::json!(1)),
        "{areas}"
    );
    assert_eq!(areas["days"], 30);
    // Again: nothing new.
    let s = ingest();
    assert_eq!(
        (&s["recorded"], &s["duplicates"]),
        (&serde_json::json!(0), &serde_json::json!(3))
    );
    assert_eq!(lines(&rec.join("signals.jsonl")).len(), 2);
    // The incident is the agent setup's.
    let summary = json(&onus(
        &r,
        &[
            "outcomes",
            "summary",
            "--file",
            rec.join("outcomes.jsonl").to_str().unwrap(),
        ],
    ));
    assert_eq!(summary["byAgent"]["codex//"]["incidents"], 1, "{summary}");

    // From stdin, as JSON Lines; a bad line fails the whole input, naming it.
    let ingest_text = |text: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_onus"))
            .current_dir(&r)
            .args(["signals", "ingest", "--records", rs, "--file", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        (
            o.status.code(),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        )
    };
    let (code, _, stderr) = ingest_text(
        "{\"id\":\"sentry:1\",\"kind\":\"error-spike\",\"components\":[\"orders\"]}\n{\"id\":\"x\",\"kind\":\"incident\",\"commit\":\"xyz\"}\n",
    );
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("line 2 of stdin") && stderr.contains("is not a commit"),
        "{stderr}"
    );
    let (code, _, stderr) = ingest_text(
        r#"{"id":"r","kind":"rollout","change":"acme/shop#1","rollout":{"percent":5,"decision":"complete"}}"#,
    );
    assert_eq!(code, Some(1));
    assert!(stderr.contains("written by `onus ci rollout`"), "{stderr}");
    assert_eq!(lines(&rec.join("signals.jsonl")).len(), 2);
    // A deploy is tied to its change and is no outcome.
    let (code, stdout, _) = ingest_text(&format!(
        r#"{{"id":"deploy:1","kind":"deploy","commit":"{sha}","environment":"production","rollout":{{"percent":25}}}}"#
    ));
    assert_eq!(code, Some(0));
    assert_eq!(json(&stdout)["outcomes"], serde_json::json!([]));
    let p = json(&onus(
        &r,
        &["provenance", "--change", "acme/shop#1", "--records", rs],
    ));
    assert_eq!(
        (&p["rollout"]["status"], &p["rollout"]["percent"]),
        (&serde_json::json!("in-progress"), &serde_json::json!(25)),
        "{p}"
    );
    assert_eq!(p["rollout"]["signals"][0]["id"], "pagerduty:Q1ABC");
}

#[test]
fn production_traces_upgrade_the_map() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let rec = t.join("records");
    let otlp = repo_root().join("crates/onus-cli/tests/otlp");
    let base = fixture().join("base");
    let traces = |file: &str| {
        json(&onus(
            t,
            &[
                "signals",
                "traces",
                "--records",
                rec.to_str().unwrap(),
                "--file",
                otlp.join(file).to_str().unwrap(),
                "--source",
                "otel-prod",
                "--repo",
                base.to_str().unwrap(),
            ],
        ))
    };
    let s = traces("order-placed.json");
    assert_eq!(
        (&s["recorded"], &s["calls"]),
        (&serde_json::json!(1), &serde_json::json!(3)),
        "{s}"
    );
    assert!(s["id"].as_str().unwrap().starts_with("trace:"));
    // The same export twice is one signal.
    assert_eq!(traces("order-placed.json")["recorded"], 0);
    let signals = lines(&rec.join("signals.jsonl"));
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0]["source"], "otel-prod");
    // The services that are the repository's components.
    assert_eq!(
        signals[0]["components"],
        serde_json::json!(["billing", "notifications", "orders"])
    );

    let map = |extra: &[&str]| {
        let base = fixture().join("base");
        let signals = rec.join("signals.jsonl");
        let mut args = vec![
            "map",
            base.to_str().unwrap(),
            "--json",
            "--traces",
            signals.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        let (code, stdout, stderr) = run(t, &args);
        assert_eq!(code, 0, "{stderr}");
        (json(&stdout), stderr)
    };
    let traced = |m: &serde_json::Value| -> Vec<(String, String)> {
        m["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["confidence"] == "traced")
            .map(|e| {
                (
                    e["from"].as_str().unwrap().into(),
                    e["to"].as_str().unwrap().into(),
                )
            })
            .collect()
    };
    // The same traced edges as from the OTLP file itself (tests/traces.rs).
    let (m, stderr) = map(&[]);
    assert!(
        stderr.contains("traces: 3 recorded calls, 2 traced calls added, 1 confirmed"),
        "{stderr}"
    );
    assert!(!stderr.contains("upgraded"), "{stderr}");
    assert_eq!(
        traced(&m),
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

    // A call the map only guessed, which production observed, is traced.
    #[cfg(unix)]
    {
        let response = t.join("guess.json");
        std::fs::write(
            &response,
            r#"{"protocol":1,"map":{"edges":[{"from":"orders:src/orders.ts#placeOrder","to":"billing:src/payments.ts#createInvoice","kind":"calls","confidence":"inferred","sites":[{"file":"services/orders/src/orders.ts","line":14}]}]}}"#,
        )
        .unwrap();
        let plugins = t.join("plugins.yaml");
        std::fs::write(
            &plugins,
            format!(
                "plugins:\n  - name: guess\n    kind: facts\n    command: [sh, -c, \"cat > /dev/null; cat '{}'\"]\n",
                response.display()
            ),
        )
        .unwrap();
        traces("inferred-confirmed.json");
        let (m, stderr) = map(&["--plugins", plugins.to_str().unwrap()]);
        assert!(stderr.contains(", 1 upgraded"), "{stderr}");
        let guessed = m["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| {
                e["from"] == "orders:src/orders.ts#placeOrder"
                    && e["to"] == "billing:src/payments.ts#createInvoice"
            })
            .unwrap();
        assert_eq!(guessed["confidence"], "traced", "{guessed}");
        // A static edge production confirmed stays static.
        let confirmed = m["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| {
                e["from"] == "billing:src/subscriptions.ts#invoiceOrder"
                    && e["to"] == "billing:src/payments.ts#createInvoice"
                    && e["kind"] == "calls"
            })
            .unwrap();
        assert_eq!(confirmed["confidence"], "static");
    }
}
