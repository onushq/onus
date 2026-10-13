//! Runtime traces as map facts (ADR 0010, ADR 0016). OpenTelemetry spans in
//! OTLP JSON that carry code attributes (`code.filepath`, `code.function`,
//! `code.namespace`) resolve to symbols; a span's parent is its caller, so
//! each resolved parent and child become a `calls` edge with confidence
//! `traced`. Spans of another service whose name is a component tie the
//! caller to that component's code. Production traces kept in the records
//! (`signals.jsonl`, lines of kind `trace`) give the same caller and callee
//! pairs. Traced edges are only added: an edge static analysis found stays.
//! A call the map had only guessed (`inferred` or `low`) that a trace
//! observed becomes `traced`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use onus_core::signal::{CodeRef, ObservedCall};
use onus_core::{CodebaseMap, Confidence, Edge, EdgeKind, Site, SymbolNode};
use serde::Serialize;
use serde_json::Value;

/// What reading traces found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceStats {
    pub spans: u32,
    /// Spans that resolved to a symbol of the map.
    pub resolved: u32,
    /// Caller and callee pairs kept in production trace signals.
    pub calls: u32,
    /// Caller and callee pairs not already in the map, added as traced edges.
    pub edges_added: u32,
    /// Pairs the map already had.
    pub edges_confirmed: u32,
    /// Pairs the map had only guessed (`inferred` or `low`), now `traced`.
    pub edges_upgraded: u32,
}

#[derive(Debug, Clone)]
struct Span {
    id: String,
    parent: String,
    code: CodeRef,
}

fn attr<'a>(attrs: &'a Value, key: &str) -> Option<&'a Value> {
    attrs
        .as_array()?
        .iter()
        .find(|a| a.get("key").and_then(Value::as_str) == Some(key))
        .and_then(|a| a.get("value"))
}

fn attr_str(attrs: &Value, key: &str) -> Option<String> {
    let v = attr(attrs, key)?;
    v.get("stringValue")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn attr_int(attrs: &Value, key: &str) -> Option<u32> {
    let v = attr(attrs, key)?;
    v.get("intValue")
        .and_then(|i| i.as_str().and_then(|s| s.parse().ok()).or(i.as_u64()))
        .and_then(|n| u32::try_from(n).ok())
}

/// The spans of one OTLP JSON document (`{"resourceSpans": [...]}`).
fn spans(doc: &Value) -> Vec<Span> {
    let mut out = Vec::new();
    for rs in doc
        .get("resourceSpans")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let service = rs
            .get("resource")
            .and_then(|r| r.get("attributes"))
            .and_then(|a| attr_str(a, "service.name"))
            .unwrap_or_default();
        for ss in rs
            .get("scopeSpans")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for s in ss
                .get("spans")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let attrs = s.get("attributes").cloned().unwrap_or(Value::Null);
                out.push(Span {
                    id: s
                        .get("spanId")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    parent: s
                        .get("parentSpanId")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    code: CodeRef {
                        service: service.clone(),
                        file: attr_str(&attrs, "code.filepath"),
                        function: attr_str(&attrs, "code.function"),
                        namespace: attr_str(&attrs, "code.namespace"),
                        line: attr_int(&attrs, "code.lineno"),
                    },
                });
            }
        }
    }
    out
}

/// Each span with its parent, as caller and callee. A pair is kept when the
/// caller names its code (file and function), and the callee does too or
/// belongs to another service: what a map can resolve.
fn calls_from_spans(spans: &[Span]) -> Vec<ObservedCall> {
    let by_id: BTreeMap<&str, &Span> = spans.iter().map(|s| (s.id.as_str(), s)).collect();
    let names = |c: &CodeRef| c.file.is_some() && c.function.is_some();
    spans
        .iter()
        .filter_map(|s| {
            let parent = by_id.get(s.parent.as_str())?;
            (names(&parent.code) && (names(&s.code) || s.code.service != parent.code.service)).then(
                || ObservedCall {
                    from: parent.code.clone(),
                    to: s.code.clone(),
                    count: 1,
                },
            )
        })
        .collect()
}

/// What trace files hold: OTLP spans, and calls kept in production signals.
#[derive(Debug, Default)]
struct Read {
    spans: Vec<Span>,
    calls: Vec<ObservedCall>,
}

