//! Read-scope filtering for answers that describe code: the MCP server with
//! a task token drops everything about files the task may not read.

use serde_json::Value;

/// The repository path a value names, if it names one: a plain path
/// (`services/billing/src/a.ts`) or a map id (`billing:src/a.ts#charge`,
/// whose path is relative to its component and resolved by `resolve`).
fn path_in(s: &str) -> Option<&str> {
    if s.contains(' ') || s.is_empty() {
        return None;
    }
    if s.contains('/') || s.ends_with(".ts") || s.ends_with(".tsx") || s.ends_with(".js") {
        return Some(s);
    }
    None
}

/// Removes, recursively, array items that are objects with an unreadable
/// `file` or `path`, and array items that are unreadable paths. Returns how
/// many items it removed. `readable` decides for repository paths;
/// `id_path` turns a map id into a repository path when it can.
pub fn filter_by_read(
    value: &mut Value,
    readable: &dyn Fn(&str) -> bool,
    id_path: &dyn Fn(&str) -> Option<String>,
) -> usize {
    let hidden = |v: &Value| -> bool {
        let named = |s: &str| -> bool {
            if let Some(p) = id_path(s) {
                return !readable(&p);
            }
            path_in(s).is_some_and(|p| !readable(p))
        };
        match v {
            Value::Object(m) => ["file", "path"]
                .iter()
                .filter_map(|k| m.get(*k).and_then(Value::as_str))
                .any(named),
            Value::String(s) => named(s),
            _ => false,
        }
    };
    let mut removed = 0;
    match value {
        Value::Array(items) => {
            let before = items.len();
            items.retain(|v| !hidden(v));
            removed += before - items.len();
            for v in items.iter_mut() {
                removed += filter_by_read(v, readable, id_path);
            }
        }
        Value::Object(map) => {
            for v in map.values_mut() {
                removed += filter_by_read(v, readable, id_path);
            }
        }
        _ => {}
    }
    removed
}

/// Whether an answer is about one unreadable file as a whole (a symbol or a
/// file lookup), which then must not be answered at all.
pub fn about_unreadable(value: &Value, readable: &dyn Fn(&str) -> bool) -> bool {
    let Value::Object(m) = value else {
        return false;
    };
    ["file", "path"]
        .iter()
        .filter_map(|k| m.get(*k).and_then(Value::as_str))
        .any(|p| !readable(p))
        || m.get("symbol")
            .is_some_and(|s| about_unreadable(s, readable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_what_the_scope_cannot_read() {
        let readable = |p: &str| p.starts_with("services/notifications/");
        let id_path = |s: &str| -> Option<String> {
            let (comp, rest) = s.split_once(':')?;
            let path = rest.split('#').next()?;
            Some(format!("services/{comp}/{path}"))
        };
        let mut v = serde_json::json!({
            "links": [
                {"id": "notifications:src/a.ts#x", "file": "services/notifications/src/a.ts"},
                {"id": "billing:src/b.ts#y", "file": "services/billing/src/b.ts"}
            ],
            "importedBy": ["services/notifications/src/c.ts", "services/billing/src/d.ts"],
            "resolved": ["billing:src/b.ts#y"],
            "total": 2
        });
        let removed = filter_by_read(&mut v, &readable, &id_path);
        assert_eq!(removed, 3);
        assert_eq!(v["links"].as_array().unwrap().len(), 1);
        assert_eq!(
            v["importedBy"],
            serde_json::json!(["services/notifications/src/c.ts"])
        );
        assert!(about_unreadable(
            &serde_json::json!({"symbol": {"file": "services/billing/src/b.ts"}}),
            &readable
        ));
    }
}
