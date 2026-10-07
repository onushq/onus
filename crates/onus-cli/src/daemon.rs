//! One map server per repository, shared by every agent and worktree.
//!
//! Each agent's `onus mcp` (or `onus query`) talks to the repository's
//! server over a Unix socket, starting it if it is not running. The server
//! keeps one [`Session`] per worktree and one [`FactsCache`] per repository,
//! kept on disk in the repository's git folder, so:
//!
//! - ten agents in one worktree share one map and one rebuild after a change;
//! - a new worktree reuses the facts of every file it shares with the others;
//! - a restarted server starts from the facts on disk.
//!
//! The protocol is one JSON request per line and one JSON response per
//! line. Without Unix sockets (Windows), the same [`Server`] runs in the
//! calling process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use onus_index::Query;
use onus_map::FactsCache;
use serde::{Deserialize, Serialize};

use crate::session::Session;

/// A request for the map server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The worktree, as an absolute path.
    pub root: PathBuf,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Op {
    Ping,
    /// A question about the worktree's map.
    Query {
        query: Query,
    },
    /// Changes in meaning between `base` (default `HEAD`) and the
    /// worktree as it is now, saved or not committed.
    Check {
        #[serde(default)]
        base: Option<String>,
    },
    /// Sessions and cache use.
    Stats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Which map answered and whether it was rebuilt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<serde_json::Value>,
}

/// Sessions and caches; serves requests from sockets or in-process.
#[derive(Debug, Default)]
pub struct Server {
    sessions: Mutex<HashMap<PathBuf, Arc<Session>>>,
    caches: Mutex<HashMap<PathBuf, Arc<FactsCache>>>,
    bases: crate::BaseTrees,
    started: Option<Instant>,
}

impl Server {
    pub fn new() -> Server {
        Server {
            started: Some(Instant::now()),
            ..Server::default()
        }
    }

    pub fn handle(&self, req: &Request) -> Response {
        match self.try_handle(req) {
            Ok((ok, map)) => Response {
                ok: Some(ok),
                error: None,
                map,
            },
            Err(e) => Response {
                ok: None,
                error: Some(format!("{e:#}")),
                map: None,
            },
        }
    }

    fn try_handle(&self, req: &Request) -> Result<(serde_json::Value, Option<serde_json::Value>)> {
        match &req.op {
            Op::Ping => Ok((serde_json::json!("pong"), None)),
            Op::Stats => Ok((self.stats(), None)),
            Op::Query { query } => {
                let session = self.session(&req.root)?;
                let (index, meta) = session.index()?;
                let answer = index.answer(query).map_err(anyhow::Error::msg)?;
                Ok((answer, Some(serde_json::to_value(meta)?)))
            }
            Op::Check { base } => {
                let session = self.session(&req.root)?;
                let (index, meta) = session.index()?;
                let base = base.as_deref().unwrap_or("HEAD");
                let report = crate::check_worktree(
                    session.root(),
                    base,
                    index.map(),
                    self.cache_for(&req.root)?,
                    &self.bases,
                )?;
                Ok((report, Some(serde_json::to_value(meta)?)))
            }
        }
    }

    fn session(&self, root: &Path) -> Result<Arc<Session>> {
        let root = canonical(root).with_context(|| format!("cannot open {}", root.display()))?;
        if let Some(s) = self
            .sessions
            .lock()
            .ok()
            .and_then(|m| m.get(&root).cloned())
        {
            return Ok(s);
        }
        let cache = self.cache_for(&root)?;
        let session = Arc::new(Session::new(&root, cache)?);
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("sessions poisoned"))?;
        Ok(sessions.entry(root).or_insert(session).clone())
    }

    /// The facts cache of the repository `root` belongs to: on disk in its
    /// git folder, shared by all of its worktrees.
    fn cache_for(&self, root: &Path) -> Result<Arc<FactsCache>> {
        let common = git_common_dir(root);
        let key = common.clone().unwrap_or_else(|| root.to_path_buf());
        let mut caches = self
            .caches
            .lock()
            .map_err(|_| anyhow::anyhow!("caches poisoned"))?;
        Ok(caches
            .entry(key)
            .or_insert_with(|| {
                Arc::new(match &common {
                    Some(dir) => FactsCache::on_disk(&dir.join("onus").join("facts")),
                    None => FactsCache::in_memory(),
                })
            })
            .clone())
    }

    fn stats(&self) -> serde_json::Value {
        let sessions: Vec<String> = self
            .sessions
            .lock()
            .map(|m| {
                let mut v: Vec<String> = m.keys().map(|p| p.display().to_string()).collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        let caches: Vec<serde_json::Value> = self
            .caches
            .lock()
            .map(|m| {
                m.iter()
                    .map(|(k, c)| {
                        serde_json::json!({ "repository": k.display().to_string(), "facts": c.stats() })
                    })
                    .collect()
            })
            .unwrap_or_default();
        serde_json::json!({
            "pid": std::process::id(),
            "uptimeSeconds": self.started.map(|s| s.elapsed().as_secs()).unwrap_or(0),
            "worktrees": sessions,
            "caches": caches,
        })
    }
}

/// `path`, absolute and with symbolic links resolved, without the `\\?\`
/// prefix Windows adds (git and file watchers use plain paths).
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let p = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
        {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(p)
}

/// The shared git folder of the repository `root` is in (the same for
/// every worktree), or `None` outside git.
pub fn git_common_dir(root: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    canonical(&dir).ok()
}

/// The worktree `dir` is in: the top of its git checkout, or `dir` itself.
pub fn worktree_root(dir: &Path) -> PathBuf {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .and_then(|p| canonical(&p).ok())
        .unwrap_or_else(|| canonical(dir).unwrap_or_else(|_| dir.to_path_buf()))
}

/// Where the server of the repository holding `root` listens. Sockets live
/// in a private folder (`$XDG_RUNTIME_DIR`, or a per-user temporary
/// folder), named by a hash of the repository's git folder, because socket
/// paths are limited to about 100 bytes.
pub fn socket_path(root: &Path) -> PathBuf {
    let repo = git_common_dir(root).unwrap_or_else(|| root.to_path_buf());
    let hash = onus_core::hash::sha256_hex(repo.to_string_lossy().as_bytes());
    runtime_dir().join(format!("{}.sock", &hash[..16]))
}

fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("onus");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("onus-{user}"))
}

