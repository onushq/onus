//! The CLI's doors into Onus Cloud, against a fake API that records every request:
//! scoped tokens (minted from a login, never sending the login on), escalations and
//! blocked reports filed in Onus Cloud and minted by the door, and questions to the maps
//! Onus Cloud keeps, answered here.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use common::{CLOUD_ENV, Mcp, copy_dir, fixture};
use serde_json::{Value, json};

/// One request the fake API received.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    auth: String,
    body: Value,
    raw: String,
}

#[derive(Default)]
struct World {
    seen: Vec<Seen>,
    minted: u32,
    /// Seconds a minted token lasts (default: until 2099).
    mint_ttl: Option<u64>,
    /// Escalations by id, as Onus Cloud shows them.
    escalations: BTreeMap<String, Value>,
    /// Map JSON by commit.
    maps: BTreeMap<String, Vec<u8>>,
}

/// A fake Onus Cloud API on 127.0.0.1, one thread, a connection per request.
struct Fake {
    url: String,
    world: Arc<Mutex<World>>,
}

impl Fake {
    fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let world = Arc::new(Mutex::new(World::default()));
        let w = world.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let w = w.clone();
                std::thread::spawn(move || serve(stream, &w));
            }
        });
        Fake { url, world }
    }

    fn seen(&self) -> Vec<Seen> {
        self.world.lock().unwrap().seen.clone()
    }

    fn requests(&self, method: &str, prefix: &str) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|s| s.method == method && s.path.starts_with(prefix))
            .collect()
    }

    /// Changes what Onus Cloud says about an escalation.
    fn set(&self, id: &str, patch: Value) {
        let mut w = self.world.lock().unwrap();
        let e = w.escalations.get_mut(id).expect("no such escalation");
        for (k, v) in patch.as_object().unwrap() {
            e[k] = v.clone();
        }
    }

    fn serve_map(&self, commit: &str, json: Vec<u8>) {
        self.world
            .lock()
            .unwrap()
            .maps
            .insert(commit.to_string(), json);
    }
}

fn serve(stream: TcpStream, world: &Mutex<World>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let mut body = Vec::new();
    if let Some(n) = headers
        .get("content-length")
        .and_then(|n| n.parse::<usize>().ok())
    {
        body.resize(n, 0);
        reader.read_exact(&mut body).unwrap();
    } else if headers.get("transfer-encoding").map(String::as_str) == Some("chunked") {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).unwrap();
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            let mut chunk = vec![0; n + 2];
            reader.read_exact(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
    }
    let raw = String::from_utf8_lossy(&body).to_string();
    let seen = Seen {
        method: method.clone(),
        path: path.clone(),
        auth: headers.get("authorization").cloned().unwrap_or_default(),
        body: serde_json::from_str(&raw).unwrap_or(Value::Null),
        raw,
    };
    let (status, extra, out) = answer(world, &seen);
    world.lock().unwrap().seen.push(seen);
    let mut stream = stream;
    let mut head = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
        out.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&out);
    let _ = stream.flush();
}

