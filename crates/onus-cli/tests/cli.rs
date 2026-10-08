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
fn the_base_map_cache_gives_the_same_report_and_is_reused() {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    copy_dir(&fixture().join("base"), repo.path());
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "base"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), repo.path());
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "sms alerts"]);
    let cache = tempfile::tempdir().unwrap();

    let report = |extra: &[&std::ffi::OsStr]| {
        let out = onus()
            .args([
                "report", "--base", "main~1", "--head", "main", "--format", "json", "--repo",
            ])
            .arg(repo.path())
            .args(extra)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        (
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    };
    let cached = [
        std::ffi::OsStr::new("--cache-dir"),
        cache.path().as_os_str(),
    ];
    let (plain, _) = report(&[]);
    let (cold, cold_err) = report(&cached);
    let (warm, warm_err) = report(&cached);
    assert_eq!(plain, cold);
    assert_eq!(cold, warm);
    assert!(!cold_err.contains("using the cached map"));
    assert!(warm_err.contains("using the cached map"));

    // The warm run really read the file: a cached map without the base's
    // symbols changes the report.
    let files: Vec<_> = std::fs::read_dir(cache.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let mut map: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
    let original = map.clone();
    map["symbols"] = serde_json::json!([]);
    std::fs::write(&files[0], map.to_string()).unwrap();
    assert!(report(&cached).0 != warm, "the cached map was not used");
    let mut map = original;

    // A cached map for another commit is ignored.
    map["commit"] = serde_json::json!("0000000");
    std::fs::write(&files[0], map.to_string()).unwrap();
    let (again, err) = report(&cached);
    assert_eq!(again, warm);
    assert!(err.contains("ignoring unreadable cached map"));
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

#[test]
fn both_trees_of_a_report_match_their_commits() {
    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    git(r, &["init", "-q", "-b", "main"]);
    let write = |p: &str, text: &str| {
        let path = r.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("onus.yaml", "components: {}\n");
    write("src/.gitignore", "*.log\n");
    write("src/kept.ts", "export const kept = 1;\n");
    write("src/changed.ts", "export const v = 1;\n");
    write("src/gone.ts", "export const gone = 1;\n");
    write("src/odd name[1].ts", "export const odd = 1;\n");
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "base"]);
    write("src/changed.ts", "export const v = 2;\n");
    write("src/new.ts", "export const fresh = 1;\n");
    write("src/odd name[1].ts", "export const odd = 2;\n");
    std::fs::remove_file(r.join("src/gone.ts")).unwrap();
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "head"]);

    let read = |root: &Path, p: &str| std::fs::read_to_string(root.join(p)).ok();
    for partial in [false, true] {
        let pair = onus_cli::materialize_pair(r, "main~1", "main", partial).unwrap();
        let (base, head) = (pair.base.dir.path(), pair.head.dir.path());
        let changed: Vec<&str> = pair.changed.iter().map(String::as_str).collect();
        assert_eq!(
            changed,
            [
                "src/changed.ts",
                "src/gone.ts",
                "src/new.ts",
                "src/odd name[1].ts"
            ]
        );
        assert_eq!(
            read(head, "src/changed.ts").as_deref(),
            Some("export const v = 2;\n")
        );
        assert_eq!(read(head, "src/gone.ts"), None);
        assert_eq!(
            read(base, "src/changed.ts").as_deref(),
            Some("export const v = 1;\n")
        );
        assert_eq!(
            read(base, "src/odd name[1].ts").as_deref(),
            Some("export const odd = 1;\n")
        );
        assert_eq!(
            read(base, "src/gone.ts").as_deref(),
            Some("export const gone = 1;\n")
        );
        assert_eq!(read(base, "src/new.ts"), None);
        // What config loading and the walk read is always there.
        assert!(read(base, "onus.yaml").is_some() && read(base, "src/.gitignore").is_some());
        // Unchanged files only once the partial base is completed.
        assert_eq!(read(base, "src/kept.ts").is_some(), !partial);
        pair.complete_base().unwrap();
        assert_eq!(
            read(base, "src/kept.ts").as_deref(),
            Some("export const kept = 1;\n")
        );
        assert_eq!(read(base, "src/new.ts"), None);
    }
}
