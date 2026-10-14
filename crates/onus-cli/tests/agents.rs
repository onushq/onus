//! The map for agents: a live worktree session, `onus mcp` over stdio and
//! the shared map server.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Mcp, apply_overlay, copy_dir, fixture};
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
            "onus_blocked",
            "onus_check",
            "onus_component",
            "onus_dependencies",
            "onus_dependents",
            "onus_env_create",
            "onus_env_destroy",
            "onus_env_run",
            "onus_envs",
            "onus_escalate",
            "onus_escalation",
            "onus_evidence",
            "onus_file",
            "onus_find",
            "onus_impact",
            "onus_invariants",
            "onus_judge",
            "onus_lanes",
            "onus_outcomes",
            "onus_owners",
            "onus_run_test",
            "onus_status",
            "onus_submit",
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

    // Each test says how its link is known; without coverage, from the syntax.
    let (failed, tests) = mcp.tool(
        "onus_tests_for",
        serde_json::json!({ "target": "applyDiscount" }),
    );
    assert!(!failed, "{tests}");
    assert_eq!(
        tests["result"]["tests"][0]["file"],
        "services/billing/src/discount.test.ts"
    );
    assert_eq!(tests["result"]["tests"][0]["link"], "static");
    assert!(tests["result"].get("coverage").is_none());

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

