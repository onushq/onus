//! Coverage into the map and the report (ADR 0017): lcov, Cobertura and
//! Istanbul files of the head, coverage recorded by environments at the head
//! commit, and the coverage files the map server reads for agents.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Scenario, apply_overlay, copy_dir, fixture, repo_root};
use serde_json::{Value, json};

fn cov(name: &str) -> PathBuf {
    repo_root()
        .join("crates/onus-cli/tests/coverage")
        .join(name)
}

/// Runs onus; its exit code, stdout and stderr.
fn onus(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(dir: &Path, args: &[&str]) -> (String, String) {
    let (code, out, err) = onus(dir, args);
    assert_eq!(code, 0, "onus {args:?}: {err}");
    (out, err)
}

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

const ETA: &str = "services/orders/src/shipping/eta.ts";

/// The s7 diff (a new dependency in `eta.ts`) with these extra arguments.
fn s7(extra: &[&str]) -> (Value, String) {
    let s = Scenario::new("s7-new-dependency");
    let mut args = vec!["diff", "base", "head", "--format", "json"];
    args.extend_from_slice(extra);
    let (out, err) = ok(s.dir.path(), &args);
    (serde_json::from_str(&out).unwrap(), err)
}

fn row<'a>(report: &'a Value, subkind: &str) -> &'a Value {
    report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["subkind"] == subkind)
        .unwrap_or_else(|| panic!("no {subkind} row"))
}

