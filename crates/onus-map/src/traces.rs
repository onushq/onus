//! Runtime traces as map facts (ADR 0010). OpenTelemetry spans in OTLP JSON
//! that carry code attributes (`code.filepath`, `code.function`,
//! `code.namespace`) resolve to symbols; a span's parent is its caller, so
//! each resolved parent and child become a `calls` edge with confidence
//! `traced`. Spans of another service whose name is a component tie the
//! caller to that component's code. Traced edges are only added: an edge
//! static analysis found stays, confirmed or not.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
    /// Caller and callee pairs not already in the map, added as traced edges.
    pub edges_added: u32,
    /// Pairs the map already had from static analysis.
    pub edges_confirmed: u32,
}

#[derive(Debug, Clone)]
struct Span {
    id: String,
    parent: String,
    service: String,
    file: Option<String>,
    function: Option<String>,
    namespace: Option<String>,
    line: Option<u32>,
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
                    service: service.clone(),
                    file: attr_str(&attrs, "code.filepath"),
                    function: attr_str(&attrs, "code.function"),
                    namespace: attr_str(&attrs, "code.namespace"),
                    line: attr_int(&attrs, "code.lineno"),
                });
            }
        }
    }
    out
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

/// Adds traced edges from OTLP JSON files to the map.
pub fn add_traced_edges(
    map: &mut CodebaseMap,
    paths: &[impl AsRef<Path>],
) -> Result<TraceStats, String> {
    let mut all = Vec::new();
    for p in paths {
        let p = p.as_ref();
        let text = std::fs::read_to_string(p)
            .map_err(|e| format!("cannot read traces {}: {e}", p.display()))?;
        // A file is one document, or one document per line (the collector's
        // file exporter writes JSON lines).
        match serde_json::from_str::<Value>(&text) {
            Ok(doc) => all.extend(spans(&doc)),
            Err(_) => {
                for line in text.lines().filter(|l| !l.trim().is_empty()) {
                    let doc: Value = serde_json::from_str(line)
                        .map_err(|e| format!("{} is not OTLP JSON: {e}", p.display()))?;
                    all.extend(spans(&doc));
                }
            }
        }
    }
    let files: BTreeSet<&str> = map.files.iter().map(|f| f.path.as_str()).collect();
    let mut by_file: BTreeMap<&str, Vec<&SymbolNode>> = BTreeMap::new();
    for s in &map.symbols {
        if let Some(l) = &s.loc {
            by_file.entry(l.file.as_str()).or_default().push(s);
        }
    }
    let components: BTreeSet<&str> = map.components.iter().map(|c| c.id.as_str()).collect();
    let resolve = |s: &Span| -> Option<(String, String, u32)> {
        let file = map_file(&files, s.file.as_deref()?)?;
        let function = s.function.as_deref()?;
        let qualified = s.namespace.as_ref().map(|n| {
            let class = n.rsplit(['.', ':']).next().unwrap_or(n);
            format!("{class}.{function}")
        });
        let symbol = by_file.get(file)?.iter().find(|sym| {
            Some(&sym.name) == qualified.as_ref()
                || sym.name == function
                || sym.name.ends_with(&format!(".{function}"))
        })?;
        let line = s
            .line
            .or_else(|| symbol.loc.as_ref().map(|l| l.start))
            .unwrap_or(1);
        Some((symbol.id.clone(), file.to_string(), line))
    };
    let by_id: BTreeMap<&str, &Span> = all.iter().map(|s| (s.id.as_str(), s)).collect();
    let existing: BTreeSet<(String, String)> = map
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    let mut stats = TraceStats {
        spans: all.len() as u32,
        ..TraceStats::default()
    };
    let mut new: BTreeMap<(String, String), BTreeSet<Site>> = BTreeMap::new();
    for s in &all {
        let callee = resolve(s);
        if callee.is_some() {
            stats.resolved += 1;
        }
        let Some(parent) = by_id.get(s.parent.as_str()) else {
            continue;
        };
        let Some((caller, file, line)) = resolve(parent) else {
            continue;
        };
        // The callee: a symbol, or a component when the span belongs to
        // another service named like one of the map's components.
        let to = match callee {
            Some((id, _, _)) => id,
            None if s.service != parent.service && components.contains(s.service.as_str()) => {
                s.service.clone()
            }
            None => continue,
        };
        if to == caller {
            continue;
        }
        if existing.contains(&(caller.clone(), to.clone())) {
            stats.edges_confirmed += 1;
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
        assert_eq!(s[0].service, "orders");
        assert_eq!(s[0].line, Some(12));
        assert_eq!(s[1].parent, "a");
        let files: BTreeSet<&str> = ["services/orders/src/ship.ts", "src/ship.ts"]
            .into_iter()
            .collect();
        assert_eq!(
            map_file(&files, "/app/services/orders/src/ship.ts"),
            Some("services/orders/src/ship.ts")
        );
    }
}
