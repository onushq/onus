//! `onus ui`: the server answers only loopback hosts and only with the
//! session token, and its API runs the same code as the commands.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use common::{apply_overlay, copy_dir, fixture};

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

/// Stops the server when the test ends, passing or not.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(repo: &Path, keys: &Path) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_onus"))
        .args(["ui", "--port", "0", "--no-open", "--keys"])
        .arg(keys)
        .arg("--repo")
        .arg(repo)
        .env("ONUS_UI_TOKEN", "test-token")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    // http://127.0.0.1:<port>/#token=test-token
    let port = line
        .trim()
        .strip_prefix("http://127.0.0.1:")
        .and_then(|r| r.split('/').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no address in {line:?}"));
    assert!(line.trim().ends_with("#token=test-token"), "{line}");
    Server { child, port }
}

/// One HTTP/1.1 request; returns the status and the body.
fn http(
    port: u16,
    method: &str,
    path: &str,
    host: &str,
    token: Option<&str>,
    body: &str,
) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(t) = token {
        req.push_str(&format!("x-onus-token: {t}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text[9..12].parse().unwrap();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap();
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(rest)
    } else {
        rest.to_string()
    };
    (status, body)
}

fn dechunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = s.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&rest[..n]);
        s = &rest[n + 2..];
    }
    out
}

fn api(server: &Server, name: &str, body: serde_json::Value) -> serde_json::Value {
    let host = format!("127.0.0.1:{}", server.port);
    let (status, text) = http(
        server.port,
        "POST",
        &format!("/api/{name}"),
        &host,
        Some("test-token"),
        &body.to_string(),
    );
    let v: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    assert_eq!(status, 200, "{name}: {v}");
    v["ok"].clone()
}

#[test]
fn the_interface_serves_the_repository_to_its_session_only() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("shop");
    copy_dir(&fixture().join("base"), &repo);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["checkout", "-q", "-b", "feature/sms"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), &repo);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "sms"]);
    git(&repo, &["checkout", "-q", "main"]);
    let keys = dir.path().join("keys");
    let server = start(&repo, &keys);
    let port = server.port;
    let host = format!("127.0.0.1:{port}");

    // Another host name (DNS rebinding), no token, a wrong token, GET.
    let (status, _) = http(
        port,
        "POST",
        "/api/status",
        &format!("attacker.example:{port}"),
        Some("test-token"),
        "{}",
    );
    assert_eq!(status, 421);
    let (status, body) = http(port, "POST", "/api/status", &host, None, "{}");
    assert_eq!(status, 401, "{body}");
    let (status, _) = http(port, "POST", "/api/status", &host, Some("test-tokem"), "{}");
    assert_eq!(status, 401);
    let (status, _) = http(port, "GET", "/api/status", &host, Some("test-token"), "");
    assert_eq!(status, 405);
    let (status, _) = http(
        port,
        "POST",
        "/api/nothing",
        &host,
        Some("test-token"),
        "{}",
    );
    assert_eq!(status, 404);

    // The page itself needs no token: the built interface, or the page that
    // says how to build it.
    let (status, page) = http(port, "GET", "/map/component/orders", &host, None, "");
    assert_eq!(status, 200);
    assert!(
        page.contains("<html") || page.contains("<!doctype html>"),
        "{page}"
    );
    if option_env!("ONUS_REQUIRE_UI").is_some() {
        // Built with the interface: the app and its assets are served.
        let asset = page
            .split("\"/_app/")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap_or_else(|| panic!("no app in {page}"));
        let (status, _) = http(port, "GET", &format!("/_app/{asset}"), &host, None, "");
        assert_eq!(status, 200, "/_app/{asset}");
    }

    // The map.
    let status = api(&server, "status", serde_json::json!({}));
    assert_eq!(status["map"]["components"], 7, "{status}");
    assert_eq!(status["branch"], "main");
    let found = api(
        &server,
        "map.query",
        serde_json::json!({ "query": { "query": "find", "text": "place order" } }),
    );
    assert!(
        found["symbols"].to_string().contains("placeOrder"),
        "{found}"
    );
    let graph = api(&server, "map.graph", serde_json::json!({}));
    assert!(
        graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["from"] == "orders" && e["to"] == "events"),
        "{graph}"
    );
    let file = api(
        &server,
        "files.read",
        serde_json::json!({ "path": "services/orders/src/orders.ts" }),
    );
    assert!(file["text"].as_str().unwrap().contains("placeOrder"));
    let host_header = format!("127.0.0.1:{port}");
    let (status, _) = http(
        port,
        "POST",
        "/api/files.read",
        &host_header,
        Some("test-token"),
        r#"{"path":"../shop/onus.yaml"}"#,
    );
    assert_eq!(status, 400, "paths outside the repository are refused");

    // A report between refs, with its lane.
    let report = api(
        &server,
        "report",
        serde_json::json!({ "base": "main", "head": "feature/sms" }),
    );
    assert_eq!(
        report["report"]["summary"]["meaningChanges"], 4,
        "{}",
        report["report"]["summary"]
    );
    assert_eq!(report["classification"]["lane"], "human");
    assert!(report["markdown"].as_str().unwrap().contains("Acme SMS"));

    // Uncommitted work, as onus_check sees it.
    std::fs::write(
        repo.join("services/orders/src/extra.ts"),
        "export function extra(): number { return 1; }\n",
    )
    .unwrap();
    let check = api(&server, "check", serde_json::json!({}));
    assert!(
        check["report"]["changes"].to_string().contains("extra"),
        "{check}"
    );

    // Keys, a token, and its scope.
    api(&server, "keys.generate", serde_json::json!({}));
    let minted = api(
        &server,
        "token.mint",
        serde_json::json!({ "plan": "task: sms\nwrites: [\"services/notifications/**\"]\n" }),
    );
    let token = minted["token"].as_str().unwrap();
    let check = api(
        &server,
        "token.check",
        serde_json::json!({ "token": token, "right": "write:path:services/billing/a.ts" }),
    );
    assert_eq!(check["allowed"], false);
    let audit = api(&server, "audit", serde_json::json!({}));
    assert_eq!(audit["intact"], true);
    assert_eq!(audit["entries"][0]["action"], "mint");

    // Outcomes go to .onus/ in the repository.
    let out = api(
        &server,
        "outcomes.record",
        serde_json::json!({ "change": "shop#1", "agent": "a/b/c", "lane": "judge", "result": "merged" }),
    );
    assert_eq!(out["summary"]["total"]["merged"], 1);
    assert!(repo.join(".onus/outcomes.jsonl").exists());

    // The guide ships with it.
    let topic = api(&server, "guide", serde_json::json!({ "name": "lanes" }));
    assert!(topic["text"].as_str().unwrap().starts_with("# Risk lanes"));
}