#[test]
fn coverage_joins_the_map() {
    let lcov = cov("discount-per-test.lcov");
    let (out, err) = ok(
        &repo_root(),
        &[
            "map",
            "fixtures/shop/base",
            "--json",
            "--coverage",
            lcov.to_str().unwrap(),
        ],
    );
    let map: Value = serde_json::from_str(&out).unwrap();
    let c = &map["coverage"];
    assert_eq!(c["files"][0]["path"], "services/billing/src/discount.ts");
    assert_eq!(c["sources"][0]["format"], "lcov");
    assert_eq!(c["sources"][0]["origin"], "supplied");
    assert_eq!(c["sources"][0]["files"], 1);
    assert!(
        c["links"].as_array().unwrap().iter().any(|l| {
            l["test"] == "billing:src/discount.test.ts"
                && l["target"] == "billing:src/discount.ts#noDiscount"
        }),
        "{}",
        c["links"]
    );
    assert!(err.contains("coverage: 1 sources (lcov 1)"), "{err}");
    assert!(err.contains("test links"), "{err}");
    // The summary says so too.
    let (summary, _) = ok(
        &repo_root(),
        &[
            "map",
            "fixtures/shop/base",
            "--coverage",
            lcov.to_str().unwrap(),
        ],
    );
    assert!(
        summary.contains("  coverage: 1 files, 9 of 9 lines ran, 4 test links"),
        "{summary}"
    );
    // A file that is not coverage is an error.
    let readme = repo_root().join("README.md");
    let (code, _, err) = onus(
        &repo_root(),
        &[
            "map",
            "fixtures/shop/base",
            "--coverage",
            readme.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 1);
    assert!(
        err.contains("is not lcov, Cobertura XML or Istanbul JSON"),
        "{err}"
    );
}

#[test]
fn the_three_formats_agree() {
    let mut seen = Vec::new();
    for name in ["eta.lcov", "eta.cobertura.xml", "eta.istanbul.json"] {
        let file = cov(name);
        let (report, err) = s7(&["--coverage", file.to_str().unwrap()]);
        let c = &report["coverage"];
        // Head lines 1 (the new import) and 13 (the new return) changed and
        // are executable; the tests ran line 1 only.
        assert_eq!(
            (c["changed"].as_u64(), c["ran"].as_u64()),
            (Some(2), Some(1)),
            "{name}: {c}"
        );
        assert_eq!(
            c["notRun"],
            json!([{ "file": ETA, "lines": [13, 13], "side": "head" }]),
            "{name}"
        );
        assert_eq!(c["notMeasured"], json!([]), "{name}");
        assert_eq!(c["sources"][0]["files"], 1, "{name}");
        assert_eq!(
            row(&report, "internal-changes")["hints"]["coverage"],
            json!({ "changed": 1, "ran": 0 }),
            "{name}"
        );
        assert_eq!(
            row(&report, "new-dependency")["hints"]["coverage"],
            json!({ "changed": 1, "ran": 1 }),
            "{name}"
        );
        assert!(err.contains("onus: coverage: 1 sources"), "{err}");
        seen.push(c["sources"][0]["format"].clone());
    }
    assert_eq!(seen, [json!("lcov"), json!("cobertura"), json!("istanbul")]);
}

#[test]
fn coverage_is_head_only_and_absent_without_it() {
    // The same tree on both sides: coverage changes nothing.
    let s = Scenario::new("s7-new-dependency");
    let file = cov("eta.lcov");
    let (out, _) = ok(
        s.dir.path(),
        &[
            "diff",
            "base",
            "base",
            "--format",
            "json",
            "--coverage",
            file.to_str().unwrap(),
        ],
    );
    let same: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(same["changes"], json!([]));
    assert_eq!(same["coverage"]["changed"], 0);
    // Without coverage, the report is the one the scenario snapshot holds.
    let (out, _) = ok(s.dir.path(), &["diff", "base", "head", "--format", "json"]);
    assert!(!out.contains("\"coverage\""));
    let snap = std::fs::read_to_string(
        repo_root().join("crates/onus-cli/tests/snapshots/scenarios__s7-new-dependency.json.snap"),
    )
    .unwrap();
    let body = snap.splitn(3, "---\n").nth(2).unwrap();
    assert_eq!(out.trim_end(), body.trim_end());
}

#[test]
fn markdown_shows_lines_no_test_ran() {
    let s = Scenario::new("s7-new-dependency");
    // A file inside the working directory is shown relative to it, even
    // when given by its absolute path.
    let file = s.dir.path().join("coverage/eta.lcov");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    // Written, not copied: a copy can keep the fixture's time, older than the sources.
    std::fs::write(&file, std::fs::read(cov("eta.lcov")).unwrap()).unwrap();
    let (md, _) = ok(
        s.dir.path(),
        &["diff", "base", "head", "--coverage", file.to_str().unwrap()],
    );
    assert!(
        md.contains("**Tests ran:** 1 of 2 changed lines (50%)"),
        "{md}"
    );
    assert!(
        md.contains("<details><summary>Changed lines no test ran: 1 line in 1 file</summary>"),
        "{md}"
    );
    assert!(md.contains(&format!("- `{ETA}` line 13\n")), "{md}");
    assert!(
        md.contains("- coverage: lcov `coverage/eta.lcov` (supplied, 1 file)"),
        "{md}"
    );
    assert!(md.contains("tests ran 0 of 1 changed line"), "{md}");
}

#[test]
fn mismatched_coverage_is_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.lcov");
    std::fs::write(
        &old,
        format!("SF:/work/{ETA}\nDA:1,1\nDA:13,1\nDA:999,1\nend_of_record\n"),
    )
    .unwrap();
    let (report, _) = s7(&["--coverage", old.to_str().unwrap()]);
    let c = &report["coverage"];
    assert_eq!(c["changed"], 0);
    assert_eq!(c["notMeasured"], json!([ETA]));
    let notes = c["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    let note = notes[0].as_str().unwrap();
    assert!(
        note.starts_with(&format!("coverage of `{ETA}` from `"))
            && note.ends_with("names line 999, but the file has 14 lines; left out"),
        "{note}"
    );
}

/// The shop at `base`, and the s7 change committed on top as `HEAD`.
fn s7_repo(t: &Path) -> (PathBuf, String, String) {
    let r = t.join("shop");
    copy_dir(&fixture().join("base"), &r);
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    git(&r, &["checkout", "-q", "-b", "claude/eta"]);
    apply_overlay(&fixture().join("scenarios/s7-new-dependency"), &r);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "Show a shipping ETA"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    (r, base, head)
}

/// Records a run at `commit` with the lcov as its coverage artifact, and
/// the HTML report the same glob caught, and writes its test run; the run
/// file's path.
fn recorded_run(r: &Path, commit: &str, name: &str) -> (PathBuf, String) {
    use onus_env::store::{Artifact, ArtifactKind, EnvironmentInfo, Manifest, SCHEMA, Store};
    let store = Store::for_repo(r).unwrap();
    let lcov = std::fs::read(cov("eta.lcov")).unwrap();
    let html = b"<html></html>".to_vec();
    let manifest = Manifest {
        schema: SCHEMA,
        commit: commit.to_string(),
        command: "npm test -- --coverage".into(),
        exit_code: 0,
        started_at: 1,
        finished_at: 2,
        environment: EnvironmentInfo {
            name: name.into(),
            image: "node:22".into(),
            warm_image: None,
            setup: None,
            seed: None,
            hosts: vec![],
            secrets: vec![],
            task: None,
        },
        artifacts: vec![
            Artifact {
                kind: ArtifactKind::Coverage,
                path: "coverage/lcov-report/index.html".into(),
                sha256: store.put(&html).unwrap(),
                size: html.len() as u64,
            },
            Artifact {
                kind: ArtifactKind::Coverage,
                path: "coverage/lcov.info".into(),
                sha256: store.put(&lcov).unwrap(),
                size: lcov.len() as u64,
            },
        ],
        tests: None,
        overlay: None,
    };
    let id = store.record(&manifest).unwrap();
    let run = onus_doors::runner::TestRun {
        commit: commit.to_string(),
        image: "node:22".into(),
        setup: None,
        command: manifest.command.clone(),
        exit_code: 0,
        output_tail: String::new(),
        manifest: Some(id.clone()),
    };
    let path = r.parent().unwrap().join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_string(&run).unwrap()).unwrap();
    (path, id)
}

