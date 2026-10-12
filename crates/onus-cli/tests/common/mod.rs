//! Helpers shared by the integration tests: the shop fixture, its
//! scenario overlays and an MCP client.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn repo_root() -> PathBuf {
    // Not canonicalized: on Windows that yields a `\\?\` path.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

pub fn fixture() -> PathBuf {
    repo_root().join("fixtures/shop")
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Files at the root of a scenario folder that are not part of the overlay.
pub const SCENARIO_META: &[&str] = &["README.md", "deleted.txt", "intent.yaml"];

/// Applies a scenario overlay (changed and added files, `deleted.txt`) to
/// the tree at `head`.
pub fn apply_overlay(scenario: &Path, head: &Path) {
    fn walk(dir: &Path, rel: &Path, head: &Path, top: bool) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if top && SCENARIO_META.contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            let rel = rel.join(&name);
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), &rel, head, false);
            } else {
                let target = head.join(&rel);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    walk(scenario, Path::new(""), head, true);
    if let Ok(list) = std::fs::read_to_string(scenario.join("deleted.txt")) {
        for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            std::fs::remove_file(onus_core::paths::native(head, line)).unwrap();
        }
    }
}

/// A base tree and the head tree of one scenario, in a temporary directory.
pub struct Scenario {
    pub dir: tempfile::TempDir,
    pub id: String,
}

impl Scenario {
    pub fn new(id: &str) -> Scenario {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixture().join("base"), &dir.path().join("base"));
        copy_dir(&fixture().join("base"), &dir.path().join("head"));
        apply_overlay(
            &fixture().join("scenarios").join(id),
            &dir.path().join("head"),
        );
        Scenario {
            dir,
            id: id.to_string(),
        }
    }

    pub fn base(&self) -> PathBuf {
        self.dir.path().join("base")
    }

    pub fn head(&self) -> PathBuf {
        self.dir.path().join("head")
    }

    pub fn intent_path(&self) -> Option<PathBuf> {
        let p = fixture()
            .join("scenarios")
            .join(&self.id)
            .join("intent.yaml");
        p.exists().then_some(p)
    }

    pub fn run(&self) -> onus_cli::Outcome {
        let intent = self
            .intent_path()
            .and_then(|p| onus_cli::read_intent(&p).unwrap());
        onus_cli::diff_dirs(
            &self.base(),
            &self.head(),
            &onus_cli::DiffOptions {
                intent,
                base_label: "base".into(),
                head_label: "head".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }
}

/// The environment variables that say how the CLI signs in to Onus Cloud.
pub const CLOUD_ENV: &[&str] = &[
    "ONUS_CLOUD_TOKEN",
    "ONUS_API_KEY",
    "ONUS_CLOUD_URL",
    "ONUS_CLOUD_ORG",
    "ONUS_CLOUD_REPO",
    "ONUS_CONFIG_DIR",
    "ONUS_CACHE_DIR",
    "ONUS_TOKEN",
];

/// `onus mcp` over stdio, driven as a client would.
pub struct Mcp {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    next: u64,
}

impl Mcp {
    pub fn start(repo: &Path) -> Mcp {
        Mcp::start_with(repo, &[])
    }

    pub fn start_with(repo: &Path, extra: &[&str]) -> Mcp {
        Mcp::start_env(repo, extra, &[])
    }

    /// With these environment variables, and none of Onus Cloud's others.
    pub fn start_env(repo: &Path, extra: &[&str], env: &[(&str, &str)]) -> Mcp {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_onus"));
        if !env.is_empty() {
            for k in CLOUD_ENV {
                cmd.env_remove(k);
            }
            cmd.envs(env.iter().copied());
        }
        let mut child = cmd
            .args(["mcp", "--no-server", "--repo"])
            .arg(repo)
            .args(extra)
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

    pub fn send(&mut self, msg: serde_json::Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    pub fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
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
    pub fn tool(&mut self, name: &str, args: serde_json::Value) -> (bool, serde_json::Value) {
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