/// One JSON document: a production signal (`kind` and `id`), whose calls
/// count only for `kind: trace`, or else OTLP, where anything without
/// `resourceSpans` holds no spans.
fn read_doc(doc: &Value, into: &mut Read) -> Result<(), String> {
    if doc.get("resourceSpans").is_none() && doc.get("kind").is_some() && doc.get("id").is_some() {
        if doc["kind"] == "trace" {
            let calls: Vec<ObservedCall> =
                serde_json::from_value(doc.get("calls").cloned().unwrap_or(Value::Array(vec![])))
                    .map_err(|e| format!("trace signal {}: {e}", doc["id"]))?;
            into.calls.extend(calls);
        }
    } else {
        into.spans.extend(spans(doc));
    }
    Ok(())
}

fn read(paths: &[impl AsRef<Path>]) -> Result<Read, String> {
    let mut out = Read::default();
    for p in paths {
        let p = p.as_ref();
        let text = std::fs::read_to_string(p)
            .map_err(|e| format!("cannot read traces {}: {e}", p.display()))?;
        // A file is one document, or one document per line (the collector's
        // file exporter and the records write JSON lines).
        match serde_json::from_str::<Value>(&text) {
            Ok(doc) => read_doc(&doc, &mut out).map_err(|e| format!("{}: {e}", p.display()))?,
            Err(_) => {
                for (i, line) in text.lines().enumerate() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let doc: Value = serde_json::from_str(line).map_err(|e| {
                        format!("line {} of {} is not JSON: {e}", i + 1, p.display())
                    })?;
                    read_doc(&doc, &mut out)
                        .map_err(|e| format!("line {} of {}: {e}", i + 1, p.display()))?;
                }
            }
        }
    }
    Ok(out)
}

/// The caller and callee pairs in trace files: OTLP JSON (one document, or
/// JSON lines of them) and the records' `signals.jsonl`, whose `trace`
/// lines carry calls and whose other lines are skipped.
pub fn observed_calls(paths: &[impl AsRef<Path>]) -> Result<Vec<ObservedCall>, String> {
    let r = read(paths)?;
    let mut calls = calls_from_spans(&r.spans);
    calls.extend(r.calls);
    Ok(calls)
}

/// Finds the map file a span's file path names: the same path, or the
/// longest map path the span's (often absolute) path ends with.
fn map_file<'a>(files: &'a BTreeSet<&str>, path: &str) -> Option<&'a str> {
    let path = path.replace('\\', "/");
    if let Some(f) = files.get(path.as_str()) {
        return Some(f);
    }
    files
        .iter()
        .filter(|f| path.ends_with(&format!("/{f}")))
        .max_by_key(|f| f.len())
        .copied()
}

