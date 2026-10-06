//! A tiny language server for testing the LSP bridge. It understands a toy
//! language where `fn name` starts a function and `call target` inside it
//! calls another function, and it answers `documentSymbol`,
//! `prepareCallHierarchy` and `callHierarchy/outgoingCalls`. It also asks
//! the client one question of its own, and, like real servers, reports
//! loading the project as work-done progress; until loading ends it knows
//! no calls.
#![allow(clippy::print_stdout)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};

use serde_json::{Value, json};

fn read(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            length = v.trim().parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(v: &Value) {
    let body = v.to_string();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

/// A function: its name, its line and the `(target, line)` calls inside it.
type Function = (String, u64, Vec<(String, u64)>);

fn functions(text: &str) -> Vec<Function> {
    let mut out: Vec<Function> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(name) = line.strip_prefix("fn ") {
            out.push((name.trim().to_string(), i as u64, vec![]));
        } else if let Some(target) = line.trim().strip_prefix("call ")
            && let Some(f) = out.last_mut()
        {
            f.2.push((target.trim().to_string(), i as u64));
        }
    }
    out
}

fn range(line: u64) -> Value {
    json!({ "start": { "line": line, "character": 3 }, "end": { "line": line, "character": 9 } })
}

fn main() {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut docs: BTreeMap<String, String> = BTreeMap::new();
    let mut loaded_at: Option<std::time::Instant> = None;
    while let Some(msg) = read(&mut reader) {
        let loaded =
            loaded_at.is_some_and(|t| t.elapsed() >= std::time::Duration::from_millis(300));
        let method = msg["method"].as_str().unwrap_or("");
        let id = msg.get("id").cloned();
        let result = match method {
            "initialize" => {
                json!({ "capabilities": { "documentSymbolProvider": true, "callHierarchyProvider": true } })
            }
            "initialized" => {
                send(
                    &json!({ "jsonrpc": "2.0", "id": 9000, "method": "workspace/configuration", "params": { "items": [{}] } }),
                );
                send(
                    &json!({ "jsonrpc": "2.0", "id": 9001, "method": "window/workDoneProgress/create", "params": { "token": "load" } }),
                );
                send(
                    &json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": "load", "value": { "kind": "begin", "title": "Loading" } } }),
                );
                loaded_at = Some(std::time::Instant::now());
                // Report the end of loading from another thread, later.
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    send(
                        &json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": "load", "value": { "kind": "end" } } }),
                    );
                });
                continue;
            }
            "textDocument/didOpen" => {
                let d = &msg["params"]["textDocument"];
                docs.insert(
                    d["uri"].as_str().unwrap_or("").into(),
                    d["text"].as_str().unwrap_or("").into(),
                );
                continue;
            }
            "textDocument/documentSymbol" => {
                let uri = msg["params"]["textDocument"]["uri"].as_str().unwrap_or("");
                let text = docs.get(uri).cloned().unwrap_or_default();
                Value::Array(
                    functions(&text)
                        .iter()
                        .map(|(name, line, calls)| {
                            let end = calls.last().map_or(*line, |c| c.1);
                            json!({
                                "name": name, "kind": 12, "detail": format!("fn {name}()"),
                                "range": { "start": { "line": line, "character": 0 }, "end": { "line": end, "character": 0 } },
                                "selectionRange": range(*line),
                            })
                        })
                        .collect(),
                )
            }
            "textDocument/prepareCallHierarchy" => {
                let uri = msg["params"]["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let line = msg["params"]["position"]["line"].as_u64().unwrap_or(0);
                let text = docs.get(&uri).cloned().unwrap_or_default();
                match functions(&text).into_iter().find(|f| f.1 == line) {
                    Some((name, l, _)) => {
                        json!([{ "name": name, "kind": 12, "uri": uri, "range": range(l), "selectionRange": range(l) }])
                    }
                    None => json!([]),
                }
            }
            "callHierarchy/outgoingCalls" if !loaded => json!([]),
            "callHierarchy/outgoingCalls" => {
                let item = &msg["params"]["item"];
                let uri = item["uri"].as_str().unwrap_or("");
                let line = item["selectionRange"]["start"]["line"]
                    .as_u64()
                    .unwrap_or(0);
                let text = docs.get(uri).cloned().unwrap_or_default();
                let calls = functions(&text)
                    .into_iter()
                    .find(|f| f.1 == line)
                    .map(|f| f.2)
                    .unwrap_or_default();
                let mut out = Vec::new();
                for (target, at) in calls {
                    for (turi, ttext) in &docs {
                        if let Some(f) = functions(ttext).into_iter().find(|f| f.0 == target) {
                            out.push(json!({
                                "to": { "name": target, "kind": 12, "uri": turi, "range": range(f.1), "selectionRange": range(f.1) },
                                "fromRanges": [range(at)],
                            }));
                        }
                    }
                }
                Value::Array(out)
            }
            "shutdown" => Value::Null,
            "exit" => return,
            _ => {
                if id.is_none() {
                    continue;
                }
                Value::Null
            }
        };
        if let Some(id) = id {
            send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    }
}
