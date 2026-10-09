//! Phase 5 end to end: environments built from a commit run commands,
//! record evidence in the content addressed store, and the judge checks
//! claimed runs against their manifests. Skipped without a container engine;
//! the egress proxy check also needs ONUS_TEST_NETWORK=1.
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

const ONUS_YAML: &str = r#"version: 1
environment:
  image: node:22-alpine
  setup: "echo installed > .deps"
  seed: "mkdir -p data && echo '{\"users\": 3}' > data/seed.json"
  evidence: ["reports/*.xml"]
  traces: ["otel/*.json"]
"#;

const TEST_SH: &str = r#"set -e
test -f .deps && test -f data/seed.json
mkdir -p reports otel
echo '<testsuites><testsuite name="a" tests="2" failures="0"><testcase name="x"/><testcase name="y"/></testsuite></testsuites>' > reports/junit.xml
echo '{"resourceSpans": []}' > otel/trace.json
echo "key is ${API_KEY:-none}"
"#;

/// Removes the environments when the test ends, passing or not.
struct Cleanup(Vec<String>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for name in &self.0 {
            let _ = Command::new(env!("CARGO_BIN_EXE_onus"))
                .args(["env", "destroy", name])
                .output();
        }
    }
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[test]
fn environments_record_evidence_the_judge_can_check() {
    if onus_doors::runner::engine().is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path().join("repo");
    std::fs::create_dir_all(&r).unwrap();
    std::fs::write(r.join("onus.yaml"), ONUS_YAML).unwrap();
    std::fs::write(r.join("package-lock.json"), "{\"lockfileVersion\": 3}\n").unwrap();
    std::fs::write(r.join("test.sh"), TEST_SH).unwrap();
    std::fs::write(r.join("old.txt"), "old\n").unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "base"]);

    // The test runner sees the commit's files, whatever the engine runs in
    // (a VM such as colima shares no temporary directories with the host).
    let (code, out, err) = onus(
        &r,
        &["run-test", "--image", "node:22-alpine", "--", "cat old.txt"],
        &[],
    );
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(json(&out)["outputTail"], "old");

    let id = std::process::id();
    let (first, second, task) = (format!("t{id}-a"), format!("t{id}-b"), format!("t{id}-c"));
    let _cleanup = Cleanup(vec![first.clone(), second.clone(), task.clone()]);

    // An environment: warm image, the commit's files, the seed.
    let (code, out, err) = onus(&r, &["env", "create", "--name", &first], &[]);
    assert_eq!(code, 0, "{err}");
    let e = json(&out);
    let warm = e["warmImage"].as_str().unwrap().to_string();
    assert!(warm.starts_with("onus-warm:"), "{e}");
    assert_eq!(e["hosts"], serde_json::json!([]));

    // A run leaves a manifest, the log, the JUnit file and the traces.
    let (code, out, err) = onus(&r, &["env", "run", &first, "--", "sh", "test.sh"], &[]);
    assert_eq!(code, 0, "{out}\n{err}");
    let run = json(&out);
    assert_eq!(run["outputTail"], "key is none");
    let manifest = run["manifest"].as_str().unwrap().to_string();
    let (code, out, err) = onus(&r, &["evidence", "show", &manifest[..10]], &[]);
    assert_eq!(code, 0, "{err}");
    let m = json(&out)["manifest"].clone();
    assert_eq!(m["tests"]["tests"], 2, "{m}");
    let kinds: Vec<&str> = m["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["log", "junit", "trace"], "{m}");
    let (_, trace, _) = onus(
        &r,
        &[
            "evidence",
            "show",
            &manifest,
            "--artifact",
            "otel/trace.json",
        ],
        &[],
    );
    assert_eq!(trace, "{\"resourceSpans\": []}");
    let (_, list, _) = onus(&r, &["evidence", "list"], &[]);
    assert!(
        list.contains(&manifest[..12]) && list.contains("2 tests, 0 failed"),
        "{list}"
    );

    // No network without a token that names hosts.
    let (code, out, _) = onus(
        &r,
        &[
            "env",
            "run",
            &first,
            "--",
            "wget -q -T 3 -O- http://example.com",
        ],
        &[],
    );
    assert_eq!(code, 1, "{out}");

    // A later commit reuses the warm image; files it deleted are gone.
    git(&r, &["rm", "-q", "old.txt"]);
    git(&r, &["commit", "-q", "-m", "drop old.txt"]);
    let (code, out, err) = onus(&r, &["env", "create", "--name", &second], &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(json(&out)["warmImage"], warm.as_str());
    let (code, out, _) = onus(&r, &["env", "run", &second, "--", "test ! -e old.txt"], &[]);
    assert_eq!(code, 0, "{out}");

    // The token's secrets come from ONUS_SECRET_<NAME> and never reach the
    // store; its hosts open the egress proxy to exactly those hosts.
    let keys = dir.path().join("keys");
    let (code, _, err) = onus(
        &r,
        &["token", "keygen", "--out", keys.to_str().unwrap()],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let network = std::env::var_os("ONUS_TEST_NETWORK").is_some();
    let hosts = if network {
        "hosts: [example.com]\n"
    } else {
        ""
    };
    let plan = dir.path().join("plan.yaml");
    std::fs::write(
        &plan,
        format!("task: {task}\nwrites: [\"src/**\"]\nsecrets: [API_KEY]\n{hosts}"),
    )
    .unwrap();
    let (code, token, err) = onus(
        &r,
        &[
            "token",
            "mint",
            "--plan",
            plan.to_str().unwrap(),
            "--key",
            keys.join("root.key").to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let public = keys.join("root.pub");
    let create = [
        "env",
        "create",
        "--token",
        &token,
        "--key-public",
        public.to_str().unwrap(),
    ];
    let (code, _, err) = onus(&r, &create, &[]);
    assert_ne!(code, 0);
    assert!(err.contains("ONUS_SECRET_API_KEY is not set"), "{err}");
    let (code, out, err) = onus(&r, &create, &[("ONUS_SECRET_API_KEY", "s3cr3t-value")]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        json(&out)["name"],
        task.as_str(),
        "the token's task names it"
    );
    let (code, out, err) = onus(&r, &["env", "run", &task, "--", "sh", "test.sh"], &[]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(json(&out)["outputTail"], "key is [secret API_KEY]");
    let store = String::from_utf8(
        Command::new("grep")
            .args(["-rl", "s3cr3t-value"])
            .arg(r.join(".git/onus/evidence"))
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(store, "", "a secret value reached the evidence store");
    if network {
        let (code, out, _) = onus(
            &r,
            &[
                "env",
                "run",
                &task,
                "--",
                "wget -q -T 10 -O- http://example.com >/dev/null",
            ],
            &[],
        );
        assert_eq!(code, 0, "{out}");
        let (code, out, _) = onus(
            &r,
            &[
                "env",
                "run",
                &task,
                "--",
                "wget -q -T 10 -O- https://github.com >/dev/null",
            ],
            &[],
        );
        assert_eq!(
            code, 1,
            "the proxy allowed a host the token does not name: {out}"
        );
    }

    // The judge checks a claimed run against its manifest.
    let runs = dir.path().join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let (_, out, _) = onus(&r, &["env", "run", &second, "--", "sh", "test.sh"], &[]);
    let honest = runs.join("honest.json");
    std::fs::write(&honest, &out).unwrap();
    let mut forged = json(&out);
    forged["command"] = "sh test.sh --all".into();
    let forged_file = runs.join("forged.json");
    std::fs::write(&forged_file, forged.to_string()).unwrap();
    for (file, passes) in [(&honest, true), (&forged_file, false)] {
        let sub = runs.join("submission.json");
        let (code, _, err) = onus(
            &r,
            &[
                "submit",
                "--base",
                "HEAD~1",
                "--head",
                "HEAD",
                "--evidence",
                file.to_str().unwrap(),
                "--agent-tool",
                "test",
                "--out",
                sub.to_str().unwrap(),
            ],
            &[],
        );
        assert_eq!(code, 0, "{err}");
        let (_, out, err) = onus(&r, &["judge", "--submission", sub.to_str().unwrap()], &[]);
        let j = json(&out);
        let evidence = &j["judgment"]["steps"][0];
        assert_eq!(evidence["name"], "evidence");
        if passes {
            assert_eq!(evidence["status"], "passed", "{j}\n{err}");
            assert!(
                evidence.to_string().contains("matched their manifests"),
                "{j}"
            );
        } else {
            assert_eq!(evidence["status"], "failed", "{j}");
            assert!(
                evidence.to_string().contains("does not match manifest"),
                "{j}"
            );
        }
    }

    // Listing and removing.
    let (_, list, _) = onus(&r, &["env", "list"], &[]);
    assert!(list.contains(&first) && list.contains(&task), "{list}");
    let (code, _, _) = onus(&r, &["env", "destroy", &first], &[]);
    assert_eq!(code, 0);
    let (code, _, err) = onus(&r, &["env", "destroy", &first], &[]);
    assert_ne!(code, 0);
    assert!(err.contains("no environment"), "{err}");
}
