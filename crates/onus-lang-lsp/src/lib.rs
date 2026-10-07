//! A language provider that drives any language server over LSP (ADR 0006).
//!
//! For each file it handles, the bridge opens the file in the server, asks
//! for document symbols, and asks the call hierarchy for each function's
//! outgoing calls. Symbols become map symbols (their `detail`, usually the
//! signature, becomes the shape) and calls become `calls` edges with the
//! confidence `compiler`. Language servers often run code from the
//! repository, so Onus starts them only in trusted mode, in a sandbox; the
//! command this adapter receives is already wrapped.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use globset::GlobSet;
use onus_core::hash::short_hash;
use onus_core::ids::symbol_id;
use onus_core::{
    Component, Confidence, ContractShape, Edge, EdgeKind, LanguageAdapter, Loc, PartialMap,
    ProviderError, ShapeKind, Site, SourceFile, SymbolKind, SymbolNode, Visibility, Workspace,
};
use serde_json::{Value, json};
use url::Url;

#[derive(Debug)]
pub struct LspAdapter {
    name: String,
    argv: Vec<String>,
    files: GlobSet,
    language_id: String,
    timeout: Duration,
}

impl LspAdapter {
    pub fn new(
        name: String,
        argv: Vec<String>,
        files: GlobSet,
        language_id: String,
        timeout: Duration,
    ) -> Self {
        LspAdapter {
            name,
            argv,
            files,
            language_id,
            timeout,
        }
    }
}

impl LanguageAdapter for LspAdapter {
    fn id(&self) -> &str {
        &self.name
    }

    fn version(&self) -> String {
        format!("lsp ({})", self.argv.last().cloned().unwrap_or_default())
    }

    fn handles(&self, path: &str) -> bool {
        self.files.is_match(path)
    }

    fn build(&self, ws: &Workspace) -> Result<PartialMap, ProviderError> {
        let mut session = Session::start(&self.argv, &ws.root, self.timeout)?;
        let result = map_files(&mut session, ws, &self.language_id);
        session.stop();
        result
    }
}

/// A running language server.
struct Session {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: i64,
    deadline: Instant,
    root: PathBuf,
    root_canonical: PathBuf,
    /// Work-done progress the server has begun and not yet ended.
    busy: BTreeSet<String>,
    /// rust-analyzer's `experimental/serverStatus`: quiescent or not.
    quiescent: Option<bool>,
    last_activity: Instant,
}

