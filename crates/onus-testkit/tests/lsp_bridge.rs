//! The LSP bridge against a small fake language server (`onus-fake-lsp`).

use std::path::PathBuf;
use std::time::Duration;

use globset::{Glob, GlobSetBuilder};
use onus_core::{
    Component, ComponentKind, Confidence, EdgeKind, LanguageAdapter, Visibility, Workspace,
    WorkspaceFile,
};
use onus_lang_lsp::LspAdapter;

fn fake_server() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_onus-fake-lsp"))
}

fn component(id: &str) -> Component {
    Component {
        id: id.into(),
        kind: ComponentKind::Package,
        roots: vec![format!("{id}/**")],
        public_entrypoints: vec![],
        owners: vec![],
        labels: vec![],
        package_name: None,
        confidence: Confidence::Inferred,
    }
}

#[test]
fn symbols_and_calls_come_from_the_server() {
    let dir = tempfile::tempdir().unwrap();
    for (p, t) in [
        ("lib/math.toy", "fn add\n  return\nfn unused\n"),
        (
            "app/main.toy",
            "fn main\n  call add\n  call helper\nfn helper\n",
        ),
    ] {
        let f = dir.path().join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
    let mut globs = GlobSetBuilder::new();
    globs.add(Glob::new("**/*.toy").unwrap());
    let adapter = LspAdapter::new(
        "toy".into(),
        vec![fake_server().to_string_lossy().into_owned()],
        globs.build().unwrap(),
        "toy".into(),
        Duration::from_secs(20),
    );
    let wf = |p: &str, c: &str| WorkspaceFile {
        path: p.into(),
        component: Some(c.into()),
        is_test: false,
    };
    let ws = Workspace {
        root: dir.path().to_path_buf(),
        files: vec![wf("app/main.toy", "app"), wf("lib/math.toy", "lib")],
        components: vec![component("app"), component("lib")],
        packages: Default::default(),
        extractors: Default::default(),
    };
    let m = adapter.build(&ws).unwrap();
    let ids: Vec<(&str, Visibility)> = m
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s.visibility))
        .collect();
    assert_eq!(
        ids,
        [
            ("app:main.toy#helper", Visibility::Internal),
            ("app:main.toy#main", Visibility::Internal),
            ("lib:math.toy#add", Visibility::Public),
            ("lib:math.toy#unused", Visibility::Internal),
        ]
    );
    assert_eq!(
        m.symbols[2].shape.as_ref().unwrap().type_text.as_deref(),
        Some("fn add()")
    );
    let edges: Vec<(&str, &str, EdgeKind, Confidence, u32)> = m
        .edges
        .iter()
        .map(|e| {
            (
                e.from.as_str(),
                e.to.as_str(),
                e.kind,
                e.confidence,
                e.sites[0].line,
            )
        })
        .collect();
    assert_eq!(
        edges,
        [
            (
                "app:main.toy#main",
                "app:main.toy#helper",
                EdgeKind::Calls,
                Confidence::Compiler,
                3
            ),
            (
                "app:main.toy#main",
                "lib:math.toy#add",
                EdgeKind::Calls,
                Confidence::Compiler,
                2
            ),
        ]
    );
    assert_eq!(m.files.len(), 2);
}

#[test]
fn a_missing_server_is_a_failure_not_a_hang() {
    let adapter = LspAdapter::new(
        "none".into(),
        vec!["onus-no-such-language-server".into()],
        GlobSetBuilder::new().build().unwrap(),
        "x".into(),
        Duration::from_secs(5),
    );
    let ws = Workspace {
        root: std::env::temp_dir(),
        files: vec![],
        components: vec![],
        packages: Default::default(),
        extractors: Default::default(),
    };
    assert!(matches!(
        adapter.build(&ws),
        Err(onus_core::ProviderError::Failed(_))
    ));
}
