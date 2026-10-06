//! The `onus` binary: exit codes, `report` on a real git repository, `init`
//! and `map`.

mod common;

use std::path::Path;
use std::process::Command;

use common::{Scenario, apply_overlay, copy_dir, fixture};

fn onus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_onus"))
}

fn diff_exit(id: &str, fail_on: &str) -> i32 {
    let s = Scenario::new(id);
    let out = onus()
        .arg("diff")
        .arg(s.base())
        .arg(s.head())
        .args(["--fail-on", fail_on])
        .output()
        .unwrap();
    out.status.code().unwrap()
}

#[test]
fn fail_on_rule_violation_exits_2() {
    assert_eq!(diff_exit("s6-boundary-rule", "rule-violation"), 2);
    assert_eq!(diff_exit("s6-boundary-rule", "secrets"), 0);
}

#[test]
fn fail_on_secrets_exits_2() {
    assert_eq!(diff_exit("s8-secret", "secrets"), 2);
    assert_eq!(diff_exit("s8-secret", "rule-violation"), 0);
    assert_eq!(diff_exit("s1-sms-alerts", "secrets,rule-violation"), 0);
}

#[test]
fn usage_errors_exit_1_and_version_exits_0() {
    let out = onus().args(["diff", "only-one-arg"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let out = onus()
        .args(["diff", "/nonexistent/a", "/nonexistent/b"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let out = onus().arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("onus "));
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Onus Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn report_works_on_a_real_git_repository() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    copy_dir(&fixture().join("base"), repo.path());
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "base"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), repo.path());
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "sms alerts"]);

    let out = onus()
        .args(["report", "--base", "main~1", "--head", "main", "--repo"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let md = String::from_utf8(out.stdout).unwrap();

    // Same report as diffing the two directories.
    let expected = Scenario::new("s1-sms-alerts")
        .run()
        .render(onus_cli::Format::Md);
    assert_eq!(md, expected);

    let out = onus()
        .args([
            "report", "--base", "main~1", "--head", "main", "--format", "json", "--repo",
        ])
        .arg(repo.path())
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["base"].as_str().unwrap().starts_with("main~1 ("));
    assert!(json["head"].as_str().unwrap().starts_with("main ("));
    assert_eq!(json["summary"]["meaningChanges"], 4);

    let out = onus()
        .args([
            "report",
            "--base",
            "no-such-ref",
            "--head",
            "main",
            "--repo",
        ])
        .arg(repo.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn init_infers_the_hand_written_components() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture().join("base"), dir.path());
    std::fs::remove_file(dir.path().join("onus.yaml")).unwrap();
    let out = onus().arg("init").arg(dir.path()).output().unwrap();
    assert!(out.status.success());
    let inferred = onus_map::config::load(&dir.path().join("onus.yaml"))
        .unwrap()
        .config;
    let written = onus_map::config::load(&fixture().join("base/onus.yaml"))
        .unwrap()
        .config;
    let summary = |c: &onus_core::OnusConfig| -> Vec<(String, Vec<String>, Vec<String>)> {
        c.components
            .iter()
            .map(|(id, cc)| (id.clone(), cc.path.to_vec(), cc.owners.clone()))
            .collect()
    };
    assert_eq!(summary(&inferred), summary(&written));
    // Labels are suggestions only: commented out, never enforced.
    assert!(inferred.components.values().all(|c| c.labels.is_empty()));
    let text = std::fs::read_to_string(dir.path().join("onus.yaml")).unwrap();
    assert!(text.contains("# labels: [payments]  # suggestion"));
    assert!(text.contains("# labels: [pii]  # suggestion"));
    // A second run refuses to overwrite.
    let out = onus().arg("init").arg(dir.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn map_summary_and_json() {
    let out = onus()
        .arg("map")
        .arg(fixture().join("base"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("7 components,"), "{text}");
    let out = onus()
        .args(["map", "--json"])
        .arg(fixture().join("base"))
        .output()
        .unwrap();
    let map: onus_core::CodebaseMap = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(map.components.len(), 7);
}

fn stdout(args: &[&str]) -> (i32, String, String) {
    let out = onus().args(args).output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn help_lists_commands_topics_and_exit_codes() {
    let (code, text, _) = stdout(&["help"]);
    assert_eq!(code, 0);
    for command in ["map", "diff", "report", "init", "schema", "help"] {
        assert!(text.contains(&format!("\n  {command} ")), "{command}");
    }
    for t in onus_cli::guide::TOPICS {
        assert!(text.contains(t.name), "{}", t.name);
    }
    assert!(text.contains("2  a --fail-on condition was met"));
}

#[test]
fn help_shows_one_command_with_examples() {
    let (code, text, _) = stdout(&["help", "diff"]);
    assert_eq!(code, 0);
    assert!(text.contains("Usage: onus diff"));
    assert!(text.contains("--fail-on"));
    assert!(text.contains("Examples:"));
    // The same text as `onus diff --help`.
    assert_eq!(text, stdout(&["diff", "--help"]).1);
}

#[test]
fn help_prints_every_guide_topic() {
    for t in onus_cli::guide::TOPICS {
        let (code, text, _) = stdout(&["help", t.name]);
        assert_eq!(code, 0, "{}", t.name);
        assert_eq!(text, t.text, "{}", t.name);
    }
    let (_, text, _) = stdout(&["help", "onus.yaml"]);
    assert!(text.starts_with("# Configuration"));
}

#[test]
fn help_for_an_unknown_name_fails_with_the_list() {
    let (code, _, err) = stdout(&["help", "nope"]);
    assert_eq!(code, 1);
    assert!(err.contains("no command or guide topic named `nope`"));
    assert!(err.contains("getting-started"));
}