#[test]
fn report_reads_recorded_coverage_at_head_only() {
    let tmp = tempfile::tempdir().unwrap();
    let (r, base, head) = s7_repo(tmp.path());
    let (at_head, id) = recorded_run(&r, &head, "at-head");
    let (out, err) = ok(
        &r,
        &[
            "report",
            "--base",
            "main",
            "--head",
            "HEAD",
            "--format",
            "json",
            "--evidence",
            at_head.to_str().unwrap(),
        ],
    );
    let report: Value = serde_json::from_str(&out).unwrap();
    let c = &report["coverage"];
    assert_eq!(c["sources"][0]["origin"], "recorded", "{err}");
    assert_eq!(c["sources"][0]["manifest"], id);
    assert_eq!(c["sources"][0]["path"], "coverage/lcov.info");
    assert_eq!(
        (c["changed"].as_u64(), c["ran"].as_u64()),
        (Some(2), Some(1))
    );
    // A recorded file that is not coverage is a note, not a failure.
    let note =
        "coverage/lcov-report/index.html is not lcov, Cobertura XML or Istanbul JSON; left out";
    assert_eq!(c["sources"].as_array().unwrap().len(), 1);
    assert_eq!(c["notes"], json!([note]));
    assert!(err.contains(note), "{err}");

    // A run at the base commit counts for nothing.
    let (at_base, _) = recorded_run(&r, &base, "at-base");
    let (out, err) = ok(
        &r,
        &[
            "report",
            "--base",
            "main",
            "--head",
            "HEAD",
            "--format",
            "json",
            "--evidence",
            at_base.to_str().unwrap(),
        ],
    );
    let report: Value = serde_json::from_str(&out).unwrap();
    assert!(report.get("coverage").is_none());
    assert!(err.contains("not the head commit"), "{err}");

    // A submission without a report (onus ci pr, onus_submit) reads it too.
    let (out, _) = ok(
        &r,
        &[
            "submit",
            "--base",
            "main",
            "--head",
            "HEAD",
            "--evidence",
            at_head.to_str().unwrap(),
        ],
    );
    let sub: Value = serde_json::from_str(&out).unwrap();
    let source = &sub["report"]["coverage"]["sources"][0];
    assert_eq!(source["origin"], "recorded");
    assert_eq!(source["command"], "npm test -- --coverage");

    // The agent picks the commands it runs, so the lane counts recorded
    // coverage only from the commands the policy names. Coverage from any
    // other command sends the change to the judge, required or not.
    let sub_path = tmp.path().join("sub.json");
    std::fs::write(&sub_path, &out).unwrap();
    let config = tmp.path().join("onus.yaml");
    let classify = |commands: &str| -> Vec<String> {
        std::fs::write(
            &config,
            format!(
                "version: 1\nlanes:\n  default: auto-merge\n  minRecord: 0\n  auditRate: 0\n  inputs:\n    coverage: {{ minChanged: 0.5{commands} }}\n"
            ),
        )
        .unwrap();
        let (out, err) = ok(
            tmp.path(),
            &[
                "classify",
                "--submission",
                sub_path.to_str().unwrap(),
                "--config",
                config.to_str().unwrap(),
                "--repo",
                r.to_str().unwrap(),
            ],
        );
        let c: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {err}"));
        c["applied"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| {
                a["source"] == "input" && a["reason"].as_str().unwrap().contains("coverage")
            })
            .map(|a| a["reason"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        classify(""),
        [
            "coverage was recorded by `npm test -- --coverage`, which lanes.inputs.coverage.commands does not name; auto-merge needs all recorded coverage from named commands"
        ]
    );
    assert!(
        classify(", commands: [\"npm test -- --coverage\"]").is_empty(),
        "the named command's coverage counts"
    );
}

#[test]
fn query_tests_for_uses_configured_coverage() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().join("shop");
    copy_dir(&fixture().join("base"), &r);
    let config = r.join("onus.yaml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        format!("{text}tests:\n  coverage: [\"coverage/lcov.info\"]\n"),
    )
    .unwrap();
    std::fs::create_dir_all(r.join("coverage")).unwrap();
    // Written, not copied: on macOS a copy keeps the fixture's time, which can be older
    // than the sources the checkout wrote after it, and stale coverage is left out.
    std::fs::write(
        r.join("coverage/lcov.info"),
        std::fs::read(cov("discount-per-test.lcov")).unwrap(),
    )
    .unwrap();
    let query = || -> Value {
        let (out, _) = ok(
            &r,
            &[
                "query",
                "tests-for",
                "noDiscount",
                "--repo",
                r.to_str().unwrap(),
                "--no-server",
            ],
        );
        serde_json::from_str(&out).unwrap()
    };
    let answer = query();
    let tests = &answer["result"]["tests"];
    assert_eq!(tests[0]["file"], "services/billing/src/discount.test.ts");
    assert_eq!(tests[0]["link"], "ran");
    assert_eq!(
        answer["result"]["coverage"],
        json!({ "executable": 1, "ran": 1 })
    );

    // An edit after the tests ran: the file's coverage is left out.
    let discount = std::fs::File::options()
        .write(true)
        .open(r.join("services/billing/src/discount.ts"))
        .unwrap();
    discount
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(1))
        .unwrap();
    let answer = query();
    assert!(
        answer["result"]["tests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["link"] != "ran"),
        "{answer}"
    );
    let (out, _) = ok(
        &r,
        &[
            "query",
            "status",
            "--repo",
            r.to_str().unwrap(),
            "--no-server",
        ],
    );
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        status["result"]["coverage"]["notes"],
        json!([
            "coverage of `services/billing/src/discount.ts` is older than the file; left out (re-run the tests)"
        ])
    );
}

