//! The Svelte plugin, run by Onus through a plugins file.

use std::path::Path;

use onus_map::BuildOptions;
use onus_map::plugin::{PluginSpec, PluginsFile, SpecKind};

fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

fn plugins() -> PluginsFile {
    PluginsFile {
        plugins: vec![PluginSpec {
            name: "svelte".into(),
            kind: SpecKind::Language,
            command: vec![env!("CARGO_BIN_EXE_onus-plugin-svelte").into()],
            files: vec!["**/*.svelte".into()],
            language_id: None,
            runs_repo_code: None,
            timeout_seconds: Some(60),
        }],
        ..PluginsFile::default()
    }
}

const PAGE: &str = "<script lang=\"ts\">\n  import { fetchOrders } from '$lib/api';\n  import Card from '$lib/Card.svelte';\n  let orders = $state([]);\n  async function load(): Promise<void> {\n    orders = await fetchOrders();\n  }\n</script>\n\n<h1>Orders</h1>\n<Card />\n";

fn tree(root: &Path, page: &str) {
    write(
        root,
        &[
            (
                "package.json",
                r#"{ "private": true, "workspaces": ["apps/*"] }"#,
            ),
            ("apps/web/package.json", r#"{ "name": "@x/web" }"#),
            ("apps/web/svelte.config.js", "export default {};\n"),
            (
                "apps/web/src/lib/api.ts",
                "export async function fetchOrders(): Promise<string[]> {\n  const r = await fetch('https://api.shop.io/orders');\n  return r.json();\n}\n",
            ),
            (
                "apps/web/src/lib/Card.svelte",
                "<script>\n  export let title = '';\n</script>\n<div>{title}</div>\n",
            ),
            ("apps/web/src/routes/+page.svelte", page),
        ],
    );
}

#[test]
fn svelte_scripts_join_the_map() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    tree(&root, PAGE);
    let map = onus_map::build_map(
        &root,
        &BuildOptions {
            plugins: plugins(),
            ..BuildOptions::default()
        },
    )
    .unwrap();
    assert!(
        map.built_with.providers.contains_key("svelte"),
        "{:#?}",
        map.diagnostics
    );
    let page = map
        .files
        .iter()
        .find(|f| f.path == "apps/web/src/routes/+page.svelte")
        .expect("the page is in the map");
    assert_eq!(page.language, "svelte");
    // The call in the page's script, resolved through $lib to the TypeScript
    // module, with its real line number.
    let call = map
        .edges
        .iter()
        .find(|e| {
            e.from == "web:src/routes/+page.svelte#load" && e.to == "web:src/lib/api.ts#fetchOrders"
        })
        .expect("load calls fetchOrders");
    assert_eq!(call.sites[0].line, 6);
    // Symbols come only from Svelte files; TypeScript files are the
    // built-in adapter's.
    assert_eq!(
        map.symbols
            .iter()
            .filter(|s| s.id == "web:src/lib/api.ts#fetchOrders")
            .count(),
        1
    );
}

#[test]
fn a_markup_change_is_not_formatting() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base");
    let head = dir.path().join("head");
    tree(&base, PAGE);
    tree(
        &head,
        &PAGE.replace("<h1>Orders</h1>", "<h1>Your orders</h1>"),
    );
    let opts = BuildOptions {
        plugins: plugins(),
        ..BuildOptions::default()
    };
    let base_map = onus_map::build_map(&base, &opts).unwrap();
    let head_map = onus_map::build_map(&head, &opts).unwrap();
    let hash = |m: &onus_core::CodebaseMap| {
        m.files
            .iter()
            .find(|f| f.path.ends_with("+page.svelte"))
            .map(|f| f.content_hash.clone())
    };
    assert_ne!(hash(&base_map), hash(&head_map));
}
