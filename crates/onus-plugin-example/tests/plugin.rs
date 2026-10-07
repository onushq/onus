//! The reference plugin, run by Onus through a plugins file.

use std::path::Path;

use onus_core::{SymbolKind, Visibility};
use onus_map::BuildOptions;
use onus_map::plugin::load_plugins_file;

fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

fn plugins_file(dir: &Path, extra: &str) -> std::path::PathBuf {
    let exe = env!("CARGO_BIN_EXE_onus-plugin-example").replace('\\', "/");
    let path = dir.join("plugins.yaml");
    std::fs::write(
        &path,
        format!(
            "plugins:\n  - {{ name: go-modules, kind: discovery, command: [\"{exe}\"] }}\n  - {{ name: express-routes, kind: facts, command: [\"{exe}\"] }}\n{extra}"
        ),
    )
    .unwrap();
    path
}

fn tree(dir: &Path) {
    write(
        dir,
        &[
            (
                "package.json",
                r#"{ "private": true, "workspaces": ["services/*"] }"#,
            ),
            (
                "services/api/package.json",
                r#"{ "name": "@x/api", "main": "src/app.ts" }"#,
            ),
            (
                "services/api/src/app.ts",
                "import express from \"express\";\nexport const app = express();\napp.get('/orders/:id', (req, res) => res.json({}));\napp.post(\"/orders\", (req, res) => res.json({}));\n",
            ),
            (
                "services/worker/go.mod",
                "module example.com/shop/worker\n\ngo 1.23\n",
            ),
            ("services/worker/main.go", "package main\n"),
        ],
    );
}

#[test]
fn discovery_and_facts_plugins_extend_the_map() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("repo");
    tree(&root);
    let plugins = load_plugins_file(&plugins_file(work.path(), "")).unwrap();
    let map = onus_map::build_map(
        &root,
        &BuildOptions {
            plugins,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    // `services/worker` has no package.json, so only the Go plugin finds it.
    assert!(
        map.components
            .iter()
            .any(|c| c.id == "worker"
                && c.package_name.as_deref() == Some("example.com/shop/worker"))
    );
    let routes: Vec<(&str, Visibility)> = map
        .symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::HttpRoute)
        .map(|s| (s.id.as_str(), s.visibility))
        .collect();
    assert_eq!(
        routes,
        [
            ("api:src/app.ts#GET /orders/:id", Visibility::Public),
            ("api:src/app.ts#POST /orders", Visibility::Public),
        ]
    );
    assert_eq!(
        map.built_with
            .providers
            .get("express-routes")
            .map(String::as_str),
        Some(concat!("onus-plugin-example ", env!("CARGO_PKG_VERSION")))
    );
    assert!(map.built_with.providers.contains_key("go-modules"));
}

#[test]
fn plugins_that_run_repo_code_are_skipped_without_trusted_mode() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("repo");
    tree(&root);
    let exe = env!("CARGO_BIN_EXE_onus-plugin-example").replace('\\', "/");
    let extra =
        format!("  - {{ name: risky, kind: facts, command: [\"{exe}\"], runs_repo_code: true }}\n");
    let plugins = load_plugins_file(&plugins_file(work.path(), &extra)).unwrap();
    let map = onus_map::build_map(
        &root,
        &BuildOptions {
            plugins,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    assert!(!map.built_with.providers.contains_key("risky"));
    let note = map
        .diagnostics
        .iter()
        .find(|d| d.kind == "provider-skipped")
        .expect("a note that the plugin was skipped");
    assert!(note.message.contains("--trusted"), "{}", note.message);
}

#[test]
fn plugins_get_an_absolute_root_when_onus_is_given_a_relative_one() {
    // Tests run in the package directory; make the path relative to it.
    let work = tempfile::tempdir_in(".").unwrap();
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let absolute = work.path().canonicalize().unwrap().join("repo");
    let root = absolute.strip_prefix(&cwd).unwrap().to_path_buf();
    assert!(root.is_relative());
    tree(&root);
    let plugins = load_plugins_file(&plugins_file(work.path(), "")).unwrap();
    let map = onus_map::build_map(
        &root,
        &BuildOptions {
            plugins,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        map.symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::HttpRoute)
            .count(),
        2
    );
}