/// `at` (seconds since the epoch) as Onus Cloud writes times.
fn utc(at: u64) -> String {
    let (days, secs) = (at / 86_400, at % 86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

fn ok(v: Value) -> (u16, Vec<(String, String)>, Vec<u8>) {
    (200, vec![], v.to_string().into_bytes())
}

fn answer(world: &Mutex<World>, req: &Seen) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut w = world.lock().unwrap();
    let (path, query) = req.path.split_once('?').unwrap_or((&req.path, ""));
    match (req.method.as_str(), path) {
        ("GET", "/v1/whoami") => {
            let mut me = json!({
                "kind": "user", "user": { "name": "Ada", "email": "ada@example.com" },
                "organisations": [{ "slug": "acme" }],
            });
            if req.auth.starts_with("Bearer onus_tok_") {
                me["token"] = json!({
                    "id": "tok-given", "kind": "scoped", "expiresAt": "2099-01-01T00:00:00.000Z",
                    "organisation": { "slug": "acme" }, "repositories": ["acme/shop"],
                    "scopes": ["pulls:read"], "label": "sms",
                });
            }
            ok(me)
        }
        // A token without env:read is turned away, and taught.
        ("GET", "/v1/orgs/acme/environments") if req.auth == "Bearer onus_tok_narrow" => (
            403,
            vec![],
            json!({
                "error": "insufficient_scope", "scope": "env:read",
                "message": "this token cannot read environments (env:read)",
            })
            .to_string()
            .into_bytes(),
        ),
        ("GET", "/v1/orgs/acme/environments") => ok(json!({
            "environments": [], "allowance": { "running": 0, "limit": 2 },
        })),
        ("POST", "/v1/orgs/acme/tokens") => {
            w.minted += 1;
            let n = w.minted;
            let expires = match w.mint_ttl {
                Some(ttl) => {
                    let at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        + ttl;
                    utc(at)
                }
                None => "2099-01-01T00:00:00.000Z".into(),
            };
            let b = &req.body;
            (
                201,
                vec![],
                json!({
                    "token": format!("onus_tok_{n}"), "tokenId": format!("tok-{n}"), "kind": "scoped",
                    "organisation": { "id": "o1", "slug": "acme", "name": "Acme" },
                    "repositories": b["repositories"], "scopes": b["scopes"],
                    "expiresAt": expires, "label": b["label"],
                })
                .to_string()
                .into_bytes(),
            )
        }
        ("GET", "/v1/orgs/acme/pulls") => ok(json!({ "pulls": [] })),
        ("POST", "/v1/orgs/acme/escalations") => {
            let id = format!("esc-{}", w.escalations.len() + 1);
            let e = json!({
                "id": id, "state": "waiting", "kind": req.body["kind"],
                "clientRequestId": req.body["clientRequestId"],
                "repository": req.body["repository"], "scopes": req.body["scopes"],
                "webUrl": format!("http://app.test/o/acme/escalations/{id}"),
                "owners": ["@team-billing"], "decision": null,
                "next": "A person decides (@team-billing).",
            });
            w.escalations.insert(id, e.clone());
            (
                202,
                vec![],
                json!({ "escalation": e }).to_string().into_bytes(),
            )
        }
        ("POST", p) if p.starts_with("/v1/orgs/acme/escalations/") && p.ends_with("/withdraw") => {
            let id = p
                .trim_start_matches("/v1/orgs/acme/escalations/")
                .trim_end_matches("/withdraw");
            match w.escalations.get_mut(id) {
                Some(e) => {
                    e["state"] = json!("withdrawn");
                    ok(json!({ "escalation": e }))
                }
                None => (404, vec![], b"{\"message\":\"no escalation\"}".to_vec()),
            }
        }
        ("GET", p) if p.starts_with("/v1/orgs/acme/escalations/") => {
            let id = p.trim_start_matches("/v1/orgs/acme/escalations/");
            match w.escalations.get(id) {
                Some(e) => ok(json!({ "escalation": e, "mayDecide": false })),
                None => (404, vec![], b"{\"message\":\"no escalation\"}".to_vec()),
            }
        }
        ("GET", "/v1/orgs/acme/repos/acme%2Fshop/map") => {
            let commit = query.strip_prefix("commit=").unwrap_or_default();
            match w.maps.get(commit) {
                Some(m) => (
                    200,
                    vec![("x-onus-map-commit".into(), commit.into())],
                    m.clone(),
                ),
                None => (
                    404,
                    vec![],
                    b"{\"error\":\"no_map\",\"message\":\"no map of that commit\"}".to_vec(),
                ),
            }
        }
        _ => (
            404,
            vec![],
            json!({ "error": "not_found", "message": format!("no route {}", req.path) })
                .to_string()
                .into_bytes(),
        ),
    }
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
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The shop fixture, committed and pushed to a bare remote (Onus Cloud reads only what
/// is pushed); its remote is not GitHub, so ONUS_CLOUD_REPO names it.
struct Shop {
    _dir: tempfile::TempDir,
    root: PathBuf,
    head: String,
}

fn shop() -> Shop {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    copy_dir(&fixture().join("base"), &root);
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "base"]);
    let bare = dir.path().join("origin.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-q", "origin", "main"]);
    git(&root, &["fetch", "-q", "origin"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    Shop {
        root,
        head,
        _dir: dir,
    }
}

/// A config folder signed in to the fake API as a person, and a cache folder.
struct Login {
    config: tempfile::TempDir,
    cache: tempfile::TempDir,
}

fn login(fake: &Fake) -> Login {
    let config = tempfile::tempdir().unwrap();
    std::fs::write(
        config.path().join("cloud.json"),
        json!({ "url": fake.url, "token": "onus_cli_login", "organisation": "acme", "user": {} })
            .to_string(),
    )
    .unwrap();
    Login {
        config,
        cache: tempfile::tempdir().unwrap(),
    }
}

impl Login {
    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("ONUS_CONFIG_DIR", self.config.path().display().to_string()),
            ("ONUS_CACHE_DIR", self.cache.path().display().to_string()),
            ("ONUS_CLOUD_REPO", "acme/shop".into()),
        ]
    }
}

