//! A reference Onus plugin (`onus help plugins`).
//!
//! Onus writes one JSON request to stdin and reads one JSON response from
//! stdout. This plugin answers two kinds of request:
//!
//! - `discovery`: every folder with a `go.mod` is a component, named after
//!   the last part of its module path.
//! - `facts`: Express-style routes (`app.get('/orders/:id', ...)` or
//!   `router.post(...)`) become public `http-route` symbols, so a pull
//!   request that adds or removes a route shows it as a contract change.
//!
//! It only reads files below the root it is given. Anything written in any
//! language that speaks the same JSON works the same way.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::Read;
use std::path::Path;

use onus_core::protocol::{PLUGIN_PROTOCOL_VERSION, PluginKind, PluginRequest, PluginResponse};
use onus_core::{
    Component, ComponentKind, Confidence, ContractShape, Loc, PartialMap, ShapeKind, SymbolKind,
    SymbolNode, Visibility, Workspace,
};

const METHODS: &[&str] = &["get", "post", "put", "patch", "delete"];
const RECEIVERS: &[&str] = &["app", "router", "server"];

fn read(root: &Path, rel: &str) -> String {
    let path = rel
        .split('/')
        .fold(root.to_path_buf(), |p, seg| p.join(seg));
    std::fs::read_to_string(path).unwrap_or_default()
}

fn go_modules(ws: &Workspace) -> Vec<Component> {
    let mut out = Vec::new();
    for f in &ws.files {
        let Some(dir) = f.path.strip_suffix("/go.mod") else {
            continue;
        };
        let text = read(&ws.root, &f.path);
        let module = text
            .lines()
            .find_map(|l| l.strip_prefix("module "))
            .map(str::trim)
            .unwrap_or(dir);
        let id: String = module
            .rsplit('/')
            .next()
            .unwrap_or(module)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        out.push(Component {
            id,
            kind: ComponentKind::Service,
            roots: vec![format!("{dir}/**")],
            public_entrypoints: vec![],
            owners: vec![],
            labels: vec![],
            package_name: Some(module.to_string()),
            confidence: Confidence::Inferred,
        });
    }
    out
}

/// `app.get('/orders/:id', ...)` → `("GET", "/orders/:id")`.
fn route(line: &str) -> Option<(String, String)> {
    for receiver in RECEIVERS {
        for method in METHODS {
            let call = format!("{receiver}.{method}(");
            if let Some(at) = line.find(&call) {
                let rest = line[at + call.len()..].trim_start();
                let quote = rest
                    .chars()
                    .next()
                    .filter(|c| matches!(c, '\'' | '"' | '`'))?;
                let path: String = rest[1..].chars().take_while(|c| *c != quote).collect();
                if path.starts_with('/') {
                    return Some((method.to_uppercase(), path));
                }
            }
        }
    }
    None
}

fn routes(ws: &Workspace, so_far: &PartialMap) -> PartialMap {
    let mut out = PartialMap::default();
    let dirs: std::collections::BTreeMap<&str, String> = ws
        .components
        .iter()
        .map(|c| {
            let dir = c.roots[0].trim_end_matches("/**").to_string();
            (c.id.as_str(), dir)
        })
        .collect();
    // Only files a language provider analyzed, and no tests.
    for file in so_far.files.iter().filter(|f| !f.is_test) {
        let component = file.component_id.clone().unwrap_or_else(|| "root".into());
        let rel = match dirs.get(component.as_str()) {
            Some(d) if !d.is_empty() => file
                .path
                .strip_prefix(&format!("{d}/"))
                .unwrap_or(&file.path),
            _ => file.path.as_str(),
        };
        for (i, line) in read(&ws.root, &file.path).lines().enumerate() {
            let Some((method, path)) = route(line) else {
                continue;
            };
            let name = format!("{method} {path}");
            out.symbols.push(SymbolNode {
                id: format!("{component}:{rel}#{name}"),
                component_id: Some(component.clone()),
                kind: SymbolKind::HttpRoute,
                name: name.clone(),
                visibility: Visibility::Public,
                shape: Some(ContractShape {
                    kind: ShapeKind::Alias,
                    type_params: None,
                    params: vec![],
                    returns: None,
                    members: vec![],
                    type_text: Some(name),
                    unverified: false,
                }),
                body_fingerprint: None,
                invariants: vec![],
                loc: Some(Loc {
                    file: file.path.clone(),
                    start: i as u32 + 1,
                    end: i as u32 + 1,
                    signature_end: None,
                }),
                facts: None,
                literal: None,
            });
        }
    }
    out
}

fn main() {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        eprintln!("cannot read the request");
        std::process::exit(1);
    }
    let request: PluginRequest = match serde_json::from_str(&input) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("invalid request: {e}");
            std::process::exit(1);
        }
    };
    if request.protocol != PLUGIN_PROTOCOL_VERSION {
        eprintln!("unsupported protocol {}", request.protocol);
        std::process::exit(1);
    }
    let mut response = PluginResponse {
        protocol: PLUGIN_PROTOCOL_VERSION,
        version: Some(format!("onus-plugin-example {}", env!("CARGO_PKG_VERSION"))),
        ..PluginResponse::default()
    };
    match request.kind {
        PluginKind::Discovery => response.components = go_modules(&request.workspace),
        PluginKind::Facts => {
            response.map = routes(&request.workspace, &request.map.unwrap_or_default())
        }
        PluginKind::Language => {}
    }
    match serde_json::to_string(&response) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("cannot encode the response: {e}");
            std::process::exit(1);
        }
    }
}