impl Session {
    fn start(argv: &[String], root: &Path, timeout: Duration) -> Result<Self, ProviderError> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| ProviderError::Failed("empty command".into()))?;
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| ProviderError::Failed(format!("cannot start `{program}`: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProviderError::Failed("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::Failed("no stdout".into()))?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(msg) = read_message(&mut reader) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        let root_canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Ok(Session {
            child,
            stdin,
            messages: rx,
            next_id: 1,
            deadline: Instant::now() + timeout,
            root: root.to_path_buf(),
            root_canonical,
            busy: BTreeSet::new(),
            quiescent: None,
            last_activity: Instant::now(),
        })
    }

    fn send(&mut self, msg: &Value) -> Result<(), ProviderError> {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| ProviderError::Failed(format!("language server closed its input: {e}")))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), ProviderError> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Notes progress and answers the server's own requests, so it never
    /// stalls waiting for the client.
    fn handle(&mut self, msg: &Value) -> Result<(), ProviderError> {
        match msg.get("method").and_then(Value::as_str) {
            Some("$/progress") => {
                let token = msg["params"]["token"].to_string();
                match msg["params"]["value"]["kind"].as_str() {
                    Some("begin") => {
                        self.busy.insert(token);
                    }
                    Some("end") => {
                        self.busy.remove(&token);
                    }
                    _ => {}
                }
                self.last_activity = Instant::now();
            }
            Some("experimental/serverStatus") => {
                self.quiescent = msg["params"]["quiescent"].as_bool();
                self.last_activity = Instant::now();
            }
            _ => {}
        }
        if let (Some(server_id), Some(server_method)) = (msg.get("id"), msg.get("method")) {
            let result = if server_method == "workspace/configuration" {
                let n = msg["params"]["items"].as_array().map_or(0, Vec::len);
                Value::Array(vec![Value::Null; n])
            } else {
                Value::Null
            };
            self.send(&json!({ "jsonrpc": "2.0", "id": server_id, "result": result }))?;
        }
        Ok(())
    }

    /// Waits until the server has finished loading the project: no
    /// progress under way, and quiet for a moment. Servers that report no
    /// progress are given `min`.
    fn wait_until_idle(&mut self, min: Duration, settle: Duration) -> Result<(), ProviderError> {
        let start = Instant::now();
        loop {
            let now = Instant::now();
            if now >= self.deadline {
                return Err(ProviderError::Failed(
                    "language server did not finish loading the project in time".into(),
                ));
            }
            let busy = !self.busy.is_empty() || self.quiescent == Some(false);
            if !busy && now - start >= min && now - self.last_activity >= settle {
                return Ok(());
            }
            match self.messages.recv_timeout(Duration::from_millis(50)) {
                Ok(msg) => self.handle(&msg)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ProviderError::Failed("language server exited".into()));
                }
            }
        }
    }

    /// Sends a request and waits for its response, answering the server's
    /// own requests meanwhile so it never stalls.
    fn request(&mut self, method: &str, params: Value) -> Result<Value, ProviderError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            let msg = match self.messages.recv_timeout(left) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(ProviderError::Failed("language server timed out".into()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ProviderError::Failed("language server exited".into()));
                }
            };
            if msg.get("id") == Some(&json!(id)) && msg.get("method").is_none() {
                if let Some(err) = msg.get("error") {
                    return Err(ProviderError::Failed(format!("`{method}` failed: {err}")));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            self.handle(&msg)?;
        }
    }

    fn uri(&self, rel: &str) -> String {
        Url::from_file_path(onus_core::paths::native(&self.root, rel))
            .map(|u| u.to_string())
            .unwrap_or_default()
    }

    /// The tree-relative path of a `file://` URI, if it is inside the tree.
    fn rel(&self, uri: &str) -> Option<String> {
        let path = Url::parse(uri).ok()?.to_file_path().ok()?;
        let path = path.canonicalize().unwrap_or(path);
        let rel = path
            .strip_prefix(&self.root_canonical)
            .or_else(|_| path.strip_prefix(&self.root))
            .ok()?;
        Some(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }

    fn stop(mut self) {
        if self.request("shutdown", Value::Null).is_ok() {
            let _ = self.notify("exit", Value::Null);
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            length = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// A symbol the server reported, before it becomes a map symbol.
struct Found {
    qualified: String,
    kind: SymbolKind,
    detail: Option<String>,
    start: u32,
    end: u32,
    /// Position of the name, for the call hierarchy.
    select: (u64, u64),
}

/// LSP `SymbolKind` numbers.
fn symbol_kind(k: u64) -> Option<SymbolKind> {
    Some(match k {
        5 | 23 => SymbolKind::Class,
        6 | 9 => SymbolKind::Method,
        10 => SymbolKind::Enum,
        11 => SymbolKind::Interface,
        12 => SymbolKind::Function,
        13 => SymbolKind::Variable,
        14 => SymbolKind::Const,
        26 => SymbolKind::Type,
        _ => return None,
    })
}

fn line_of(v: &Value) -> u32 {
    v["line"].as_u64().unwrap_or(0) as u32 + 1
}

/// Hierarchical `DocumentSymbol`s: top-level declarations and the members of
/// types; locals inside functions are skipped.
fn collect(symbols: &[Value], prefix: Option<&str>, out: &mut Vec<Found>) {
    for s in symbols {
        let Some(name) = s["name"].as_str() else {
            continue;
        };
        let k = s["kind"].as_u64().unwrap_or(0);
        let range = if s.get("range").is_some() {
            &s["range"]
        } else {
            &s["location"]["range"]
        };
        let select = if s.get("selectionRange").is_some() {
            &s["selectionRange"]["start"]
        } else {
            &range["start"]
        };
        let qualified = match prefix {
            Some(p) => format!("{p}.{name}"),
            None => name.to_string(),
        };
        // Modules and namespaces group symbols without being one.
        if matches!(k, 2..=4) {
            if let Some(children) = s["children"].as_array() {
                collect(children, prefix, out);
            }
            continue;
        }
        let Some(kind) = symbol_kind(k) else {
            continue;
        };
        let kind = if prefix.is_some() && kind == SymbolKind::Function {
            SymbolKind::Method
        } else {
            kind
        };
        out.push(Found {
            qualified: qualified.clone(),
            kind,
            detail: s["detail"]
                .as_str()
                .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|d| !d.is_empty()),
            start: line_of(&range["start"]),
            end: line_of(&range["end"]),
            select: (
                select["line"].as_u64().unwrap_or(0),
                select["character"].as_u64().unwrap_or(0),
            ),
        });
        if matches!(
            kind,
            SymbolKind::Class | SymbolKind::Interface | SymbolKind::Enum
        ) && let Some(children) = s["children"].as_array()
        {
            collect(children, Some(&qualified), out);
        }
    }
}

fn component_dir(c: &Component) -> String {
    c.roots
        .first()
        .map(|r| {
            r.split('/')
                .take_while(|s| !s.contains(['*', '?', '[', '{']))
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

fn map_files(
    s: &mut Session,
    ws: &Workspace,
    language_id: &str,
) -> Result<PartialMap, ProviderError> {
    let root_uri = s.uri("");
    let init = s.request(
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
            "capabilities": {
                "textDocument": {
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "callHierarchy": {}
                },
                "window": { "workDoneProgress": true },
                "experimental": { "serverStatusNotification": true }
            }
        }),
    )?;
    s.notify("initialized", json!({}))?;
    let calls_supported = !init["capabilities"]["callHierarchyProvider"].is_null()
        && init["capabilities"]["callHierarchyProvider"] != json!(false);

    let dirs: BTreeMap<&str, String> = ws
        .components
        .iter()
        .map(|c| (c.id.as_str(), component_dir(c)))
        .collect();
    let comp = |f: &onus_core::WorkspaceFile| f.component.clone().unwrap_or_else(|| "root".into());
    let rel = |f: &onus_core::WorkspaceFile| -> String {
        let c = comp(f);
        match dirs.get(c.as_str()) {
            Some(d) if !d.is_empty() => f
                .path
                .strip_prefix(&format!("{d}/"))
                .unwrap_or(&f.path)
                .to_string(),
            _ => f.path.clone(),
        }
    };

    // Open every file, let the server load the project, then ask.
    let mut texts: BTreeMap<String, (Vec<u8>, String)> = BTreeMap::new();
    for f in &ws.files {
        let bytes = std::fs::read(onus_core::paths::native(&ws.root, &f.path)).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let uri = s.uri(&f.path);
        s.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": language_id, "version": 1, "text": text } }),
        )?;
        texts.insert(f.path.clone(), (bytes, text));
    }
    s.wait_until_idle(Duration::from_millis(500), Duration::from_millis(300))?;

    let mut out = PartialMap::default();
    let mut found: BTreeMap<String, Vec<Found>> = BTreeMap::new();
    for f in &ws.files {
        let (bytes, text) = texts.remove(&f.path).unwrap_or_default();
        let uri = s.uri(&f.path);
        let result = s.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        )?;
        let mut symbols = Vec::new();
        if let Some(list) = result.as_array() {
            collect(list, None, &mut symbols);
        }
        out.files.push(SourceFile {
            path: f.path.clone(),
            component_id: Some(comp(f)),
            language: language_id.to_string(),
            is_test: f.is_test,
            lines: text.lines().count() as u32,
            content_hash: short_hash(&bytes),
            imports: vec![],
            external_apis: vec![],
        });
        found.insert(f.path.clone(), symbols);
    }

    // Where each symbol's name is, to recognize call targets.
    let by_file: BTreeMap<&str, &onus_core::WorkspaceFile> =
        ws.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let id_of = |file: &str, qualified: &str| -> Option<String> {
        let f = by_file.get(file)?;
        Some(symbol_id(&comp(f), &rel(f), qualified))
    };
    let mut targets: BTreeMap<(String, u32), String> = BTreeMap::new();
    for (file, symbols) in &found {
        for sym in symbols {
            if let Some(id) = id_of(file, &sym.qualified) {
                targets.insert((file.clone(), sym.select.0 as u32 + 1), id);
            }
        }
    }

    let mut edges: BTreeMap<(String, String), BTreeSet<Site>> = BTreeMap::new();
    let mut used_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if calls_supported {
        for (file, symbols) in &found {
            if by_file.get(file.as_str()).is_some_and(|f| f.is_test) {
                continue;
            }
            for sym in symbols
                .iter()
                .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
            {
                let Some(from) = id_of(file, &sym.qualified) else {
                    continue;
                };
                let items = s.request(
                    "textDocument/prepareCallHierarchy",
                    json!({
                        "textDocument": { "uri": s.uri(file) },
                        "position": { "line": sym.select.0, "character": sym.select.1 }
                    }),
                )?;
                let Some(item) = items.as_array().and_then(|a| a.first()).cloned() else {
                    continue;
                };
                let calls = s.request("callHierarchy/outgoingCalls", json!({ "item": item }))?;
                for call in calls.as_array().into_iter().flatten() {
                    let to = &call["to"];
                    let Some(target_file) = to["uri"].as_str().and_then(|u| s.rel(u)) else {
                        continue;
                    };
                    let line = line_of(&to["selectionRange"]["start"]);
                    let Some(target) = targets.get(&(target_file.clone(), line)) else {
                        continue;
                    };
                    if *target == from {
                        continue;
                    }
                    let sites = edges.entry((from.clone(), target.clone())).or_default();
                    for r in call["fromRanges"].as_array().into_iter().flatten() {
                        sites.insert(Site {
                            file: file.clone(),
                            line: line_of(&r["start"]),
                        });
                    }
                    if let Some(f) = by_file.get(file.as_str()) {
                        used_by.entry(target.clone()).or_default().insert(comp(f));
                    }
                }
            }
        }
    }
    out.edges = edges
        .into_iter()
        .filter(|(_, sites)| !sites.is_empty())
        .map(|((from, to), sites)| Edge {
            from,
            to,
            kind: EdgeKind::Calls,
            confidence: Confidence::Compiler,
            sites: sites.into_iter().collect(),
        })
        .collect();
    for (file, symbols) in &found {
        let Some(f) = by_file.get(file.as_str()) else {
            continue;
        };
        if f.is_test {
            continue;
        }
        let own = comp(f);
        for sym in symbols {
            let Some(id) = id_of(file, &sym.qualified) else {
                continue;
            };
            let public = used_by
                .get(&id)
                .is_some_and(|c| c.iter().any(|c| *c != own));
            out.symbols.push(SymbolNode {
                visibility: if public {
                    Visibility::Public
                } else {
                    Visibility::Internal
                },
                id,
                component_id: Some(own.clone()),
                kind: sym.kind,
                name: sym.qualified.clone(),
                shape: sym.detail.clone().map(|d| ContractShape {
                    kind: ShapeKind::Alias,
                    type_params: None,
                    params: vec![],
                    returns: None,
                    members: vec![],
                    type_text: Some(d),
                    unverified: false,
                }),
                body_fingerprint: None,
                invariants: vec![],
                loc: Some(Loc {
                    file: file.clone(),
                    start: sym.start,
                    end: sym.end.max(sym.start),
                    signature_end: None,
                }),
                facts: None,
                literal: None,
            });
        }
    }
    out.symbols.sort_by(|a, b| a.id.cmp(&b.id));
    out.symbols.dedup_by(|a, b| a.id == b.id);
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