#[test]
fn lanes_coverage_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let (report, _) = s7(&["--coverage", cov("eta.lcov").to_str().unwrap()]);
    let path = tmp.path().join("r.json");
    std::fs::write(&path, report.to_string()).unwrap();
    let config = tmp.path().join("onus.yaml");
    std::fs::write(
        &config,
        "version: 1\nlanes:\n  default: auto-merge\n  minRecord: 0\n  auditRate: 0\n  inputs:\n    coverage: { minChanged: 0.9 }\n",
    )
    .unwrap();
    let (out, err) = ok(
        tmp.path(),
        &[
            "classify",
            "--report",
            path.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
            "--repo",
            tmp.path().to_str().unwrap(),
        ],
    );
    let c: Value = serde_json::from_str(&out).unwrap();
    // `date-fns` is new to the repository, a floor that sends it to a
    // person; the coverage step asks for the judge.
    assert_eq!(c["lane"], "human", "{err}");
    let input = c["applied"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["source"] == "input" && a["reason"].as_str().unwrap().starts_with("tests ran"))
        .unwrap_or_else(|| panic!("no coverage input: {c}"));
    assert_eq!(
        input["reason"],
        "tests ran 1 of 2 changed lines (50%); auto-merge needs 90%"
    );
    assert_eq!(input["lane"], "judge");
    assert_eq!(input["rows"], json!(["internal-changes:orders"]));
    // Without a floor, the coverage step decides the lane: an edit to
    // `eta.ts` whose changed line no test ran goes to the judge.
    let edit_dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture().join("base"), &edit_dir.path().join("base"));
    copy_dir(&fixture().join("base"), &edit_dir.path().join("head"));
    let eta = edit_dir.path().join("head").join(ETA);
    let text = std::fs::read_to_string(&eta).unwrap();
    std::fs::write(
        &eta,
        text.replace("  return eta;", "  return new Date(eta);"),
    )
    .unwrap();
    let lcov = tmp.path().join("edit.lcov");
    std::fs::write(
        &lcov,
        format!("SF:/work/{ETA}\nDA:1,1\nDA:14,1\nDA:15,1\nDA:24,0\nend_of_record\n"),
    )
    .unwrap();
    let (out, err) = ok(
        edit_dir.path(),
        &[
            "diff",
            "base",
            "head",
            "--format",
            "json",
            "--coverage",
            lcov.to_str().unwrap(),
        ],
    );
    let edit: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (
            edit["coverage"]["changed"].as_u64(),
            edit["coverage"]["ran"].as_u64()
        ),
        (Some(1), Some(0)),
        "{err}"
    );
    std::fs::write(&path, edit.to_string()).unwrap();
    let classify = || -> Value {
        let (out, _) = ok(
            tmp.path(),
            &[
                "classify",
                "--report",
                path.to_str().unwrap(),
                "--config",
                config.to_str().unwrap(),
                "--repo",
                tmp.path().to_str().unwrap(),
            ],
        );
        serde_json::from_str(&out).unwrap()
    };
    let c = classify();
    assert_eq!(c["lane"], "judge", "{c}");
    let reasons: Vec<&str> = c["applied"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["lane"] == "judge")
        .map(|a| a["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        reasons,
        ["tests ran 0 of 1 changed line (0%); auto-merge needs 90%"],
        "{c}"
    );
    // Once the line ran, it auto-merges.
    std::fs::write(
        &lcov,
        format!("SF:/work/{ETA}\nDA:1,1\nDA:14,1\nDA:15,1\nDA:24,3\nend_of_record\n"),
    )
    .unwrap();
    let (out, _) = ok(
        edit_dir.path(),
        &[
            "diff",
            "base",
            "head",
            "--format",
            "json",
            "--coverage",
            lcov.to_str().unwrap(),
        ],
    );
    std::fs::write(&path, out).unwrap();
    assert_eq!(classify()["lane"], "auto-merge");

    // Coverage of other files leaves `eta.ts` out: it counts as not run, so
    // giving that coverage cannot let the change through.
    let (other, _) = s7(&[
        "--coverage",
        cov("discount-per-test.lcov").to_str().unwrap(),
    ]);
    assert_eq!(other["coverage"]["notMeasured"], json!([ETA]));
    std::fs::write(&path, other.to_string()).unwrap();
    let (out, err) = ok(
        tmp.path(),
        &[
            "classify",
            "--report",
            path.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
            "--repo",
            tmp.path().to_str().unwrap(),
        ],
    );
    let c: Value = serde_json::from_str(&out).unwrap();
    let input = c["applied"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["source"] == "input" && a["reason"].as_str().unwrap().contains("leaves out"))
        .unwrap_or_else(|| panic!("no coverage input: {c} {err}"));
    assert_eq!(
        input["reason"],
        format!(
            "the coverage leaves out 1 changed file with runtime code (`{ETA}`); auto-merge needs every changed file measured"
        )
    );
    assert_eq!(input["lane"], "judge");
    // A share above 1 is refused.
    std::fs::write(
        &config,
        "version: 1\nlanes:\n  inputs:\n    coverage: { minChanged: 90 }\n",
    )
    .unwrap();
    let (code, _, err) = onus(
        tmp.path(),
        &[
            "classify",
            "--report",
            path.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("minChanged is a share from 0 to 1"), "{err}");
}