/// Adds traced edges from trace files (see [`observed_calls`]) to the map,
/// and upgrades the guessed calls they confirm.
pub fn add_traced_edges(
    map: &mut CodebaseMap,
    paths: &[impl AsRef<Path>],
) -> Result<TraceStats, String> {
    let read = read(paths)?;
    let recorded = read.calls.len() as u32;
    let mut calls = calls_from_spans(&read.spans);
    calls.extend(read.calls);
    let files: BTreeSet<&str> = map.files.iter().map(|f| f.path.as_str()).collect();
    let mut by_file: BTreeMap<&str, Vec<&SymbolNode>> = BTreeMap::new();
    for s in &map.symbols {
        if let Some(l) = &s.loc {
            by_file.entry(l.file.as_str()).or_default().push(s);
        }
    }
    let components: BTreeSet<&str> = map.components.iter().map(|c| c.id.as_str()).collect();
    let resolve = |c: &CodeRef| -> Option<(String, String, u32)> {
        let file = map_file(&files, c.file.as_deref()?)?;
        let function = c.function.as_deref()?;
        let qualified = c.namespace.as_ref().map(|n| {
            let class = n.rsplit(['.', ':']).next().unwrap_or(n);
            format!("{class}.{function}")
        });
        let symbol = by_file.get(file)?.iter().find(|sym| {
            Some(&sym.name) == qualified.as_ref()
                || sym.name == function
                || sym.name.ends_with(&format!(".{function}"))
        })?;
        let line = c
            .line
            .or_else(|| symbol.loc.as_ref().map(|l| l.start))
            .unwrap_or(1);
        Some((symbol.id.clone(), file.to_string(), line))
    };
    let mut stats = TraceStats {
        spans: read.spans.len() as u32,
        resolved: read
            .spans
            .iter()
            .filter(|s| resolve(&s.code).is_some())
            .count() as u32,
        calls: recorded,
        ..TraceStats::default()
    };
    let existing: BTreeMap<(String, String), usize> = map
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == EdgeKind::Calls)
        .map(|(i, e)| ((e.from.clone(), e.to.clone()), i))
        .collect();
    let mut new: BTreeMap<(String, String), BTreeSet<Site>> = BTreeMap::new();
    for call in &calls {
        let Some((caller, file, line)) = resolve(&call.from) else {
            continue;
        };
        // The callee: a symbol, or a component when the call went to another
        // service named like one of the map's components.
        let to = match resolve(&call.to) {
            Some((id, _, _)) => id,
            None if call.to.service != call.from.service
                && components.contains(call.to.service.as_str()) =>
            {
                call.to.service.clone()
            }
            None => continue,
        };
        if to == caller {
            continue;
        }
        if let Some(&i) = existing.get(&(caller.clone(), to.clone())) {
            stats.edges_confirmed += 1;
            let edge = &mut map.edges[i];
            if matches!(edge.confidence, Confidence::Inferred | Confidence::Low) {
                edge.confidence = Confidence::Traced;
                stats.edges_upgraded += 1;
            }
            continue;
        }
        new.entry((caller, to))
            .or_default()
            .insert(Site { file, line });
    }
    stats.edges_added = new.len() as u32;
    for ((from, to), sites) in new {
        map.edges.push(Edge {
            from,
            to,
            kind: EdgeKind::Calls,
            confidence: Confidence::Traced,
            sites: sites.into_iter().collect(),
        });
    }
    map.edges
        .sort_by(|a, b| (&a.from, &a.to, a.kind as u8).cmp(&(&b.from, &b.to, b.kind as u8)));
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_otlp_spans() {
        let doc = serde_json::json!({
            "resourceSpans": [{
                "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "orders"}}]},
                "scopeSpans": [{"spans": [
                    {"spanId": "a", "parentSpanId": "", "attributes": [
                        {"key": "code.filepath", "value": {"stringValue": "/app/services/orders/src/ship.ts"}},
                        {"key": "code.function", "value": {"stringValue": "ship"}},
                        {"key": "code.lineno", "value": {"intValue": "12"}}
                    ]},
                    {"spanId": "b", "parentSpanId": "a", "attributes": []}
                ]}]
            }]
        });
        let s = spans(&doc);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].code.service, "orders");
        assert_eq!(s[0].code.line, Some(12));
        assert_eq!(s[1].parent, "a");
        let files: BTreeSet<&str> = ["services/orders/src/ship.ts", "src/ship.ts"]
            .into_iter()
            .collect();
        assert_eq!(
            map_file(&files, "/app/services/orders/src/ship.ts"),
            Some("services/orders/src/ship.ts")
        );
    }

    fn fixture_trace() -> Value {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../onus-cli/tests/otlp/order-placed.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn pair(c: &ObservedCall) -> (String, String) {
        let name = |r: &CodeRef| {
            format!(
                "{}:{}",
                r.service,
                r.function.as_deref().unwrap_or("(no code)")
            )
        };
        (name(&c.from), name(&c.to))
    }

    #[test]
    fn calls_from_spans_pair_each_span_with_its_caller() {
        let calls = calls_from_spans(&spans(&fixture_trace()));
        let pairs: Vec<(String, String)> = calls.iter().map(pair).collect();
        // What add_traced_edges resolved before calls were kept: the bus
        // dispatch, the call within billing, and the call into notifications,
        // whose span names no code but belongs to another service.
        assert_eq!(
            pairs,
            [
                ("orders:placeOrder".into(), "billing:invoiceOrder".into()),
                (
                    "billing:invoiceOrder".into(),
                    "billing:createInvoice".into()
                ),
                ("orders:placeOrder".into(), "notifications:(no code)".into()),
            ]
        );
        assert!(calls.iter().all(|c| c.count == 1));
        assert_eq!(calls[0].from.line, Some(14));
    }

    #[test]
    fn records_give_only_their_trace_lines_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("signals.jsonl");
        let call = r#"{"from":{"service":"orders","file":"src/a.ts","function":"a"},"to":{"service":"orders","file":"src/b.ts","function":"b"},"count":4}"#;
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                r#"{"id":"pd:1","kind":"incident","components":["orders"]}"#,
                format_args!(r#"{{"id":"trace:1","kind":"trace","calls":[{call}]}}"#)
            ),
        )
        .unwrap();
        let calls = observed_calls(&[&path]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].count, 4);
        assert_eq!(calls[0].to.function.as_deref(), Some("b"));
        // A document that is neither holds nothing, as before signals; a
        // line that is not JSON is an error naming it.
        std::fs::write(&path, "{\"hello\":1}\n[]\n").unwrap();
        assert!(observed_calls(&[&path]).unwrap().is_empty());
        std::fs::write(&path, "{\"hello\":1}\nnot json\n").unwrap();
        let err = observed_calls(&[&path]).unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    fn map(edges: &[(&str, &str, &str)]) -> CodebaseMap {
        let file = |p: &str| {
            serde_json::json!({"path": p, "language": "typescript", "isTest": false,
                "lines": 10, "contentHash": "h", "imports": []})
        };
        let symbol = |name: &str| {
            serde_json::json!({"id": format!("app:src/{name}.ts#{name}"), "kind": "function",
                "name": name, "visibility": "public",
                "loc": {"file": format!("src/{name}.ts"), "start": 1, "end": 3}})
        };
        let edges: Vec<Value> = edges
            .iter()
            .map(|(from, to, confidence)| {
                serde_json::json!({"from": format!("app:src/{from}.ts#{from}"),
                    "to": format!("app:src/{to}.ts#{to}"), "kind": "calls",
                    "confidence": confidence, "sites": []})
            })
            .collect();
        let files = ["a", "b", "c"].map(|n| file(&format!("src/{n}.ts")));
        let symbols = ["a", "b", "c"].map(symbol);
        serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "commit": "x",
            "builtWith": {"onus": "x", "providers": {}, "configHash": "x"},
            "components": [],
            "files": files,
            "symbols": symbols,
            "edges": edges,
            "externals": [], "packages": [], "tests": [], "rules": [], "diagnostics": []
        }))
        .unwrap()
    }

    #[test]
    fn production_confirms_guessed_calls() {
        let mut m = map(&[("a", "b", "inferred"), ("b", "c", "static")]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("signals.jsonl");
        let call = |from: &str, to: &str| {
            format!(
                r#"{{"from":{{"service":"app","file":"/srv/src/{from}.ts","function":"{from}"}},"to":{{"service":"app","file":"/srv/src/{to}.ts","function":"{to}"}}}}"#
            )
        };
        std::fs::write(
            &path,
            format!(
                r#"{{"id":"trace:1","kind":"trace","calls":[{},{},{}]}}"#,
                call("a", "b"),
                call("b", "c"),
                call("c", "a")
            ) + "\n",
        )
        .unwrap();
        let stats = add_traced_edges(&mut m, &[&path]).unwrap();
        assert_eq!(
            (
                stats.edges_confirmed,
                stats.edges_upgraded,
                stats.edges_added
            ),
            (2, 1, 1)
        );
        let confidence = |from: &str, to: &str| {
            m.edges
                .iter()
                .find(|e| {
                    e.from.ends_with(&format!("#{from}")) && e.to.ends_with(&format!("#{to}"))
                })
                .map(|e| e.confidence)
        };
        assert_eq!(confidence("a", "b"), Some(Confidence::Traced));
        assert_eq!(confidence("b", "c"), Some(Confidence::Static));
        assert_eq!(confidence("c", "a"), Some(Confidence::Traced));
        // Seen again: confirmed, already traced.
        let again = add_traced_edges(&mut m, &[&path]).unwrap();
        assert_eq!((again.edges_upgraded, again.edges_added), (0, 0));
    }
}
