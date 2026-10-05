//! Cross-file resolution: imports with aliases, re-export chains, `export *`,
//! namespace imports, workspace packages, local shadowing and visibility.

use std::collections::BTreeMap;
use std::path::Path;

use onus_core::{
    Component, ComponentKind, Confidence, EdgeKind, LanguageAdapter, PartialMap, Visibility,
    Workspace, WorkspaceFile, WorkspacePackage,
};
use onus_lang_ts::TypeScriptAdapter;

fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

fn component(id: &str, dir: &str, entry: &str) -> Component {
    Component {
        id: id.into(),
        kind: ComponentKind::Package,
        roots: vec![format!("{dir}/**")],
        public_entrypoints: vec![entry.into()],
        owners: vec![],
        labels: vec![],
        package_name: Some(format!("@x/{id}")),
        confidence: Confidence::Declared,
    }
}

fn build(root: &Path, files: &[(&str, &str)]) -> PartialMap {
    write(root, files);
    let components = vec![
        component("lib", "packages/lib", "packages/lib/src/index.ts"),
        component("app", "services/app", "services/app/src/index.ts"),
    ];
    let mut packages = BTreeMap::new();
    for c in &components {
        packages.insert(
            c.package_name.clone().unwrap(),
            WorkspacePackage {
                name: c.package_name.clone().unwrap(),
                component: c.id.clone(),
                dir: c.roots[0].trim_end_matches("/**").into(),
                entrypoints: c.public_entrypoints.clone(),
            },
        );
    }
    let ws_files = files
        .iter()
        .map(|(p, _)| WorkspaceFile {
            path: p.to_string(),
            component: Some(
                if p.starts_with("packages/lib") {
                    "lib"
                } else {
                    "app"
                }
                .into(),
            ),
            is_test: p.contains(".test."),
        })
        .collect();
    let ws = Workspace {
        root: root.to_path_buf(),
        files: ws_files,
        components,
        packages,
        extractors: onus_core::ResolvedExtractors {
            publish_patterns: vec!["bus.publish($EVENT, ...)".into()],
            subscribe_patterns: vec!["bus.subscribe($EVENT, ...)".into()],
            externals: BTreeMap::new(),
            prisma_clients: vec!["prisma".into()],
        },
    };
    TypeScriptAdapter.build(&ws)
}

fn has_edge(m: &PartialMap, from: &str, to: &str, kind: EdgeKind) -> bool {
    m.edges
        .iter()
        .any(|e| e.from == from && e.to == to && e.kind == kind)
}

#[test]
fn resolves_references_across_files_and_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let m = build(
        tmp.path(),
        &[
            (
                "packages/lib/src/index.ts",
                "export * from \"./money\";\nexport { Bus as EventBus } from \"./bus\";\nexport const EVENT = \"Paid\";\n",
            ),
            (
                "packages/lib/src/money.ts",
                "export function format(c: number): string { return String(c); }\nexport function hidden(): void {}\n",
            ),
            (
                "packages/lib/src/bus.ts",
                "export class Bus { publish(name: string): void {} }\n",
            ),
            (
                "packages/lib/src/internal.ts",
                "export function secret(): number { return 1; }\n",
            ),
            (
                "services/app/src/index.ts",
                "import { format as fmt, EventBus, EVENT } from \"@x/lib\";\nimport * as m from \"@x/lib/src/money\";\nimport { secret } from \"@x/lib/src/internal\";\nconst bus = new EventBus();\nexport function run(total: number): string {\n  const format = (x: number) => x;\n  format(1);\n  m.hidden();\n  secret();\n  bus.publish(EVENT);\n  return fmt(total);\n}\n",
            ),
        ],
    );
    let run = "app:src/index.ts#run";
    // Alias through `export *`.
    assert!(has_edge(
        &m,
        run,
        "lib:src/money.ts#format",
        EdgeKind::Calls
    ));
    // Namespace import of a subpath.
    assert!(has_edge(
        &m,
        run,
        "lib:src/money.ts#hidden",
        EdgeKind::Calls
    ));
    assert!(has_edge(
        &m,
        run,
        "lib:src/internal.ts#secret",
        EdgeKind::Calls
    ));
    // Renamed re-export resolves to the class.
    assert!(has_edge(
        &m,
        "app:src/index.ts#bus",
        "lib:src/bus.ts#Bus",
        EdgeKind::Calls
    ));
    // Event name resolved through an imported const.
    assert!(has_edge(&m, run, "event:Paid", EdgeKind::Publishes));
    // The local `format` arrow shadows nothing outside `run`, and its call is
    // not an edge.
    assert_eq!(
        m.edges
            .iter()
            .filter(|e| e.from == run && e.to == "lib:src/money.ts#format")
            .flat_map(|e| &e.sites)
            .count(),
        1
    );
    let vis = |id: &str| m.symbols.iter().find(|s| s.id == id).map(|s| s.visibility);
    assert_eq!(vis("lib:src/money.ts#format"), Some(Visibility::Public));
    assert_eq!(vis("lib:src/money.ts#hidden"), Some(Visibility::Public));
    assert_eq!(vis("lib:src/bus.ts#Bus"), Some(Visibility::Public));
    assert_eq!(vis("lib:src/bus.ts#Bus.publish"), Some(Visibility::Public));
    assert_eq!(
        vis("lib:src/internal.ts#secret"),
        Some(Visibility::Internal)
    );
    assert!(m.diagnostics.is_empty(), "{:?}", m.diagnostics);
}

#[test]
fn reports_unresolved_and_dynamic_code() {
    let tmp = tempfile::tempdir().unwrap();
    let m = build(
        tmp.path(),
        &[
            (
                "packages/lib/src/index.ts",
                "import { x } from \"./missing\";\nexport async function load(p: string): Promise<unknown> { return import(p); }\n",
            ),
            ("services/app/src/index.ts", "export {};\n"),
        ],
    );
    let kinds: Vec<&str> = m.diagnostics.iter().map(|d| d.kind.as_str()).collect();
    assert_eq!(kinds, ["dynamic-import", "unresolved-import"]);
    assert!(
        m.diagnostics
            .iter()
            .all(|d| d.confidence == Confidence::Low)
    );
}
