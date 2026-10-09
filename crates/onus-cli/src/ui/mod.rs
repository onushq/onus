//! `onus ui`: the web interface, served on loopback.
//!
//! The interface is a static app built from `ui/` and embedded in the
//! binary; everything it shows comes from a JSON API over the same code the
//! commands run. The server answers only on 127.0.0.1, only to requests
//! whose `Host` is a loopback name (so a web page elsewhere cannot reach it
//! through DNS rebinding), and the API only with the session's token, which
//! the opened URL carries in its fragment.

pub mod api;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Args;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::HeaderValue;
use hyper::{Method, Request, Response, StatusCode};

include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));

#[derive(Debug, Args)]
pub struct UiArgs {
    /// The repository (default: the one containing the current directory).
    #[arg(long, value_name = "DIR")]
    repo: Option<PathBuf>,
    /// The port on 127.0.0.1 (0 picks a free one).
    #[arg(long, default_value_t = 4387)]
    port: u16,
    /// Do not open a browser.
    #[arg(long)]
    no_open: bool,
    /// Outcome records (default: .onus/outcomes.jsonl in the repository).
    #[arg(long, value_name = "FILE")]
    outcomes: Option<PathBuf>,
    /// The audit log (default: .onus/audit.jsonl in the repository).
    #[arg(long, value_name = "FILE")]
    audit: Option<PathBuf>,
    /// Escalation requests (default: .onus/escalations in the repository).
    #[arg(long, value_name = "DIR")]
    escalations: Option<PathBuf>,
    /// The root key pair, root.key and root.pub (default: ~/.onus-keys).
    #[arg(long, value_name = "DIR")]
    keys: Option<PathBuf>,
    /// Use this session token instead of a random one (for developing the
    /// interface against a running server).
    #[arg(long, value_name = "TOKEN", hide = true, env = "ONUS_UI_TOKEN")]
    session_token: Option<String>,
}

/// What the API works on.
pub struct State {
    pub root: PathBuf,
    pub token: String,
    pub port: u16,
    pub server: onus_cli::daemon::Server,
    pub outcomes: PathBuf,
    pub audit: PathBuf,
    pub escalations: PathBuf,
    /// Submissions handed in by agents (`onus_submit`) and their judgments.
    pub submissions: PathBuf,
    pub keys: PathBuf,
    /// The root public key that verifies task tokens.
    pub public_key: PathBuf,
}

/// Where a repository's records are kept unless flags say otherwise.
#[derive(Debug, Default)]
pub struct Places {
    pub outcomes: Option<PathBuf>,
    pub audit: Option<PathBuf>,
    pub escalations: Option<PathBuf>,
    pub keys: Option<PathBuf>,
    pub public_key: Option<PathBuf>,
}

impl State {
    /// The state for the worktree `root`; records go to `.onus/` in it.
    pub fn new(root: PathBuf, token: String, port: u16, places: Places) -> State {
        let dot = root.join(".onus");
        let keys = places
            .keys
            .or_else(|| home().map(|h| h.join(".onus-keys")))
            .unwrap_or_else(|| dot.join("keys"));
        State {
            token,
            port,
            server: onus_cli::daemon::Server::new(),
            outcomes: places
                .outcomes
                .unwrap_or_else(|| dot.join("outcomes.jsonl")),
            audit: places.audit.unwrap_or_else(|| dot.join("audit.jsonl")),
            escalations: places
                .escalations
                .unwrap_or_else(|| dot.join("escalations")),
            submissions: dot.join("submissions"),
            public_key: places.public_key.unwrap_or_else(|| keys.join("root.pub")),
            keys,
            root,
        }
    }
}