fn mcp(root: &Path, extra: &[&str], env: &[(&'static str, String)]) -> Mcp {
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    Mcp::start_env(root, extra, &env)
}

/// Runs the binary with only these Onus Cloud variables: (exit code, stdout, stderr).
fn onus(dir: &Path, env: &[(&'static str, String)], args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_onus"));
    for k in CLOUD_ENV {
        cmd.env_remove(k);
    }
    let out = cmd
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Keys, and a token for a task that writes `writes`.
struct Keys {
    dir: tempfile::TempDir,
    token: String,
}

impl Keys {
    fn new(task: &str, writes: &str) -> Keys {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (code, _, err) = onus(d, &[], &["token", "keygen", "--out", "."]);
        assert_eq!(code, 0, "{err}");
        std::fs::write(
            d.join("plan.yaml"),
            format!("task: {task}\nwrites: [\"{writes}\"]\n"),
        )
        .unwrap();
        let (code, token, err) = onus(
            d,
            &[],
            &["token", "mint", "--plan", "plan.yaml", "--key", "root.key"],
        );
        assert_eq!(code, 0, "{err}");
        Keys { dir, token }
    }

    fn public(&self) -> String {
        self.dir.path().join("root.pub").display().to_string()
    }

    fn private(&self) -> String {
        self.dir.path().join("root.key").display().to_string()
    }
}

fn tool_names(m: &mut Mcp) -> Vec<String> {
    m.call("tools/list", json!({}))["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_person_mints_a_narrow_token_from_the_command_line() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let (code, out, err) = onus(
        &shop.root,
        &l.env(),
        &[
            "cloud",
            "token",
            "create",
            "--repo",
            "acme/shop",
            "--scope",
            "pulls:read",
            "--ttl",
            "1h",
        ],
    );
    assert_eq!(code, 0, "{err}");
    // The token alone on stdout, what it opens on stderr.
    assert_eq!(out, "onus_tok_1");
    assert!(
        err.contains("token for acme (acme/shop): pulls:read"),
        "{err}"
    );
    assert!(err.contains("onus cloud token revoke tok-1"), "{err}");
    let seen = fake.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        (seen[0].method.as_str(), seen[0].path.as_str()),
        ("POST", "/v1/orgs/acme/tokens")
    );
    assert_eq!(seen[0].auth, "Bearer onus_cli_login");
    assert_eq!(
        seen[0].body,
        json!({ "repositories": ["acme/shop"], "scopes": ["pulls:read"], "ttlSeconds": 3600 })
    );
    // Wider than a day, or a scope that does not exist: refused before any request.
    let (code, _, err) = onus(
        &shop.root,
        &l.env(),
        &[
            "cloud",
            "token",
            "create",
            "--scope",
            "pulls:read",
            "--ttl",
            "2d",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("60 seconds to 24 hours"), "{err}");
    let (code, _, err) = onus(
        &shop.root,
        &l.env(),
        &[
            "cloud",
            "token",
            "create",
            "--repo",
            "acme/shop",
            "--scope",
            "admin",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("`admin` is not a scope"), "{err}");
    assert_eq!(fake.seen().len(), 1);
}

#[test]
fn onus_mcp_narrows_a_login_and_never_sends_it_on() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let mut m = mcp(&shop.root, &[], &l.env());
    let names = tool_names(&mut m);
    for t in [
        "onus_blocked",
        "onus_cloud_map",
        "onus_cloud_token",
        "onus_cloud_pulls",
    ] {
        assert!(names.iter().any(|n| n == t), "{t} in {names:?}");
    }
    for _ in 0..2 {
        let (failed, out) = m.tool("onus_cloud_pulls", json!({}));
        assert!(!failed, "{out}");
    }
    let mints = fake.requests("POST", "/v1/orgs/acme/tokens");
    assert_eq!(mints.len(), 1, "{:?}", fake.seen());
    assert_eq!(mints[0].auth, "Bearer onus_cli_login");
    assert_eq!(
        mints[0].body,
        json!({
            "repositories": ["acme/shop"],
            "scopes": ["env:read", "env:run", "evidence:write", "pulls:read", "map:read", "escalation:write"],
            "ttlSeconds": 28800,
            "label": "onus mcp",
        })
    );
    let others: Vec<Seen> = fake
        .seen()
        .into_iter()
        .filter(|s| !s.path.ends_with("/tokens"))
        .collect();
    assert_eq!(others.len(), 2, "{others:?}");
    for s in &others {
        assert_eq!(s.auth, "Bearer onus_tok_1", "{s:?}");
    }

    // A narrower token for a sub-agent, minted from the scoped one.
    let (failed, out) = m.tool("onus_cloud_token", json!({ "scopes": ["pulls:read"] }));
    assert!(!failed, "{out}");
    assert_eq!(out["token"], "onus_tok_2");
    assert!(
        out["next"].as_str().unwrap().contains("ONUS_CLOUD_TOKEN"),
        "{out}"
    );
    let mints = fake.requests("POST", "/v1/orgs/acme/tokens");
    assert_eq!(mints.len(), 2);
    assert_eq!(mints[1].auth, "Bearer onus_tok_1");
    assert_eq!(mints[1].body["repositories"], json!(["acme/shop"]));
    assert_eq!(mints[1].body["scopes"], json!(["pulls:read"]));
    // The audit log keeps its id, never the token.
    let log = std::fs::read_to_string(shop.root.join(".onus/audit.jsonl")).unwrap();
    assert!(
        log.contains("tok-2") && !log.contains("onus_tok_2"),
        "{log}"
    );
    assert!(
        !fake
            .seen()
            .iter()
            .skip(1)
            .any(|s| s.auth.contains("onus_cli_login") && !s.path.ends_with("/tokens"))
    );
}

#[test]
fn a_given_cloud_token_is_used_and_nothing_else_is_read() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    // A login that cannot be read: it is never read.
    std::fs::write(l.config.path().join("cloud.json"), "not json").unwrap();
    let mut env = l.env();
    env.push(("ONUS_CLOUD_TOKEN", "onus_tok_given".into()));
    env.push(("ONUS_CLOUD_URL", fake.url.clone()));
    env.push(("ONUS_CLOUD_ORG", "acme".into()));
    let mut m = mcp(&shop.root, &[], &env);
    let (failed, out) = m.tool("onus_cloud_pulls", json!({}));
    assert!(!failed, "{out}");
    let seen = fake.seen();
    assert!(fake.requests("POST", "/v1/orgs/acme/tokens").is_empty());
    assert!(!seen.is_empty());
    for s in &seen {
        assert_eq!(s.auth, "Bearer onus_tok_given");
    }
}

/// Files a permission request through MCP; returns its local id and what the agent read.
fn escalate(m: &mut Mcp) -> (String, Value) {
    let (failed, esc) = m.tool(
        "onus_escalate",
        json!({
            "scopes": ["write:path:services/billing/src/**"],
            "evidence": ["rationale:billing formats the SMS"],
            "reason": "the SMS needs the billing format",
        }),
    );
    assert!(!failed, "{esc}");
    (esc["id"].as_str().unwrap().to_string(), esc)
}

#[test]
fn a_grant_in_onus_cloud_is_minted_by_the_door_from_its_own_request() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let keys = Keys::new("sms-alerts", "services/notifications/**");
    let mut m = mcp(
        &shop.root,
        &[
            "--token",
            &keys.token,
            "--key-public",
            &keys.public(),
            "--key",
            &keys.private(),
        ],
        &l.env(),
    );
    let (id, esc) = escalate(&mut m);
    assert_eq!(esc["state"], "waiting for a person in Onus Cloud", "{esc}");
    assert_eq!(esc["owners"], json!(["@team-billing"]));
    assert!(esc["url"].as_str().unwrap().contains("/escalations/esc-1"));
    assert!(
        shop.root
            .join(format!(".onus/escalations/{id}.json"))
            .exists()
    );

    let filed = fake.requests("POST", "/v1/orgs/acme/escalations");
    assert_eq!(filed.len(), 1);
    let b = &filed[0].body;
    assert_eq!(b["clientRequestId"], id.as_str());
    assert_eq!(b["repository"], "acme/shop");
    assert_eq!(b["ref"], shop.head.as_str());
    assert_eq!(b["kind"], "permission");
    assert_eq!(b["scopes"], json!(["write:path:services/billing/src/**"]));
    assert_eq!(b["evidence"], json!(["rationale:billing formats the SMS"]));
    // The token's inspection, never the token.
    assert_eq!(b["token"]["task"], "sms-alerts");
    assert!(
        b["token"]["rights"]
            .as_array()
            .unwrap()
            .contains(&json!("write:path:services/notifications/**")),
        "{b}"
    );
    assert!(b["token"].get("attenuations").is_none());
    assert!(b["token"]["expires"].is_u64());
    assert!(!filed[0].raw.contains(&keys.token));
    assert!(fake.seen().iter().all(|s| !s.raw.contains(&keys.token)));
    // The local policy did not run: the log says only where it went.
    let log = shop.root.join(".onus/audit.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("sent-to-onus-cloud"), "{text}");
    assert!(!text.contains("\"needs-person\""), "{text}");

    // Still waiting.
    let (_, state) = m.tool("onus_escalation", json!({ "id": id, "wait": 1 }));
    assert_eq!(
        state["state"], "waiting for a person in Onus Cloud",
        "{state}"
    );
    assert!(state.get("token").is_none());

    // Ada grants it in Onus Cloud: the door mints the widened token itself.
    fake.set(
        "esc-1",
        json!({ "state": "granted", "decision": { "by": { "id": "user:ada", "name": "Ada" }, "at": "now" } }),
    );
    let (failed, state) = m.tool("onus_escalation", json!({ "id": id, "wait": 1 }));
    assert!(!failed, "{state}");
    assert_eq!(state["state"], "granted", "{state}");
    assert_eq!(state["adopted"], true, "{state}");
    let granted = state["token"].as_str().unwrap().to_string();
    let (code, _, err) = onus(
        &shop.root,
        &[],
        &[
            "token",
            "check",
            "--token",
            &granted,
            "--key-public",
            &keys.public(),
            "write:path:services/billing/src/a.ts",
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, out, err) = onus(&shop.root, &[], &["audit", log.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("chain intact"), "{out}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("\"actor\":\"Ada (Onus Cloud)\""), "{text}");
    assert!(text.contains("granted by Ada (Onus Cloud)"), "{text}");
    // Settled: asking again does not ask Onus Cloud.
    let asked = fake.requests("GET", "/v1/orgs/acme/escalations/").len();
    let (_, again) = m.tool("onus_escalation", json!({ "id": id }));
    assert_eq!(again["state"], "granted");
    assert_eq!(
        fake.requests("GET", "/v1/orgs/acme/escalations/").len(),
        asked
    );
}

#[test]
fn without_the_root_key_the_operator_mints_the_grant() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let keys = Keys::new("sms-alerts", "services/notifications/**");
    let mut m = mcp(
        &shop.root,
        &["--token", &keys.token, "--key-public", &keys.public()],
        &l.env(),
    );
    let (id, _) = escalate(&mut m);
    fake.set(
        "esc-1",
        json!({ "state": "granted", "decision": { "by": { "name": "Ada" } } }),
    );
    let (_, state) = m.tool("onus_escalation", json!({ "id": id }));
    assert_eq!(state["state"], "granted", "{state}");
    assert!(state.get("token").is_none(), "{state}");
    let next = state["next"].as_str().unwrap();
    assert!(
        next.contains("the operator mints it: onus escalation grant"),
        "{next}"
    );
    assert!(next.contains("Ada (Onus Cloud)"), "{next}");
    // It names the request and the scopes Onus Cloud granted.
    assert!(next.contains(&format!("--id {id}")), "{next}");
    assert!(
        next.contains("--scope 'write:path:services/billing/src/**'"),
        "{next}"
    );

    let request = shop.root.join(format!(".onus/escalations/{id}.json"));
    let grant = |request: &Path| {
        onus(
            &shop.root,
            &[],
            &[
                "escalation",
                "grant",
                request.to_str().unwrap(),
                "--id",
                &id,
                "--scope",
                "write:path:services/billing/src/**",
                "--token",
                &keys.token,
                "--key-public",
                &keys.public(),
                "--key",
                &keys.private(),
                "--by",
                "Ada (Onus Cloud)",
            ],
        )
    };
    // The agent swaps in a wider request of its own making, with a consistent id:
    // the operator's command mints nothing from it.
    let kept = std::fs::read(&request).unwrap();
    let (code, _, err) = onus(
        &shop.root,
        &[],
        &[
            "escalate",
            "--task",
            "sms-alerts",
            "--scope",
            "write:path:services/billing/src/**",
            "--scope",
            "write:path:services/auth/**",
            "--evidence",
            "rationale:billing formats the SMS",
            "--reason",
            "the SMS needs the billing format",
            "--out",
            request.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = grant(&request);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("nothing is minted"), "{err}");
    let swapped = shop.root.join(".onus/escalations/swapped.json");
    std::fs::copy(&request, &swapped).unwrap();
    std::fs::write(&request, &kept).unwrap();
    // Under the right id, scopes other than the ones granted mint nothing either.
    let (code, _, err) = onus(
        &shop.root,
        &[],
        &[
            "escalation",
            "grant",
            swapped.to_str().unwrap(),
            "--scope",
            "write:path:services/billing/src/**",
            "--token",
            &keys.token,
            "--key-public",
            &keys.public(),
            "--key",
            &keys.private(),
            "--by",
            "Ada (Onus Cloud)",
        ],
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("nothing is minted"), "{err}");
    std::fs::remove_file(&swapped).unwrap();

    let (code, _, err) = grant(&request);
    assert_eq!(code, 0, "{err}");
    let (_, state) = m.tool("onus_escalation", json!({ "id": id }));
    assert_eq!(state["state"], "granted", "{state}");
    assert_eq!(state["adopted"], true, "{state}");
    assert!(state["token"].is_string());
}

#[test]
fn denials_answers_and_answers_about_other_requests() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let keys = Keys::new("sms-alerts", "services/notifications/**");
    let mut m = mcp(
        &shop.root,
        &[
            "--token",
            &keys.token,
            "--key-public",
            &keys.public(),
            "--key",
            &keys.private(),
        ],
        &l.env(),
    );
    let (id, _) = escalate(&mut m);
    // An answer about another request is refused, and nothing is minted.
    fake.set(
        "esc-1",
        json!({ "state": "granted", "clientRequestId": "ffffffffffff", "decision": { "by": { "name": "Mallory" } } }),
    );
    let (failed, msg) = m.tool("onus_escalation", json!({ "id": id }));
    assert!(failed, "{msg}");
    assert!(msg.as_str().unwrap().contains("is refused"), "{msg}");
    assert!(
        !shop
            .root
            .join(format!(".onus/escalations/{id}.token"))
            .exists()
    );

    fake.set(
        "esc-1",
        json!({ "state": "denied", "clientRequestId": id, "decision": { "by": { "name": "Grace" }, "answer": "billing is not this task's" } }),
    );
    let (_, state) = m.tool("onus_escalation", json!({ "id": id }));
    assert_eq!(state["state"], "denied", "{state}");
    assert_eq!(state["reason"], "billing is not this task's");

    // A blocked report: no rights asked for, and the agent is told to stop.
    let (failed, blocked) = m.tool(
        "onus_blocked",
        json!({
            "kind": "broken-test",
            "evidence": ["failing-test:services/orders/src/eta.test.ts"],
            "reason": "the test expects UTC; the fixture is local time",
        }),
    );
    assert!(!failed, "{blocked}");
    assert!(blocked["next"].as_str().unwrap().starts_with("Stop here."));
    assert_eq!(blocked["state"], "waiting for a person in Onus Cloud");
    let filed = fake.requests("POST", "/v1/orgs/acme/escalations");
    assert_eq!(filed[1].body["kind"], "broken-test");
    assert_eq!(filed[1].body["scopes"], json!([]));
    let bid = blocked["id"].as_str().unwrap();
    fake.set(
        "esc-2",
        json!({ "state": "resolved", "decision": { "by": { "name": "Grace" }, "answer": "Leave the test; say so in the PR." } }),
    );
    let (_, state) = m.tool("onus_escalation", json!({ "id": bid }));
    assert_eq!(state["state"], "answered", "{state}");
    assert_eq!(state["answer"], "Leave the test; say so in the PR.");
    let (failed, msg) = m.tool(
        "onus_blocked",
        json!({ "kind": "permission", "reason": "x" }),
    );
    assert!(failed);
    assert!(msg.as_str().unwrap().contains("onus_escalate"), "{msg}");
    let (code, out, err) = onus(
        &shop.root,
        &[],
        &[
            "audit",
            shop.root.join(".onus/audit.jsonl").to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("chain intact"));
}

#[test]
fn the_safe_exit_is_there_signed_out_too() {
    let shop = shop();
    let config = tempfile::tempdir().unwrap();
    let env = vec![("ONUS_CONFIG_DIR", config.path().display().to_string())];
    let mut m = mcp(&shop.root, &[], &env);
    let names = tool_names(&mut m);
    assert!(names.iter().any(|n| n == "onus_blocked"));
    assert!(
        !names.iter().any(|n| n.starts_with("onus_cloud_")),
        "{names:?}"
    );
    let (failed, out) = m.tool(
        "onus_blocked",
        json!({ "kind": "impossible-task", "reason": "there is no SMS provider", "task": "sms-alerts" }),
    );
    assert!(!failed, "{out}");
    assert_eq!(out["state"], "open");
    let id = out["id"].as_str().unwrap();
    let kept: Value = serde_json::from_str(
        &std::fs::read_to_string(shop.root.join(format!(".onus/escalations/{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(kept["kind"], "impossible-task");
    assert_eq!(kept["scopes"], json!([]));
}

#[test]
fn the_map_onus_cloud_keeps_answers_as_onus_query_does() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    // The map the binary built in Onus Cloud, at the pushed head.
    let (code, map, err) = onus(
        &shop.root,
        &[],
        &["map", shop.root.to_str().unwrap(), "--json"],
    );
    assert_eq!(code, 0, "{err}");
    // The worker maps a checked-out folder, so the map names no commit: Onus Cloud
    // vouches for it in x-onus-map-commit.
    let map: Value = serde_json::from_str(&map).unwrap();
    assert_eq!(map["commit"], "worktree");
    fake.serve_map(&shop.head, serde_json::to_vec(&map).unwrap());
    let (code, local, err) = onus(
        &shop.root,
        &[],
        &[
            "query",
            "dependents",
            "UserPreferences",
            "--no-server",
            "--repo",
            shop.root.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{err}");
    let local: Value = serde_json::from_str(&local).unwrap();

    let mut m = mcp(&shop.root, &[], &l.env());
    let ask = json!({
        "commit": shop.head,
        "question": { "query": "dependents", "target": "UserPreferences" },
    });
    let (failed, hosted) = m.tool("onus_cloud_map", ask.clone());
    assert!(!failed, "{hosted}");
    assert_eq!(hosted["result"], local["result"]);
    assert_eq!(hosted["map"]["source"], "onus-cloud");
    assert_eq!(hosted["map"]["commit"], shop.head.as_str());
    assert!(hosted.get("componentDirs").is_none());
    // A second question is answered from the cache.
    let (failed, again) = m.tool("onus_cloud_map", ask.clone());
    assert!(!failed, "{again}");
    assert_eq!(again["map"]["commit"], shop.head.as_str());
    assert_eq!(
        fake.requests("GET", "/v1/orgs/acme/repos/acme%2Fshop/map")
            .len(),
        1
    );
    let cached = l
        .cache
        .path()
        .join("cloud-maps")
        .join(fake.url.trim_start_matches("http://").replace(':', "_"))
        .join("acme/acme__shop")
        .join(format!("{}.json", shop.head));
    assert!(cached.exists(), "{}", cached.display());
    drop(m);

    // Under a task token, what it may not read is left out.
    let keys = Keys::new("billing-fix", "services/billing/**");
    let mut m = mcp(
        &shop.root,
        &["--token", &keys.token, "--key-public", &keys.public()],
        &l.env(),
    );
    let (failed, limited) = m.tool("onus_cloud_map", ask.clone());
    assert!(!failed, "{limited}");
    assert!(
        limited["hidden"]
            .as_str()
            .unwrap()
            .contains("outside your read scope"),
        "{limited}"
    );
    // The remote is not GitHub's, so the repository the operator named is the
    // task's: naming it is allowed, naming another is refused.
    let mut named = ask.clone();
    named["repository"] = json!("acme/shop");
    let (failed, same) = m.tool("onus_cloud_map", named);
    assert!(!failed, "{same}");
    let mut other = ask;
    other["repository"] = json!("acme/storefront");
    let (failed, refused) = m.tool("onus_cloud_map", other);
    assert!(failed);
    assert!(
        refused.as_str().unwrap().contains(
            "your token is for this worktree's repository `acme/shop`, not `acme/storefront`"
        ),
        "{refused}"
    );
    drop(m);

    // The command line: the same answer as onus query.
    let (code, out, err) = onus(
        &shop.root,
        &l.env(),
        &[
            "cloud",
            "map",
            "dependents",
            "UserPreferences",
            "--commit",
            &shop.head,
        ],
    );
    assert_eq!(code, 0, "{err}");
    let out: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["result"], local["result"]);
    let (code, out, err) = onus(
        &shop.root,
        &l.env(),
        &["cloud", "map", "status", "--commit", &shop.head],
    );
    assert_eq!(code, 0, "{err}");
    let out: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["result"]["commit"], shop.head.as_str(), "{out}");
    let saved = l.cache.path().join("map.json");
    let (code, _, err) = onus(
        &shop.root,
        &l.env(),
        &[
            "cloud",
            "map",
            "--download",
            saved.to_str().unwrap(),
            "--commit",
            &shop.head,
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(err.contains(&format!("at {}", shop.head)), "{err}");
    let saved: Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(saved["commit"], shop.head.as_str());
    let (code, _, err) = onus(&shop.root, &l.env(), &["cloud", "map", "check"]);
    assert_eq!(code, 1);
    assert!(err.contains("needs a worktree"), "{err}");
    // A commit Onus Cloud never mapped.
    let (code, _, err) = onus(
        &shop.root,
        &l.env(),
        &["cloud", "map", "status", "--commit", &"0".repeat(40)],
    );
    assert_eq!(code, 1);
    assert!(err.contains("no map of that commit"), "{err}");
}

#[test]
fn signed_out_the_local_policy_decides_only_from_a_trusted_checkout() {
    let shop = shop();
    let config = tempfile::tempdir().unwrap();
    let env = vec![("ONUS_CONFIG_DIR", config.path().display().to_string())];
    let keys = Keys::new("sms-alerts", "services/notifications/**");
    // The agent weakens its own worktree: billing is no longer labelled payments.
    let yaml = shop.root.join("onus.yaml");
    let text = std::fs::read_to_string(&yaml).unwrap();
    let weakened = text.replace(
        r#"owners: ["@team-payments"], labels: [payments]"#,
        r#"owners: ["@team-payments"]"#,
    );
    assert_ne!(text, weakened);
    std::fs::write(&yaml, weakened).unwrap();
    let door = [
        "--token",
        keys.token.as_str(),
        "--key-public",
        &keys.public(),
        "--key",
        &keys.private(),
    ]
    .map(str::to_string);
    let door: Vec<&str> = door.iter().map(String::as_str).collect();

    // Without a trusted checkout the policy does not run: a person decides.
    let mut m = mcp(&shop.root, &door, &env);
    let (_, esc) = escalate(&mut m);
    assert_eq!(esc["state"], "open", "{esc}");
    drop(m);
    let log = shop.root.join(".onus/audit.jsonl");
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(!text.contains("\"needs-person\""), "{text}");

    // With one, the facts are the trusted checkout's, not the worktree's.
    let outside = tempfile::tempdir().unwrap();
    let trusted = outside.path().join("trusted");
    git(
        &shop.root,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            trusted.to_str().unwrap(),
            "main",
        ],
    );
    let mut with = door.clone();
    with.extend(["--trusted-repo", trusted.to_str().unwrap()]);
    let mut m = mcp(&shop.root, &with, &env);
    let (_, esc) = escalate(&mut m);
    assert_eq!(esc["state"], "waiting for a person", "{esc}");
    assert_eq!(esc["sensitive"], json!(["payments"]), "{esc}");
    // A rationale alone is never granted by policy.
    assert!(esc["reasons"].to_string().contains("grade 5"), "{esc}");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("\"needs-person\""), "{text}");
    drop(m);

    // A "trusted" checkout the agent can edit is refused at start.
    let mut inside = door.clone();
    inside.extend(["--trusted-repo", "services"]);
    let mut args = vec!["mcp", "--no-server"];
    args.extend(inside);
    let (code, _, err) = onus(&shop.root, &env, &args);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("inside the agent's worktree"), "{err}");
}

#[test]
fn a_request_changed_after_it_was_filed_mints_nothing() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let keys = Keys::new("sms-alerts", "services/notifications/**");
    let mut m = mcp(
        &shop.root,
        &[
            "--token",
            &keys.token,
            "--key-public",
            &keys.public(),
            "--key",
            &keys.private(),
        ],
        &l.env(),
    );
    let (id, _) = escalate(&mut m);
    let request = shop.root.join(format!(".onus/escalations/{id}.json"));
    let token = shop.root.join(format!(".onus/escalations/{id}.token"));
    let filed = std::fs::read_to_string(&request).unwrap();
    fake.set(
        "esc-1",
        json!({ "state": "granted", "decision": { "by": { "id": "user:ada", "name": "Ada" } } }),
    );

    // The agent widens the request it kept, after it was filed.
    let mut wider: Value = serde_json::from_str(&filed).unwrap();
    wider["scopes"]
        .as_array_mut()
        .unwrap()
        .push(json!("write:path:services/auth/**"));
    std::fs::write(&request, wider.to_string()).unwrap();
    let (failed, msg) = m.tool("onus_escalation", json!({ "id": id, "wait": 1 }));
    assert!(failed, "{msg}");
    assert!(
        msg.as_str().unwrap().contains("changed after it was filed"),
        "{msg}"
    );
    assert!(!token.exists());

    // As filed, but Onus Cloud shows other scopes as granted.
    std::fs::write(&request, &filed).unwrap();
    fake.set("esc-1", json!({ "scopes": ["write:path:services/**"] }));
    let (failed, msg) = m.tool("onus_escalation", json!({ "id": id, "wait": 1 }));
    assert!(failed, "{msg}");
    assert!(msg.as_str().unwrap().contains("nothing is minted"), "{msg}");
    assert!(!token.exists());

    // As filed and as granted: exactly what was asked for.
    fake.set(
        "esc-1",
        json!({ "scopes": ["write:path:services/billing/src/**"] }),
    );
    let (failed, state) = m.tool("onus_escalation", json!({ "id": id, "wait": 1 }));
    assert!(!failed, "{state}");
    assert_eq!(state["adopted"], true, "{state}");
    let granted = state["token"].as_str().unwrap().to_string();
    let check = |right: &str| {
        onus(
            &shop.root,
            &[],
            &[
                "token",
                "check",
                "--token",
                &granted,
                "--key-public",
                &keys.public(),
                right,
            ],
        )
        .0
    };
    assert_eq!(check("write:path:services/billing/src/a.ts"), 0);
    assert_ne!(check("write:path:services/auth/x.ts"), 0);

    // A kept token that does not prove this grant is not handed out.
    let other = Keys::new("sms-alerts", "services/**");
    std::fs::write(&token, format!("{}\n", other.token)).unwrap();
    let (_, state) = m.tool("onus_escalation", json!({ "id": id }));
    assert!(state.get("token").is_none(), "{state}");
    assert!(
        state["next"].as_str().unwrap().contains("cannot be used"),
        "{state}"
    );
}

#[test]
fn the_policy_in_onus_cloud_is_logged_as_the_policy() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let keys = Keys::new("sms-alerts", "services/notifications/**");
    let mut m = mcp(
        &shop.root,
        &[
            "--token",
            &keys.token,
            "--key-public",
            &keys.public(),
            "--key",
            &keys.private(),
        ],
        &l.env(),
    );
    let (id, _) = escalate(&mut m);
    fake.set(
        "esc-1",
        json!({ "state": "granted", "decision": { "by": { "id": "system:policy", "name": "policy" } } }),
    );
    let (failed, state) = m.tool("onus_escalation", json!({ "id": id, "wait": 1 }));
    assert!(!failed, "{state}");
    assert_eq!(state["state"], "granted", "{state}");
    let text = std::fs::read_to_string(shop.root.join(".onus/audit.jsonl")).unwrap();
    assert!(
        text.contains("\"actor\":\"system:policy (Onus Cloud)\""),
        "{text}"
    );
    assert!(!text.contains("a person (Onus Cloud)"), "{text}");
}

#[test]
fn a_narrow_token_is_taught_what_it_lacks() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let mut env = l.env();
    env.push(("ONUS_CLOUD_TOKEN", "onus_tok_narrow".into()));
    env.push(("ONUS_CLOUD_URL", fake.url.clone()));
    env.push(("ONUS_CLOUD_ORG", "acme".into()));
    let (code, _, err) = onus(&shop.root, &env, &["cloud", "env", "list"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("this token cannot read environments (env:read)"),
        "{err}"
    );
    assert!(
        err.contains(
            "mint one with `onus cloud token create --scope env:read`, or ask with onus_escalate"
        ),
        "{err}"
    );
    // Its status says what it is, and that it cannot see environments.
    let mut m = mcp(&shop.root, &[], &env);
    let (failed, status) = m.tool("onus_cloud_status", json!({}));
    assert!(!failed, "{status}");
    assert_eq!(status["signedInWith"], "token", "{status}");
    assert!(status["environments"].is_null(), "{status}");
    assert_eq!(status["token"]["scopes"], json!(["pulls:read"]), "{status}");
}

#[test]
fn a_scoped_token_near_its_end_is_minted_again() {
    let fake = Fake::start();
    // Each token ends within the minute.
    fake.world.lock().unwrap().mint_ttl = Some(30);
    let shop = shop();
    let l = login(&fake);
    let mut m = mcp(&shop.root, &[], &l.env());
    for _ in 0..2 {
        let (failed, out) = m.tool("onus_cloud_pulls", json!({}));
        assert!(!failed, "{out}");
    }
    let mints = fake.requests("POST", "/v1/orgs/acme/tokens");
    assert_eq!(mints.len(), 2, "{:?}", fake.seen());
    assert!(mints.iter().all(|s| s.auth == "Bearer onus_cli_login"));
    let pulls = fake.requests("GET", "/v1/orgs/acme/pulls");
    assert_eq!(pulls[0].auth, "Bearer onus_tok_1");
    assert_eq!(pulls[1].auth, "Bearer onus_tok_2");
}

#[test]
fn the_command_line_files_reports_and_collects_answers() {
    let fake = Fake::start();
    let shop = shop();
    let l = login(&fake);
    let root = shop.root.to_str().unwrap();
    let (code, _, err) = onus(
        &shop.root,
        &l.env(),
        &[
            "cloud",
            "blocked",
            "--kind",
            "contradictory-spec",
            "--task",
            "sms-alerts",
            "--evidence",
            "rationale:the spec says UTC and local time",
            "--reason",
            "which time zone?",
            "--repo",
            root,
        ],
    );
    assert_eq!(code, 3, "{err}");
    assert!(err.contains("waiting for a person in Onus Cloud"), "{err}");
    assert!(err.contains("Stop here."), "{err}");
    let id = fake.requests("POST", "/v1/orgs/acme/escalations")[0].body["clientRequestId"]
        .as_str()
        .unwrap()
        .to_string();
    fake.set(
        "esc-1",
        json!({ "state": "resolved", "decision": { "by": { "name": "Grace" }, "answer": "Local time." } }),
    );
    let (code, _, err) = onus(&shop.root, &l.env(), &["cloud", "escalation", &id]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("answered — Local time."), "{err}");

    // A permission request, granted in Onus Cloud and minted here with --key.
    let keys = Keys::new("sms-alerts", "services/notifications/**");
    let door = [
        "--token",
        keys.token.as_str(),
        "--key-public",
        &keys.public(),
        "--key",
        &keys.private(),
    ]
    .map(str::to_string);
    let mut args: Vec<String> = [
        "cloud",
        "escalate",
        "--task",
        "sms-alerts",
        "--scope",
        "write:path:services/billing/**",
        "--evidence",
        "rationale:billing formats the SMS",
        "--reason",
        "the SMS needs it",
        "--repo",
        root,
        "--pull",
        "7",
    ]
    .map(str::to_string)
    .to_vec();
    args.extend(door.iter().cloned());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, out, err) = onus(&shop.root, &l.env(), &args);
    assert_eq!(code, 3, "{err}");
    assert!(out.is_empty());
    let filed = fake.requests("POST", "/v1/orgs/acme/escalations");
    assert_eq!(filed[1].body["pull"], 7);
    let id = filed[1].body["clientRequestId"]
        .as_str()
        .unwrap()
        .to_string();
    fake.set(
        "esc-2",
        json!({ "state": "granted", "decision": { "by": { "name": "Ada" } } }),
    );
    let mut show = vec!["cloud", "escalation", id.as_str()];
    show.extend(door.iter().map(String::as_str));
    let (code, token, err) = onus(&shop.root, &l.env(), &show);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = onus(
        &shop.root,
        &[],
        &[
            "token",
            "check",
            "--token",
            &token,
            "--key-public",
            &keys.public(),
            "write:path:services/billing/x.ts",
        ],
    );
    assert_eq!(code, 0, "{err}");

    // Withdrawn from the command line: the audit log has it at once.
    let (code, _, err) = onus(
        &shop.root,
        &l.env(),
        &[
            "cloud",
            "blocked",
            "--kind",
            "impossible-task",
            "--task",
            "sms-alerts",
            "--reason",
            "there is no SMS provider",
            "--repo",
            root,
        ],
    );
    assert_eq!(code, 3, "{err}");
    let id = fake.requests("POST", "/v1/orgs/acme/escalations")[2].body["clientRequestId"]
        .as_str()
        .unwrap()
        .to_string();
    let elsewhere = tempfile::tempdir().unwrap();
    let (code, _, err) = onus(
        elsewhere.path(),
        &l.env(),
        &["cloud", "escalation", "withdraw", &id, "--repo", root],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        fake.requests("POST", "/v1/orgs/acme/escalations/esc-3/withdraw")
            .len(),
        1
    );
    let log = std::fs::read_to_string(shop.root.join(".onus/audit.jsonl")).unwrap();
    assert!(
        log.lines()
            .any(|l| l.contains(&id) && l.contains("\"decision\":\"withdrawn\"")),
        "{log}"
    );
}
