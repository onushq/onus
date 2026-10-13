//! The behavior diff end to end (ADR 0019): `onus report --behavior` runs
//! the tests that reach a change at the base and at the head, in
//! environments, and adds what changed as rows the lanes act on. Skipped
//! without a container engine.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn onus(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .current_dir(dir)
        .args(args)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
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

/// Node 22's own test runner writes JUnit: no network, no setup.
const ONUS_YAML: &str = r#"version: 1
components: { shop: { path: "src/**" } }
environment:
  image: node:22-alpine
  behavior:
    command: "mkdir -p reports && node --test --test-reporter=junit --test-reporter-destination=reports/onus-behavior.xml {files}"
    junit: ["reports/onus-behavior.xml"]
    budget: 300
"#;

const DISCOUNT: &str = "export function discount(t) { return t > 100 ? 10 : 0 }\n";

const DISCOUNT_TEST: &str = "import { test } from 'node:test'
import assert from 'node:assert/strict'
import { discount } from './discount.mjs'

test('gives 10 above the threshold', () => {
  assert.equal(discount(150), 10)
})
test('discount at the threshold', () => {
  assert.equal(discount(100), 0)
})
";

const TAX: &str = "export function tax(n) { return n * 0.2 }\n";

const TAX_TEST: &str = "import { test } from 'node:test'
import assert from 'node:assert/strict'
import { tax } from './tax.mjs'

test('taxes 20 percent', () => {
  assert.equal(tax(10), 2)
})
";

/// Rounds up: its test fails at the base.
const REFUND: &str = "export function refund(n) { return Math.ceil(n) }\n";

const REFUND_TEST: &str = "import { test } from 'node:test'
import assert from 'node:assert/strict'
import { refund } from './refund.mjs'

test('rounds refunds down', () => {
  assert.equal(refund(4.5), 4)
})
";

/// Removes the behavior diff's environments when the test ends, passing or
/// not.
struct Cleanup;

impl Drop for Cleanup {
    fn drop(&mut self) {
        let Ok(engine) = onus_doors::runner::engine() else {
            return;
        };
        let out = Command::new(&engine)
            .args([
                "ps",
                "-a",
                "--filter",
                "label=onus.env",
                "--format",
                "{{.Names}}",
            ])
            .output()
            .unwrap();
        for name in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(env) = name.strip_prefix("onus-env-")
                && env.starts_with("bhv-")
            {
                let _ = Command::new(&engine).args(["rm", "-f", name]).output();
            }
        }
    }
}