#[cfg(unix)]
pub use unix::{Client, serve};

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use anyhow::bail;

    use super::*;

    /// Runs the server on `socket` until it has been idle for `idle`.
    pub fn serve(socket: &Path, idle: Duration) -> Result<()> {
        let dir = socket.parent().context("socket path has no folder")?;
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        if socket.exists() {
            if UnixStream::connect(socket).is_ok() {
                // Another server is already listening.
                return Ok(());
            }
            std::fs::remove_file(socket)?;
        }
        let listener = UnixListener::bind(socket)
            .with_context(|| format!("cannot listen on {}", socket.display()))?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
        let server = Arc::new(Server::new());
        let active = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(Mutex::new(Instant::now()));
        {
            let (active, last, socket) = (active.clone(), last.clone(), socket.to_path_buf());
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(5));
                    let quiet = last.lock().map(|t| t.elapsed() >= idle).unwrap_or(false);
                    if quiet && active.load(Ordering::SeqCst) == 0 {
                        let _ = std::fs::remove_file(&socket);
                        std::process::exit(0);
                    }
                }
            });
        }
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (server, active, last) = (server.clone(), active.clone(), last.clone());
            active.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let _ = serve_connection(&server, stream, &last);
                active.fetch_sub(1, Ordering::SeqCst);
                if let Ok(mut t) = last.lock() {
                    *t = Instant::now();
                }
            });
        }
        Ok(())
    }

    fn serve_connection(server: &Server, stream: UnixStream, last: &Mutex<Instant>) -> Result<()> {
        let mut writer = stream.try_clone()?;
        for line in BufReader::new(stream).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(mut t) = last.lock() {
                *t = Instant::now();
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => server.handle(&req),
                Err(e) => Response {
                    ok: None,
                    error: Some(format!("invalid request: {e}")),
                    map: None,
                },
            };
            serde_json::to_writer(&mut writer, &response)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
        Ok(())
    }

    /// A connection to the repository's server, started on first use.
    #[derive(Debug)]
    pub struct Client {
        stream: Mutex<Option<(UnixStream, BufReader<UnixStream>)>>,
        socket: PathBuf,
    }

    impl Client {
        pub fn new(socket: PathBuf) -> Client {
            Client {
                stream: Mutex::new(None),
                socket,
            }
        }

        pub fn request(&self, req: &Request) -> Result<Response> {
            let mut guard = self
                .stream
                .lock()
                .map_err(|_| anyhow::anyhow!("client poisoned"))?;
            // One retry: the server may have exited while idle.
            for attempt in 0..2 {
                if guard.is_none() {
                    *guard = Some(self.connect()?);
                }
                let Some((writer, reader)) = guard.as_mut() else {
                    continue;
                };
                let mut line = serde_json::to_string(req)?;
                line.push('\n');
                let mut answer = String::new();
                let sent = writer
                    .write_all(line.as_bytes())
                    .and_then(|_| writer.flush())
                    .and_then(|_| reader.read_line(&mut answer));
                match sent {
                    Ok(n) if n > 0 => return Ok(serde_json::from_str(&answer)?),
                    _ if attempt == 0 => *guard = None,
                    Ok(_) => bail!("the map server closed the connection"),
                    Err(e) => return Err(e.into()),
                }
            }
            bail!("cannot reach the map server")
        }

        fn connect(&self) -> Result<(UnixStream, BufReader<UnixStream>)> {
            if let Ok(s) = UnixStream::connect(&self.socket) {
                return Ok((s.try_clone()?, BufReader::new(s)));
            }
            start_server(&self.socket)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(s) = UnixStream::connect(&self.socket) {
                    return Ok((s.try_clone()?, BufReader::new(s)));
                }
                if Instant::now() > deadline {
                    bail!("the map server did not start ({})", self.socket.display());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn start_server(socket: &Path) -> Result<()> {
        use std::os::unix::process::CommandExt;
        let exe = std::env::current_exe().context("cannot find the onus binary")?;
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("serve").arg("--socket").arg(socket);
        // How long the server stays up without requests (seconds).
        if let Ok(idle) = std::env::var("ONUS_SERVER_IDLE") {
            cmd.arg("--idle").arg(idle);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // Its own process group: it outlives the agent that started it.
            .process_group(0)
            .spawn()
            .context("cannot start the map server")?;
        Ok(())
    }
}
