//! Small hand-built maps for the passes' unit tests.

use std::path::Path;

use onus_core::{CodebaseMap, OnusConfig, ids};
use serde_json::{Value, json};

use crate::ctx::Ctx;
use crate::text::TreeDiff;

/// A map with the given parts (`components`, `files`, `symbols`, `edges`,
/// `tests`, …); the others are empty.
pub fn map(parts: Value) -> CodebaseMap {
    let mut m = json!({
        "schemaVersion": 1, "commit": "test",
        "builtWith": { "onus": "test", "providers": {}, "configHash": "inferred" },
        "components": [], "files": [], "symbols": [], "edges": [], "externals": [],
        "packages": [], "tests": [], "rules": [], "diagnostics": [],
    });
    for (k, v) in parts.as_object().expect("an object of map parts") {
        m[k] = v.clone();
    }
    serde_json::from_value(m).expect("a map")
}

/// A package component rooted at `packages/<id>`.
pub fn component(id: &str) -> Value {
    json!({
        "id": id, "kind": "package", "roots": [format!("packages/{id}/**")],
        "publicEntrypoints": [], "owners": [], "labels": [], "confidence": "declared",
    })
}

/// A source file of `packages/<component>/<rel>`; a test when its name says so.
pub fn file(component: &str, rel: &str) -> Value {
    json!({
        "path": format!("packages/{component}/{rel}"), "componentId": component,
        "language": "typescript", "isTest": rel.contains(".test."), "lines": 10,
        "contentHash": "x", "imports": [],
    })
}

/// A function `component:rel#name` in `packages/<component>/<rel>`, lines
/// `start..=start + 2`.
pub fn function(id: &str, start: u32, fingerprint: &str) -> Value {
    let component = ids::component_of(id).unwrap_or("root");
    json!({
        "id": id, "componentId": component, "kind": "function", "name": ids::name_of(id),
        "visibility": "public", "bodyFingerprint": fingerprint,
        "loc": { "file": format!("packages/{component}/{}", ids::path_of(id).unwrap_or("")),
                 "start": start, "end": start + 2 },
    })
}

/// An edge used at `site` (a repository path).
pub fn edge(from: &str, to: &str, site: &str, confidence: &str) -> Value {
    json!({
        "from": from, "to": to, "kind": "calls", "confidence": confidence,
        "sites": [{ "file": site, "line": 1 }],
    })
}

/// A test file of `packages/<component>/<rel>` that exercises `exercises`.
pub fn test(component: &str, rel: &str, exercises: &[&str]) -> Value {
    json!({
        "id": format!("{component}:{rel}"), "componentId": component,
        "file": format!("packages/{component}/{rel}"), "cases": [], "exercises": exercises,
    })
}

/// A diff context over two maps, with no files on disk.
pub fn ctx<'a>(
    base: &'a CodebaseMap,
    head: &'a CodebaseMap,
    text: &'a TreeDiff,
    config: Option<&OnusConfig>,
) -> Ctx<'a> {
    let root = Path::new("/nonexistent");
    Ctx::new(base, head, root, root, text, config)
}