/// One HTTP/1.1 POST, read to the end: the status, the headers and the
/// JSON-RPC messages of the body (plain JSON or `data:` lines of an event
/// stream, possibly chunked).
fn http_post(
    addr: &str,
    host: &str,
    session: Option<&str>,
    body: &serde_json::Value,
) -> (u16, String, Vec<serde_json::Value>) {
    use std::io::Read;
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    let body = body.to_string();
    let mut req = format!(
        "POST / HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(s) = session {
        req.push_str(&format!("Mcp-Session-Id: {s}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(&body);
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    let _ = stream.read_to_string(&mut raw);
    let (head, rest) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let messages = rest
        .lines()
        .filter_map(|l| l.trim().strip_prefix("data:"))
        .chain(rest.lines().filter(|l| l.trim_start().starts_with('{')))
        .filter_map(|d| serde_json::from_str(d.trim()).ok())
        .collect();
    (status, head.to_string(), messages)
}

#[test]
fn mcp_serves_the_same_tools_over_http() {
    let repo = shop_repo();
    let mut child = Command::new(env!("CARGO_BIN_EXE_onus"))
        .args(["mcp", "--no-server", "--http", "127.0.0.1:0", "--repo"])
        .arg(repo.path())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let addr = line
        .trim()
        .rsplit("http://")
        .next()
        .unwrap()
        .trim_end_matches('/')
        .to_string();

    let (status, head, msgs) = http_post(
        &addr,
        "127.0.0.1",
        None,
        &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "onus-test", "version": "1"}}}),
    );
    assert_eq!(status, 200, "{head}");
    assert_eq!(msgs[0]["result"]["serverInfo"]["name"], "onus");
    let session = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("mcp-session-id")
                .then(|| v.trim().to_string())
        })
        .unwrap();
    http_post(
        &addr,
        "127.0.0.1",
        Some(&session),
        &serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    let (_, _, msgs) = http_post(
        &addr,
        "127.0.0.1",
        Some(&session),
        &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "onus_impact", "arguments": {"target": "applyDiscount", "change": "remove"}}}),
    );
    let text = msgs[0]["result"]["content"][0]["text"].as_str().unwrap();
    let answer: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(answer["result"]["total"], 2, "{answer}");

    // Agent-agnostic: every client gets the same answers, whatever the
    // transport. The same calls over stdio return the same results.
    let mut stdio = Mcp::start(repo.path());
    for (i, (tool, args)) in [
        (
            "onus_impact",
            serde_json::json!({"target": "applyDiscount", "change": "remove"}),
        ),
        ("onus_find", serde_json::json!({"text": "apply discount"})),
        (
            "onus_dependents",
            serde_json::json!({"target": "UserPreferences", "depth": 2}),
        ),
        ("onus_invariants", serde_json::json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let (_, _, msgs) = http_post(
            &addr,
            "127.0.0.1",
            Some(&session),
            &serde_json::json!({"jsonrpc": "2.0", "id": 10 + i, "method": "tools/call",
                "params": {"name": tool, "arguments": args}}),
        );
        let text = msgs[0]["result"]["content"][0]["text"].as_str().unwrap();
        let over_http: serde_json::Value = serde_json::from_str(text).unwrap();
        let (_, over_stdio) = stdio.tool(tool, args);
        assert_eq!(over_http["result"], over_stdio["result"], "{tool}");
    }

    // Another host name (DNS rebinding) is refused.
    let (status, _, _) = http_post(
        &addr,
        "evil.example",
        None,
        &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    );
    assert!(status >= 400, "{status}");
    child.kill().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_with_a_task_token_answers_only_within_its_read_scope() {
    let repo = shop_repo();
    let keys = tempfile::tempdir().unwrap();
    let onus = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_onus"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    onus(&["token", "keygen", "--out", keys.path().to_str().unwrap()]);
    let plan = keys.path().join("plan.yaml");
    std::fs::write(
        &plan,
        "task: billing-fix\nwrites: [\"services/billing/**\"]\n",
    )
    .unwrap();
    let token = onus(&[
        "token",
        "mint",
        "--plan",
        plan.to_str().unwrap(),
        "--key",
        keys.path().join("root.key").to_str().unwrap(),
    ]);
    let public = keys.path().join("root.pub");
    let mut mcp = Mcp::start_with(
        repo.path(),
        &["--token", &token, "--key-public", public.to_str().unwrap()],
    );
    // A symbol in another component is not described at all.
    let (failed, body) = mcp.tool(
        "onus_symbol",
        serde_json::json!({ "id": "user-preferences:src/types.ts#UserPreferences" }),
    );
    assert!(failed, "{body}");
    assert!(body.as_str().unwrap().contains("outside the read scope"));
    assert!(body.as_str().unwrap().contains("onus_escalate"), "{body}");
    // Search answers keep only readable files.
    let (failed, found) = mcp.tool("onus_find", serde_json::json!({ "text": "preferences" }));
    assert!(!failed);
    let text = found.to_string();
    assert!(!text.contains("services/user-preferences/"), "{text}");
    let (_, discount) = mcp.tool("onus_find", serde_json::json!({ "text": "apply discount" }));
    assert!(
        discount
            .to_string()
            .contains("services/billing/src/discount.ts")
    );
}

#[test]
fn agents_hand_in_changes_and_ask_for_more_through_mcp() {
    let repo = shop_repo();
    let r = repo.path();
    // A change on a branch, committed as an agent would.
    git(r, &["checkout", "-q", "-b", "feature/sms"]);
    apply_overlay(&fixture().join("scenarios/s1-sms-alerts"), r);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "sms"]);
    let keys = tempfile::tempdir().unwrap();
    let onus = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_onus"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    onus(&["token", "keygen", "--out", keys.path().to_str().unwrap()]);
    let plan = keys.path().join("plan.yaml");
    std::fs::write(
        &plan,
        "task: sms-alerts\nwrites: [\"services/notifications/**\"]\nagent: {tool: claude-code, model: opus}\n",
    )
    .unwrap();
    let key = keys.path().join("root.key");
    let token = onus(&[
        "token",
        "mint",
        "--plan",
        plan.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
    ]);
    let public = keys.path().join("root.pub");
    let mut mcp = Mcp::start_with(
        r,
        &["--token", &token, "--key-public", public.to_str().unwrap()],
    );

    let (failed, policy) = mcp.tool("onus_lanes", serde_json::json!({}));
    assert!(!failed, "{policy}");
    assert_eq!(policy["configured"], false);

    // Submitting: the report, the lane and why, under the token's scope.
    let (failed, sub) = mcp.tool(
        "onus_submit",
        serde_json::json!({
            "base": "main",
            "head": "feature/sms",
            "intent": "summary: Text customers when their order ships\ntouches: [notifications]",
            "agent_tool": "codex",
        }),
    );
    assert!(!failed, "{sub}");
    // Who did it comes from the token, not from what the agent says.
    assert_eq!(sub["agent"]["tool"], "claude-code", "{sub}");
    assert!(
        sub["agentNote"]
            .as_str()
            .unwrap()
            .contains("comes from your token"),
        "{sub}"
    );
    // The change also edits user-preferences, outside the token's scope: the
    // scope the server attached, not one the agent chose, blocks it.
    assert_eq!(sub["lane"], "blocked", "{sub}");
    assert!(
        sub["why"].to_string().contains("outside its token's scope"),
        "{sub}"
    );
    assert_eq!(sub["scope"]["task"], "sms-alerts");
    let id = sub["id"].as_str().unwrap();
    let stored: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.join(format!(".onus/submissions/{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["agent"]["tool"], "claude-code");
    assert_eq!(stored["agent"]["model"], "opus");
    assert!(sub["rows"].as_array().unwrap().len() >= 3);

    // Asking for more: filed as the token's task, whatever the agent says.
    let (failed, esc) = mcp.tool(
        "onus_escalate",
        serde_json::json!({
            "scopes": ["write:path:services/orders/src/**"],
            "evidence": ["rationale:the shipped event must carry the phone number"],
            "reason": "the event needs the phone",
            "task": "someone-else",
        }),
    );
    assert!(!failed, "{esc}");
    assert_eq!(esc["task"], "sms-alerts");
    let eid = esc["id"].as_str().unwrap().to_string();
    let (_, state) = mcp.tool("onus_escalation", serde_json::json!({ "id": eid }));
    assert_eq!(state["state"], "open", "{state}");
    let submit = |mcp: &mut Mcp, ids: &[&str]| {
        mcp.tool(
            "onus_submit",
            serde_json::json!({
                "base": "main",
                "head": "feature/sms",
                "intent": "summary: Text customers when their order ships\ntouches: [notifications, orders]",
                "escalations": ids,
            }),
        )
    };
    // An escalation not granted yet, or one that does not exist, is refused.
    let (failed, msg) = submit(&mut mcp, &[&eid]);
    assert!(failed, "{msg}");
    let msg = msg.as_str().unwrap();
    assert!(
        msg.contains("is still open") || msg.contains("is waiting for a person"),
        "{msg}"
    );
    let (failed, msg) = submit(&mut mcp, &["deadbeef0000"]);
    assert!(failed);
    assert!(msg.as_str().unwrap().contains("no escalation"), "{msg}");

    // A person grants it from the command line, logging it in their own log
    // rather than the server's: the kept token proves the grant by itself.
    // The agent collects it.
    let request = r.join(format!(".onus/escalations/{eid}.json"));
    onus(&[
        "escalation",
        "grant",
        request.to_str().unwrap(),
        "--token",
        &token,
        "--key-public",
        public.to_str().unwrap(),
        "--key",
        key.to_str().unwrap(),
        "--by",
        "@lead",
        "--audit",
        keys.path().join("person.jsonl").to_str().unwrap(),
    ]);
    let (_, state) = mcp.tool("onus_escalation", serde_json::json!({ "id": eid }));
    assert_eq!(state["state"], "granted", "{state}");
    // The server uses the granted token from now on.
    assert_eq!(state["adopted"], true, "{state}");
    let granted = state["token"].as_str().unwrap();
    let check = Command::new(env!("CARGO_BIN_EXE_onus"))
        .args([
            "token",
            "check",
            "--token",
            granted,
            "--key-public",
            public.to_str().unwrap(),
            "write:path:services/orders/src/ship.ts",
        ])
        .output()
        .unwrap();
    assert!(check.status.success());

    // Submitting with the granted escalation: verified, listed, and the
    // scope is the granted token's.
    let (failed, sub) = submit(&mut mcp, &[&eid]);
    assert!(!failed, "{sub}");
    assert_eq!(sub["escalations"], serde_json::json!([eid]), "{sub}");
    let id = sub["id"].as_str().unwrap();
    let stored: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.join(format!(".onus/submissions/{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["escalations"], serde_json::json!([eid]));
    assert!(
        stored["scope"]["rights"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("write:path:services/orders/src/**")),
        "{stored}"
    );

    // Outcomes are for reading only; nothing lets an agent record its own.
    let (failed, outcomes) = mcp.tool("onus_outcomes", serde_json::json!({}));
    assert!(!failed, "{outcomes}");

    // Without actions, only questions.
    drop(mcp);
    let mut quiet = Mcp::start_with(r, &["--no-actions"]);
    let (failed, msg) = quiet.tool("onus_lanes", serde_json::json!({}));
    assert!(failed);
    assert!(msg.as_str().unwrap().contains("only answers questions"));
}

#[test]
fn the_judge_never_shows_an_agent_its_held_out_checks() {
    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    copy_dir(&fixture().join("base"), r);
    let mut yaml = std::fs::read_to_string(r.join("onus.yaml")).unwrap();
    yaml.push_str(
        "\nlanes:\n  default: judge\n  heldOut: { command: \"npm run secret-legacy\" }\n  judges:\n    - name: a\n    - name: b\n",
    );
    std::fs::write(r.join("onus.yaml"), yaml).unwrap();
    git(r, &["init", "-q", "-b", "main"]);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "base"]);
    git(r, &["checkout", "-q", "-b", "feature/extra"]);
    std::fs::write(
        r.join("services/orders/src/extra.ts"),
        "export const EXTRA = 1;\n",
    )
    .unwrap();
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "extra"]);
    let suite = tempfile::tempdir().unwrap();
    std::fs::write(
        suite.path().join("held-out.yaml"),
        "version: 1\nchecks:\n  - { id: orders-secret-check, command: \"npm run hidden-thing\", covers: [orders] }\n",
    )
    .unwrap();
    let mut mcp = Mcp::start_with(r, &["--held-out", suite.path().to_str().unwrap()]);

    // The policy says there are held-out checks, never what they run.
    let (failed, policy) = mcp.tool("onus_lanes", serde_json::json!({}));
    assert!(!failed, "{policy}");
    assert_eq!(policy["policy"]["heldOut"], "configured", "{policy}");
    assert!(policy.get("heldOutSuite").is_none(), "{policy}");
    assert!(!policy.to_string().contains("secret-legacy"), "{policy}");
    assert_eq!(policy["judges"][0]["name"], "a", "{policy}");

    let (failed, sub) = mcp.tool(
        "onus_submit",
        serde_json::json!({
            "base": "main",
            "head": "feature/extra",
            "intent": "summary: An extra constant\ntouches: [orders]",
        }),
    );
    assert!(!failed, "{sub}");
    let id = sub["id"].as_str().unwrap().to_string();
    // Without evidence the judge rejects first: no container engine needed.
    let (failed, judged) = mcp.tool("onus_judge", serde_json::json!({ "id": id }));
    assert!(!failed, "{judged}");
    assert_eq!(judged["judgment"]["verdict"], "reject", "{judged}");
    assert!(judged.get("heldOut").is_none(), "{judged}");
    assert_eq!(judged["judgment"]["heldOut"]["selected"], 2, "{judged}");
    let text = judged.to_string();
    for secret in ["orders-secret-check", "hidden-thing", "secret-legacy"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    // Nor which configuration judged: the policy's id, the same for each.
    assert!(judged["judgment"].get("rotation").is_none(), "{judged}");
    assert_eq!(judged["judgment"]["judge"], policy["judge"], "{judged}");
    assert!(!text.contains("a@") && !text.contains("b@"), "{text}");
    // A person reads what was chosen, and by which, beside the judgment.
    let kept =
        std::fs::read_to_string(r.join(format!(".onus/submissions/{id}.held-out.json"))).unwrap();
    assert!(kept.contains("orders-secret-check"), "{kept}");
    let kept: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.join(format!(".onus/submissions/{id}.judgment.json"))).unwrap(),
    )
    .unwrap();
    let config = kept["judgment"]["rotation"]["config"].as_str().unwrap();
    assert!(
        kept["judgment"]["judge"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{config}@")),
        "{kept}"
    );

    // A suite inside the worktree refuses to start.
    let inside = r.join("held-out");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::copy(
        suite.path().join("held-out.yaml"),
        inside.join("held-out.yaml"),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .current_dir(r)
        .args(["mcp", "--held-out", inside.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside the repository"));
}

/// A key pair and a token for `plan`, minted with the binary.
fn task_token(plan: &str) -> (tempfile::TempDir, String, std::path::PathBuf) {
    let keys = tempfile::tempdir().unwrap();
    let onus = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_onus"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    onus(&["token", "keygen", "--out", keys.path().to_str().unwrap()]);
    let file = keys.path().join("plan.yaml");
    std::fs::write(&file, plan).unwrap();
    let token = onus(&[
        "token",
        "mint",
        "--plan",
        file.to_str().unwrap(),
        "--key",
        keys.path().join("root.key").to_str().unwrap(),
    ]);
    let public = keys.path().join("root.pub");
    (keys, token, public)
}

#[test]
fn every_mcp_call_is_logged_and_checked() {
    let repo = shop_repo();
    let (keys, token, public) = task_token(
        "task: billing-fix\nwrites: [\"services/billing/**\"]\nhosts: [api.stripe.test]\n",
    );
    // The log is outside the worktree, where the agent cannot write.
    let audit = keys.path().join("audit.jsonl");
    let mut mcp = Mcp::start_with(
        repo.path(),
        &[
            "--token",
            &token,
            "--key-public",
            public.to_str().unwrap(),
            "--audit",
            audit.to_str().unwrap(),
        ],
    );
    let (failed, _) = mcp.tool("onus_find", serde_json::json!({ "text": "apply discount" }));
    assert!(!failed);
    // A file outside the read scope: the denial names the right and how to ask.
    let (failed, body) = mcp.tool(
        "onus_symbol",
        serde_json::json!({ "id": "user-preferences:src/types.ts#UserPreferences" }),
    );
    assert!(failed);
    let text = body.as_str().unwrap();
    assert!(text.starts_with("denied: "), "{text}");
    assert!(
        text.contains("read:path:services/user-preferences/src/**"),
        "{text}"
    );
    // A short name's candidates leave out those in unreadable files.
    let (failed, body) = mcp.tool("onus_symbol", serde_json::json!({ "id": "refundNote" }));
    assert!(!failed, "{body}");
    assert!(!body.to_string().contains("orders:"), "{body}");
    assert!(body["hidden"].is_string(), "{body}");
    // A test setup would get open network; the token allows one host.
    let (failed, body) = mcp.tool(
        "onus_run_test",
        serde_json::json!({ "image": "node:22", "setup": "npm ci", "command": "npm test" }),
    );
    assert!(failed);
    let text = body.as_str().unwrap();
    assert!(text.contains("open network"), "{text}");
    assert!(text.contains("net:host:api.stripe.test"), "{text}");
    // An environment that does not exist: nothing to protect.
    mcp.tool("onus_env_destroy", serde_json::json!({ "name": "nobody" }));
    drop(mcp);

    let out = Command::new(env!("CARGO_BIN_EXE_onus"))
        .arg("audit")
        .arg(&audit)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let raw = std::fs::read_to_string(&audit).unwrap();
    assert!(!raw.contains(&token), "the raw token reached the log");
    let entries: Vec<serde_json::Value> = raw
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let find = |action: &str, decision: &str| {
        entries
            .iter()
            .find(|e| e["action"] == action && e["decision"] == decision)
            .unwrap_or_else(|| panic!("no {action} {decision} in {raw}"))
    };
    let found = find("mcp:onus_find", "allowed");
    assert_eq!(found["actor"], "billing-fix");
    assert_eq!(found["subject"], "apply discount");
    assert_eq!(found["details"]["door"], "mcp");
    assert_eq!(found["details"]["token"]["task"], "billing-fix");
    assert_eq!(found["details"]["token"]["id"].as_str().unwrap().len(), 16);
    assert!(
        found["details"]["argsDigest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let symbol = find("mcp:onus_symbol", "denied");
    assert_eq!(
        symbol["details"]["missing"],
        serde_json::json!(["read:path:services/user-preferences/src/types.ts"])
    );
    find("mcp:onus_run_test", "denied");
    find("mcp:onus_env_destroy", "allowed");
    // One entry per question, two per action.
    let destroys = entries
        .iter()
        .filter(|e| e["action"] == "mcp:onus_env_destroy")
        .count();
    assert_eq!(destroys, 2, "{raw}");
}

#[test]
fn an_expired_token_is_refused_with_its_expiry() {
    let repo = shop_repo();
    let (keys, token, public) =
        task_token("task: quick\nwrites: [\"services/billing/**\"]\nttl: 1s\n");
    let deadline = Instant::now() + Duration::from_millis(2100);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let audit = keys.path().join("audit.jsonl");
    let mut mcp = Mcp::start_with(
        repo.path(),
        &[
            "--token",
            &token,
            "--key-public",
            public.to_str().unwrap(),
            "--audit",
            audit.to_str().unwrap(),
        ],
    );
    let (failed, msg) = mcp.tool("onus_lanes", serde_json::json!({}));
    assert!(failed);
    let msg = msg.as_str().unwrap();
    assert!(
        msg.contains("the token of task `quick` expired at "),
        "{msg}"
    );
    assert!(msg.contains("onus token mint"), "{msg}");
    let (failed, _) = mcp.tool("onus_status", serde_json::json!({}));
    assert!(failed);
    drop(mcp);
    let raw = std::fs::read_to_string(&audit).unwrap();
    assert!(raw.contains("\"decision\":\"denied\""), "{raw}");
}
