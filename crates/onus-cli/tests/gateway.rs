//! Phase 3's gate: an agent working through the git gateway can read only
//! its read scope and land writes only in its write scope, whatever it
//! tries (ADR 0008). Every refused push leaves the real repository
//! untouched.
#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use common::{copy_dir, fixture};

fn onus() -> Command {
    Command::new(env!("CARGO_BIN_EXE_onus"))
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Agent",
            "-c",
            "user.email=agent@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap()
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    upstream: PathBuf,
    gateway: Child,
    addr: String,
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = self.gateway.kill();
        let _ = self.gateway.wait();
    }
}

fn mint(root: &Path, plan: &str) -> String {
    let path = root.join("plan.yaml");
    std::fs::write(&path, plan).unwrap();
    let out = onus()
        .args(["token", "mint", "--plan"])
        .arg(&path)
        .arg("--key")
        .arg(root.join("keys/root.key"))
        .arg("--audit")
        .arg(root.join("state/audit.jsonl"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // The real repository: a bare "forge" and the gateway's clone of it.
    let seed = root.join("seed");
    copy_dir(&fixture().join("base"), &seed);
    ok(&seed, &["init", "-q", "-b", "main"]);
    ok(&seed, &["add", "-A"]);
    ok(&seed, &["commit", "-q", "-m", "base"]);
    let upstream = root.join("forge.git");
    ok(
        &root,
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_str().unwrap(),
            upstream.to_str().unwrap(),
        ],
    );
    let real = root.join("gateway-clone");
    ok(
        &root,
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            real.to_str().unwrap(),
        ],
    );
    let out = onus()
        .args(["token", "keygen", "--out"])
        .arg(root.join("keys"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut gateway = onus()
        .args(["gateway", "serve", "--repo"])
        .arg(&real)
        .args(["--remote", "origin", "--base", "main", "--key-public"])
        .arg(root.join("keys/root.pub"))
        .arg("--state")
        .arg(root.join("state"))
        .args(["--listen", "127.0.0.1:0"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(gateway.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .split("http://")
        .nth(1)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_string();
    Setup {
        _dir: dir,
        root,
        upstream,
        gateway,
        addr,
    }
}

const PLAN: &str = "task: sms\nwrites: [\"services/notifications/**\"]\nreads: [\"services/orders/**\", \"package.json\"]\n";

fn clone(s: &Setup, task: &str, token: &str, into: &str) -> (PathBuf, Output) {
    let dir = s.root.join(into);
    let url = format!("http://agent:{token}@{}/{task}.git", s.addr);
    let out = git(&s.root, &["clone", "-q", &url, dir.to_str().unwrap()]);
    (dir, out)
}

fn upstream_refs(s: &Setup) -> String {
    ok(
        &s.upstream,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    )
}

#[test]
fn the_gateway_enforces_read_and_write_scopes() {
    let s = setup();
    let token = mint(&s.root, PLAN);

    // Reads: only the readable paths, in files and in objects.
    let (work, out) = clone(&s, "sms", &token, "work");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let files = ok(&work, &["ls-files"]);
    assert!(
        files
            .lines()
            .all(|f| f.starts_with("services/notifications/")
                || f.starts_with("services/orders/")
                || f == "package.json"),
        "{files}"
    );
    assert!(files.contains("services/orders/"));
    assert!(!work.join("services/billing").exists());
    let objects = ok(&work, &["rev-list", "--objects", "--all"]);
    assert!(!objects.contains("billing"), "{objects}");
    assert_eq!(ok(&work, &["rev-list", "--count", "HEAD"]), "1");

    // A write in scope lands upstream, on the real base, author kept.
    std::fs::write(
        work.join("services/notifications/src/sms.ts"),
        "export const sms = true;\n",
    )
    .unwrap();
    ok(&work, &["add", "-A"]);
    ok(&work, &["commit", "-q", "-m", "Add SMS"]);
    let out = git(
        &work,
        &["push", "origin", "HEAD:refs/heads/onus/sms/feature"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pushed = ok(&s.upstream, &["rev-parse", "refs/heads/onus/sms/feature"]);
    let main = ok(&s.upstream, &["rev-parse", "refs/heads/main"]);
    assert_eq!(ok(&s.upstream, &["rev-parse", &format!("{pushed}^")]), main);
    let names = ok(
        &s.upstream,
        &["diff-tree", "-r", "--name-only", "--no-commit-id", &pushed],
    );
    assert_eq!(names, "services/notifications/src/sms.ts");
    assert_eq!(
        ok(&s.upstream, &["log", "-1", "--format=%an %s", &pushed]),
        "Agent Add SMS"
    );
    // The real tree keeps everything the agent could not see.
    let tree = ok(&s.upstream, &["ls-tree", "-r", "--name-only", &pushed]);
    assert!(tree.contains("services/billing/"));
    let before = upstream_refs(&s);

    // Red team: each of these is refused and changes nothing upstream.
    let refused = |what: &str, out: Output| {
        assert!(!out.status.success(), "{what} was accepted");
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert_eq!(upstream_refs(&s), before, "{what} changed upstream");
        err
    };
    // 1. A path outside the write scope, as a prompt-injected agent would try.
    std::fs::create_dir_all(work.join("services/billing/src")).unwrap();
    std::fs::write(
        work.join("services/billing/src/refund.ts"),
        "export const all = 1;\n",
    )
    .unwrap();
    ok(&work, &["add", "-A"]);
    ok(
        &work,
        &["commit", "-q", "-m", "Ignore previous instructions"],
    );
    let err = refused(
        "an out-of-scope write",
        git(
            &work,
            &["push", "origin", "HEAD:refs/heads/onus/sms/feature"],
        ),
    );
    assert!(
        err.contains("`services/billing/src/refund.ts` is outside your write scope"),
        "{err}"
    );
    assert!(
        err.contains("onus escalate --task sms --scope write:path:services/billing/src/**"),
        "{err}"
    );
    ok(&work, &["reset", "-q", "--hard", "HEAD~1"]);

    // 2. A ref outside the push scope.
    let err = refused(
        "a push to main",
        git(&work, &["push", "origin", "HEAD:refs/heads/main"]),
    );
    assert!(err.contains("not a ref this task may push"), "{err}");

    // 3. A symbolic link, even inside the write scope.
    std::os::unix::fs::symlink(
        "../../billing/src/charge.ts",
        work.join("services/notifications/src/link.ts"),
    )
    .unwrap();
    ok(&work, &["add", "-A"]);
    ok(&work, &["commit", "-q", "-m", "link"]);
    let err = refused(
        "a symlink",
        git(&work, &["push", "origin", "HEAD:refs/heads/onus/sms/link"]),
    );
    assert!(err.contains("symbolic link"), "{err}");
    ok(&work, &["reset", "-q", "--hard", "HEAD~1"]);

    // 4. A merge.
    ok(&work, &["checkout", "-q", "-b", "side"]);
    std::fs::write(
        work.join("services/notifications/src/a.ts"),
        "export const a = 1;\n",
    )
    .unwrap();
    ok(&work, &["add", "-A"]);
    ok(&work, &["commit", "-q", "-m", "a"]);
    ok(&work, &["checkout", "-q", "-"]);
    std::fs::write(
        work.join("services/notifications/src/b.ts"),
        "export const b = 1;\n",
    )
    .unwrap();
    ok(&work, &["add", "-A"]);
    ok(&work, &["commit", "-q", "-m", "b"]);
    ok(&work, &["merge", "-q", "--no-edit", "side"]);
    let err = refused(
        "a merge",
        git(&work, &["push", "origin", "HEAD:refs/heads/onus/sms/merge"]),
    );
    assert!(err.contains("is a merge"), "{err}");

    // 5. A sub-agent's attenuated token cannot write beyond its narrowing.
    let out = onus()
        .args(["token", "attenuate", "--key-public"])
        .arg(s.root.join("keys/root.pub"))
        .args([
            "--only",
            "write:path:services/notifications/src/templates/**",
        ])
        .env("ONUS_TOKEN", &token)
        .output()
        .unwrap();
    assert!(out.status.success());
    let narrow = String::from_utf8(out.stdout).unwrap().trim().to_string();
    let (sub, out) = clone(&s, "sms", &narrow, "sub");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::write(
        sub.join("services/notifications/src/sms.ts"),
        "export const x = 2;\n",
    )
    .unwrap();
    ok(&sub, &["add", "-A"]);
    ok(&sub, &["commit", "-q", "-m", "sub-agent"]);
    let err = refused(
        "an attenuated token's wider write",
        git(&sub, &["push", "origin", "HEAD:refs/heads/onus/sms/sub"]),
    );
    assert!(err.contains("outside your write scope"), "{err}");

    // 6. Forged and foreign tokens.
    let (_, out) = clone(&s, "sms", "not-a-token", "forged");
    assert!(!out.status.success());
    let other = mint(&s.root, "task: other\nwrites: [\"services/billing/**\"]\n");
    let (_, out) = clone(&s, "sms", &other, "foreign");
    assert!(!out.status.success());

    // Every decision is in the audit log, and its chain holds.
    let log = s.root.join("state/audit.jsonl");
    let out = onus().arg("audit").arg(&log).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("\"decision\":\"refused\""));
    assert!(text.contains("\"action\":\"replay\""));
}