/// 128 random bits as hex, from the same generator as root keys.
fn random_token() -> String {
    onus_doors::token::RootKey::generate().private_hex()[..32].to_string()
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn run(args: UiArgs) -> Result<i32> {
    let start = match &args.repo {
        Some(r) => r.clone(),
        None => std::env::current_dir()?,
    };
    let root = onus_cli::daemon::canonical(&onus_cli::daemon::worktree_root(&start))
        .with_context(|| format!("cannot open {}", start.display()))?;
    let token = args.session_token.clone().unwrap_or_else(random_token);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("cannot listen on {addr} (try --port 0)"))?;
        let bound = listener.local_addr()?;
        let state = Arc::new(State::new(
            root.clone(),
            token.clone(),
            bound.port(),
            Places {
                outcomes: args.outcomes,
                audit: args.audit,
                escalations: args.escalations,
                keys: args.keys,
                public_key: None,
            },
        ));
        let url = format!("http://127.0.0.1:{}/#token={token}", bound.port());
        // The URL is the first line on stdout, so scripts and tests can read it.
        println!("{url}");
        eprintln!(
            "onus: serving {} at http://127.0.0.1:{} (Ctrl-C to stop)",
            root.display(),
            bound.port()
        );
        if ASSETS.is_empty() {
            eprintln!("onus: this build has no web interface; see the page for how to add it");
        }
        if !args.no_open {
            open_browser(&url);
        }
        // Build the map in the background so the first page is quick.
        {
            let state = state.clone();
            tokio::task::spawn_blocking(move || {
                let _ = state.server.session(&state.root).and_then(|s| s.index());
            });
        }
        loop {
            let (stream, _) = listener.accept().await?;
            let state = state.clone();
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                let service = hyper::service::service_fn(move |req| {
                    let state = state.clone();
                    async move { Ok::<_, Infallible>(handle(state, req).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
        #[allow(unreachable_code)]
        Ok::<i32, anyhow::Error>(0)
    })
}

fn open_browser(url: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = cmd
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

type Reply = Response<Full<Bytes>>;

fn reply(status: StatusCode, content_type: &'static str, body: impl Into<Bytes>) -> Reply {
    let mut r = Response::new(Full::new(body.into()));
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert("content-type", HeaderValue::from_static(content_type));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    r
}

fn json_reply(status: StatusCode, value: &serde_json::Value) -> Reply {
    let mut r = reply(
        status,
        "application/json",
        serde_json::to_vec(value).unwrap_or_default(),
    );
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r
}

fn error(status: StatusCode, message: impl Into<String>) -> Reply {
    json_reply(status, &serde_json::json!({ "error": message.into() }))
}

/// Whether `host` (a Host header) names this server on loopback.
fn loopback_host(host: &str, port: u16) -> bool {
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|h| host == format!("{h}:{port}"))
}

/// Compares in time independent of where the strings differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

async fn handle(state: Arc<State>, req: Request<Incoming>) -> Reply {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !loopback_host(host, state.port) {
        return error(
            StatusCode::MISDIRECTED_REQUEST,
            "this server answers only on 127.0.0.1",
        );
    }
    let path = req.uri().path().to_string();
    if let Some(name) = path.strip_prefix("/api/") {
        return api_request(state, name.to_string(), req).await;
    }
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return error(StatusCode::METHOD_NOT_ALLOWED, "only GET");
    }
    asset(&path)
}

async fn api_request(state: Arc<State>, name: String, req: Request<Incoming>) -> Reply {
    if req.method() != Method::POST {
        return error(StatusCode::METHOD_NOT_ALLOWED, "the API takes POST");
    }
    let token = req
        .headers()
        .get("x-onus-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !same(token, &state.token) {
        return error(
            StatusCode::UNAUTHORIZED,
            "open the address `onus ui` printed: it carries this session's token",
        );
    }
    // A page from elsewhere cannot send the token, but say no to it early.
    if let Some(origin) = req.headers().get("origin").and_then(|h| h.to_str().ok())
        && !origin
            .strip_prefix("http://")
            .is_some_and(|h| loopback_host(h, state.port))
    {
        return error(StatusCode::FORBIDDEN, "cross-origin requests are refused");
    }
    let body = match Limited::new(req.into_body(), 16 * 1024 * 1024)
        .collect()
        .await
    {
        Ok(b) => b.to_bytes(),
        Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "the request is too large"),
    };
    let input: serde_json::Value = if body.is_empty() {
        serde_json::Value::Object(Default::default())
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return error(StatusCode::BAD_REQUEST, format!("not JSON: {e}")),
        }
    };
    let answer = tokio::task::spawn_blocking(move || api::call(&state, &name, input)).await;
    match answer {
        Ok(Ok(value)) => json_reply(StatusCode::OK, &serde_json::json!({ "ok": value })),
        Ok(Err(api::Error::NotFound(what))) => error(StatusCode::NOT_FOUND, what),
        Ok(Err(api::Error::Failed(e))) => error(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("the request failed: {e}"),
        ),
    }
}

fn content_type(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

const CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn asset(path: &str) -> Reply {
    let rel = path.trim_start_matches('/');
    let found = ASSETS
        .iter()
        .find(|(p, _)| *p == rel)
        // Pages of the app are routes of one page.
        .or_else(|| {
            (rel.is_empty() || !rel.rsplit('/').next().unwrap_or("").contains('.'))
                .then(|| ASSETS.iter().find(|(p, _)| *p == "index.html"))
                .flatten()
        });
    let Some((name, bytes)) = found else {
        if ASSETS.is_empty() {
            return reply(StatusCode::OK, "text/html; charset=utf-8", NOT_BUILT);
        }
        return error(StatusCode::NOT_FOUND, format!("no {path}"));
    };
    let mut r = reply(
        StatusCode::OK,
        content_type(name),
        Bytes::from_static(bytes),
    );
    let h = r.headers_mut();
    h.insert(
        "cache-control",
        HeaderValue::from_static(if name.starts_with("_app/immutable/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    h.insert("content-security-policy", HeaderValue::from_static(CSP));
    r
}

const NOT_BUILT: &str = "<!doctype html><meta charset=utf-8><title>Onus</title>\
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem}code{background:#8882;padding:.1em .3em;border-radius:4px}</style>\
<h1>This build of Onus has no web interface</h1>\
<p>Release builds include it. To add it to a build from source:</p>\
<pre><code>cd ui &amp;&amp; npm ci &amp;&amp; npm run build\ncargo build --release -p onus-cli</code></pre>\
<p>The API is running; everything else works from the command line (<code>onus help</code>).</p>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_on_this_port() {
        assert!(loopback_host("127.0.0.1:4387", 4387));
        assert!(loopback_host("localhost:4387", 4387));
        assert!(!loopback_host("localhost:80", 4387));
        assert!(!loopback_host("evil.example:4387", 4387));
        assert!(!loopback_host("127.0.0.1.evil.example:4387", 4387));
    }

    #[test]
    fn tokens_are_random_and_compared_in_full() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(same(&a, &a.clone()));
        assert!(!same(&a, &b));
        assert!(!same(&a, &a[..31]));
    }
}
