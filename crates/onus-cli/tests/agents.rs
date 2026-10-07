//! The map for agents: a live worktree session, `onus mcp` over stdio and
//! the shared map server.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{copy_dir, fixture};
use onus_cli::session::Session;
use onus_map::FactsCache;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Onus Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// A git repository holding the shop fixture, committed.
fn shop_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture().join("base"), dir.path());
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "base"]);
    dir
}

#[test]
fn a_session_follows_file_changes_and_reparses_only_them() {
    let repo = shop_repo();
    let cache = Arc::new(FactsCache::in_memory());
    let session = Session::new(repo.path(), cache.clone()).unwrap();

    let (first, meta) = session.index().unwrap();
    assert!(meta.rebuilt);
    let parsed = cache.stats().misses;
    let (_, meta) = session.index().unwrap();
    assert!(!meta.rebuilt, "no change, no rebuild");

    std::fs::write(
        repo.path().join("services/orders/src/probe.ts"),
        "export function onusProbe(): number {\n  return 1;\n}\n",
    )
    .unwrap();
    // The watcher (or, without one, the file check) notices the new file.
    let deadline = Instant::now() + Duration::from_secs(15);
    let (index, meta) = loop {
        let (index, meta) = session.index().unwrap();
        if !index.find("onusProbe", 5).symbols.is_empty() {
            break (index, meta);
        }
        assert!(Instant::now() < deadline, "the new file never showed up");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(meta.version >= 2);
    assert_eq!(index.status().symbols, first.status().symbols + 1);
    // Only the new file was parsed again.
    assert_eq!(cache.stats().misses, parsed + 1);
}

struct Mcp {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    next: u64,
}

impl Mcp {
    fn start(repo: &Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_onus"))
            .args(["mcp", "--no-server", "--repo"])
            .arg(repo)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Mcp {
            child,
            out,
            next: 1,
        };
        let init = mcp.call(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "onus-test", "version": "0" }
            }),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "onus");
        mcp.send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        mcp
    }

    fn send(&mut self, msg: serde_json::Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next;
        self.next += 1;
        self.send(
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        );
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    /// Calls a tool; returns whether it failed and its JSON (or message).
    fn tool(&mut self, name: &str, args: serde_json::Value) -> (bool, serde_json::Value) {
        let r = self.call(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": args }),
        );
        let result = &r["result"];
        let text = result["content"][0]["text"].as_str().unwrap().to_string();
        let failed = result["isError"].as_bool().unwrap_or(false);
        let body = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
        (failed, body)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_serves_the_map_tools_over_stdio() {
    let repo = shop_repo();
    let mut mcp = Mcp::start(repo.path());

    let tools = mcp.call("tools/list", serde_json::json!({}));
    let mut names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "onus_check",
            "onus_component",
            "onus_dependencies",
            "onus_dependents",
            "onus_file",
            "onus_find",
            "onus_owners",
            "onus_status",
            "onus_symbol",
            "onus_tests_for"
        ]
    );

    let (failed, status) = mcp.tool("onus_status", serde_json::json!({}));
    assert!(!failed);
    assert_eq!(status["result"]["components"], 7);
    assert_eq!(status["map"]["rebuilt"], true);

    let (_, found) = mcp.tool("onus_find", serde_json::json!({ "text": "apply discount" }));
    assert_eq!(found["result"]["symbols"][0]["name"], "applyDiscount");
    assert_eq!(found["map"]["rebuilt"], false);

    let (_, deps) = mcp.tool(
        "onus_dependents",
        serde_json::json!({ "target": "UserPreferences", "depth": 2 }),
    );
    assert!(
        deps["result"]["components"]["notifications"]
            .as_u64()
            .unwrap()
            > 0
    );

    let (failed, msg) = mcp.tool("onus_symbol", serde_json::json!({ "id": "nope" }));
    assert!(failed);
    assert!(msg.as_str().unwrap().contains("find"));

    // The codebase talks back: an uncommitted change checked against HEAD.
    std::fs::write(
        repo.path().join("services/billing/src/payments.ts"),
        std::fs::read_to_string(repo.path().join("services/billing/src/payments.ts")).unwrap()
            + "\nconst AWS_ACCESS_KEY_ID = \""
            + concat!("AKIA", "IOSFODNN7EXAMPLE")
            + "\";\n",
    )
    .unwrap();
    let (failed, check) = mcp.tool("onus_check", serde_json::json!({}));
    assert!(!failed, "{check}");
    assert_eq!(check["result"]["summary"]["secrets"], 1);
    assert!(
        check["result"]["markdown"]
            .as_str()
            .unwrap()
            .contains("committed")
    );
}

#[cfg(unix)]
#[test]
fn agents_share_one_map_server_per_repository() {
    let repo = shop_repo();
    let runtime = tempfile::tempdir().unwrap();
    let query = |args: &[&str]| -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_onus"))
            .arg("query")
            .args(args)
            .arg("--repo")
            .arg(repo.path())
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("ONUS_SERVER_IDLE", "5")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let first = query(&["status"]);
    assert_eq!(first["map"]["rebuilt"], true);
    // A second agent (another process) gets the same map without a rebuild.
    let second = query(&["find", "order"]);
    assert_eq!(second["map"]["rebuilt"], false);
    assert_eq!(second["map"]["version"], first["map"]["version"]);
    let stats = query(&["stats"]);
    let worktrees = stats["result"]["worktrees"].as_array().unwrap();
    assert_eq!(worktrees.len(), 1);
    let pid = stats["result"]["pid"].as_u64().unwrap();
    assert_eq!(query(&["stats"])["result"]["pid"].as_u64().unwrap(), pid);
}
