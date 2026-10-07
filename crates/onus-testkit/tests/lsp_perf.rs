//! How fast the LSP bridge itself is: 20,000 functions in 2,000 files,
//! against the fake server (which answers instantly), so the time is the
//! bridge's own. Run with
//! `cargo test --release -p onus-testkit --test lsp_perf -- --ignored --nocapture`.
#![allow(clippy::print_stderr)]

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use globset::{Glob, GlobSetBuilder};
use onus_core::{Component, ComponentKind, Confidence, LanguageAdapter, Workspace, WorkspaceFile};
use onus_lang_lsp::LspAdapter;

#[test]
#[ignore = "performance measurement; run explicitly"]
fn bridge_throughput() {
    const FILES: usize = 2_000;
    const FUNCTIONS: usize = 10;
    let dir = tempfile::tempdir().unwrap();
    let mut files = Vec::new();
    for f in 0..FILES {
        let mut text = String::new();
        for k in 0..FUNCTIONS {
            let _ = writeln!(text, "fn f{f}_{k}");
            let _ = writeln!(text, "  call f{}_{}", (f + 1) % FILES, k);
        }
        let path = format!("lib/m{f}.toy");
        std::fs::create_dir_all(dir.path().join("lib")).unwrap();
        std::fs::write(dir.path().join(&path), text).unwrap();
        files.push(WorkspaceFile {
            path,
            component: Some("lib".into()),
            is_test: false,
        });
    }
    let mut globs = GlobSetBuilder::new();
    globs.add(Glob::new("**/*.toy").unwrap());
    let adapter = LspAdapter::new(
        "toy".into(),
        vec![env!("CARGO_BIN_EXE_onus-fake-lsp").into()],
        globs.build().unwrap(),
        "toy".into(),
        Duration::from_secs(1_800),
    );
    let ws = Workspace {
        root: dir.path().to_path_buf(),
        files,
        components: vec![Component {
            id: "lib".into(),
            kind: ComponentKind::Package,
            roots: vec!["lib/**".into()],
            public_entrypoints: vec![],
            owners: vec![],
            labels: vec![],
            package_name: None,
            confidence: Confidence::Inferred,
        }],
        packages: Default::default(),
        extractors: Default::default(),
    };
    let start = Instant::now();
    let m = adapter.build(&ws).unwrap();
    let elapsed = start.elapsed();
    let requests = FILES + 2 * FILES * FUNCTIONS;
    eprintln!(
        "LSP bridge: {} files, {} symbols, {} edges in {elapsed:.2?} ({requests} requests, {:.0} requests/s)",
        FILES,
        m.symbols.len(),
        m.edges.len(),
        requests as f64 / elapsed.as_secs_f64()
    );
    assert_eq!(m.symbols.len(), FILES * FUNCTIONS);
    assert_eq!(m.edges.len(), FILES * FUNCTIONS);
}