/// The behavior environments still there.
fn leftover() -> Vec<String> {
    let engine = onus_doors::runner::engine().unwrap();
    let out = Command::new(engine)
        .args([
            "ps",
            "-a",
            "--filter",
            "label=onus.env",
            "--format",
            "{{.Names}}",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|n| n.starts_with("onus-env-bhv-"))
        .map(str::to_string)
        .collect()
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn write(r: &Path, path: &str, text: &str) {
    let p = r.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// `main`, and a branch from it with `edit` applied.
fn branch(r: &Path, name: &str, edit: &[(&str, &str)]) {
    git(r, &["checkout", "-q", "-b", name, "main"]);
    for (path, text) in edit {
        write(r, path, text);
    }
    git(r, &["commit", "-q", "-am", name]);
    git(r, &["checkout", "-q", "main"]);
}

fn behavior_rows(report: &serde_json::Value) -> Vec<String> {
    report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["subkind"].as_str().unwrap().starts_with("behavior-"))
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

fn sides(report: &serde_json::Value) -> Vec<String> {
    report["behavior"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["side"].as_str().unwrap().to_string())
        .collect()
}

/// `onus classify` on a report: its lane and the reasons of its floors.
fn classify(r: &Path, report: &str) -> (String, Vec<String>) {
    let path = r.join("report.json");
    std::fs::write(&path, report).unwrap();
    let (code, out, err) = onus(r, &["classify", "--report", path.to_str().unwrap()], &[]);
    assert!(code == 0, "{err}");
    let c = json(&out);
    let floors = c["applied"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["source"] == "floor")
        .map(|a| a["reason"].as_str().unwrap().to_string())
        .collect();
    (c["lane"].as_str().unwrap().to_string(), floors)
}

#[test]
fn behavior_diff_runs_tests_at_base_and_head() {
    if onus_doors::runner::engine().is_err() {
        return;
    }
    let _cleanup = Cleanup;
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().join("repo");
    std::fs::create_dir_all(&r).unwrap();
    write(&r, "onus.yaml", ONUS_YAML);
    write(&r, "src/discount.mjs", DISCOUNT);
    write(&r, "src/discount.test.mjs", DISCOUNT_TEST);
    write(&r, "src/tax.mjs", TAX);
    write(&r, "src/tax.test.mjs", TAX_TEST);
    write(&r, "src/refund.mjs", REFUND);
    write(&r, "src/refund.test.mjs", REFUND_TEST);
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);
    let base = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&r)
            .args(["rev-parse", "main"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let at_threshold = DISCOUNT.replace("t > 100", "t >= 100");
    branch(
        &r,
        "edited-test",
        &[
            ("src/discount.mjs", &at_threshold),
            (
                "src/discount.test.mjs",
                &DISCOUNT_TEST.replace("discount(100), 0", "discount(100), 10"),
            ),
        ],
    );
    branch(&r, "broken", &[("src/discount.mjs", &at_threshold)]);
    branch(
        &r,
        "fix",
        &[("src/refund.mjs", &REFUND.replace("ceil", "floor"))],
    );
    let report = |head: &str, extra: &[&str], env: &[(&str, &str)]| {
        let mut args = vec![
            "report",
            "--base",
            "main",
            "--head",
            head,
            "--format",
            "json",
            "--behavior",
        ];
        args.extend(extra);
        onus(&r, &args, env)
    };

    // 1. The test was edited to match the new behavior.
    let (code, out, err) = report("edited-test", &[], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("onus: behavior: 1 test file selected"),
        "{err}"
    );
    let rep = json(&out);
    assert_eq!(rep["behavior"]["status"], "complete", "{}", rep["behavior"]);
    assert_eq!(behavior_rows(&rep), ["behavior-changed:shop"]);
    assert_eq!(sides(&rep), ["base", "head", "cross"]);
    let selected: Vec<&str> = rep["behavior"]["selected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["file"].as_str().unwrap())
        .collect();
    assert_eq!(selected, ["src/discount.test.mjs"]);
    let row = rep["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "behavior-changed:shop")
        .unwrap();
    assert_eq!(row["hints"]["confidence"], "traced");
    assert!(
        row["whyItMatters"].as_str().unwrap().contains(
            "\"discount at the threshold\" (`src/discount.test.mjs`) fails against the new code"
        ),
        "{row}"
    );
    let (lane, floors) = classify(&r, &out);
    assert_eq!(lane, "human", "{floors:?}");
    assert!(
        floors
            .iter()
            .any(|f| f.contains("changes the rules of the game")),
        "{floors:?}"
    );
    // Every run is evidence, and the cross run says the tree was altered.
    for run in rep["behavior"]["runs"].as_array().unwrap() {
        let id = run["manifest"].as_str().unwrap();
        let (code, shown, err) = onus(&r, &["evidence", "show", id], &[]);
        assert_eq!(code, 0, "{err}");
        let shown = json(&shown);
        if run["side"] == "cross" {
            assert_eq!(shown["manifest"]["overlay"]["fromCommit"], base.as_str());
            assert_eq!(
                shown["manifest"]["overlay"]["paths"],
                serde_json::json!(["src/discount.test.mjs"])
            );
        } else {
            assert!(shown["manifest"].get("overlay").is_none());
        }
    }
    // The Markdown says what ran.
    let (code, md, _) = onus(
        &r,
        &[
            "report",
            "--base",
            "main",
            "--head",
            "edited-test",
            "--behavior",
        ],
        &[],
    );
    assert_eq!(code, 0);
    assert!(md.contains("**Behavior:** 1 test file run at base and head (selected from the map) · 1 changed, 0 newly failing"), "{md}");
    assert!(md.contains("<details><summary>Behavior runs</summary>"));

    // 2. The behavior changed and the test did not.
    let (code, out, err) = report("broken", &["--fail-on", "behavior"], &[]);
    assert_eq!(code, 2, "{err}");
    let rep = json(&out);
    assert_eq!(behavior_rows(&rep), ["behavior-newly-failing:shop"]);
    assert_eq!(sides(&rep), ["base", "head", "retry"]);
    let (lane, floors) = classify(&r, &out);
    assert_eq!(lane, "blocked");
    assert!(
        floors
            .iter()
            .any(|f| f.contains("tests that pass at the base fail at the head")),
        "{floors:?}"
    );

    // 3. A fix: the failing test passes, and nothing is blocked.
    let (code, out, err) = report("fix", &["--fail-on", "behavior"], &[]);
    assert_eq!(code, 0, "{err}");
    let rep = json(&out);
    assert_eq!(behavior_rows(&rep), ["behavior-newly-passing:shop"]);
    let (lane, floors) = classify(&r, &out);
    assert_ne!(lane, "blocked");
    assert!(floors.is_empty(), "{floors:?}");

    // 4. The budget runs out: partial, no rows, nothing left running. A
    // slow command makes the runs outlast it.
    let slow = r.join("slow.yaml");
    std::fs::write(
        &slow,
        ONUS_YAML.replace("command: \"mkdir", "command: \"sleep 30 && mkdir"),
    )
    .unwrap();
    let (code, out, err) = report(
        "broken",
        &[
            "--behavior-budget",
            "1",
            "--config",
            slow.to_str().unwrap(),
            "--fail-on",
            "behavior",
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let rep = json(&out);
    assert_eq!(rep["behavior"]["status"], "partial");
    assert!(
        rep["behavior"]["reason"]
            .as_str()
            .unwrap()
            .contains("the budget of 1s ran out"),
        "{}",
        rep["behavior"]
    );
    assert!(behavior_rows(&rep).is_empty());
    assert_eq!(leftover(), Vec::<String>::new());

    // 5. No container engine: skipped, and the report as usual.
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let git_path = String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_string();
    std::os::unix::fs::symlink(git_path, bin.join("git")).unwrap();
    let (code, out, err) = report("broken", &[], &[("PATH", bin.to_str().unwrap())]);
    assert_eq!(code, 0, "{err}");
    let rep = json(&out);
    assert_eq!(rep["behavior"]["status"], "skipped");
    assert!(
        rep["behavior"]["reason"]
            .as_str()
            .unwrap()
            .contains("no container engine"),
        "{}",
        rep["behavior"]
    );
    assert!(behavior_rows(&rep).is_empty());
    assert!(
        rep["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["subkind"] == "boundary-condition-changed")
    );
    assert!(
        err.contains("onus: behavior: skipped: no container engine"),
        "{err}"
    );
}
