//! The platform sandbox, for real: a plugin that may run repository code
//! works inside it, but cannot write into the tree or reach the network.
//!
//! Needs bubblewrap (Linux) or sandbox-exec (macOS) and network access
//! outside the sandbox, so it is ignored by default; CI's "Sandbox" jobs run
//! it with `cargo test -p onus-plugin-example --test sandbox -- --ignored`.

use onus_core::SymbolKind;
use onus_map::BuildOptions;
use onus_map::plugin::{PluginSpec, PluginsFile, SandboxSpec, SpecKind};

#[test]
#[ignore = "needs a platform sandbox and network; run by CI's Sandbox jobs"]
fn the_platform_sandbox_allows_reading_and_denies_writes_and_network() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("repo");
    std::fs::create_dir_all(root.join("services/api/src")).unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{ "private": true, "workspaces": ["services/*"] }"#,
    )
    .unwrap();
    std::fs::write(
        root.join("services/api/package.json"),
        r#"{ "name": "@x/api", "main": "src/app.ts" }"#,
    )
    .unwrap();
    std::fs::write(
        root.join("services/api/src/app.ts"),
        "export const app = {} as any;\napp.get('/orders/:id', () => 1);\n",
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_onus-plugin-example");
    let ok = r#"printf '{"protocol":1}'"#;
    let plugin = |name: &str, command: Vec<String>| PluginSpec {
        name: name.into(),
        kind: SpecKind::Facts,
        command,
        files: vec![],
        language_id: None,
        runs_repo_code: Some(true),
        timeout_seconds: Some(60),
    };
    let sh = |script: String| vec!["sh".to_string(), "-c".to_string(), script];
    let plugins = PluginsFile {
        plugins: vec![
            plugin("routes", vec![exe.to_string()]),
            plugin("writer", sh(format!("touch {{root}}/escaped && {ok}"))),
            plugin(
                "network",
                sh(format!(
                    "curl -sS --max-time 10 https://example.org > /dev/null && {ok}"
                )),
            ),
        ],
        sandbox: SandboxSpec::default(),
        ..PluginsFile::default()
    };
    // Network works outside the sandbox, so a failure inside means it was
    // blocked, not that the machine is offline.
    let outside = std::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "10",
            "https://example.org",
            "-o",
            "/dev/null",
        ])
        .status()
        .unwrap();
    assert!(
        outside.success(),
        "this test needs network access outside the sandbox"
    );

    let map = onus_map::build_map(
        &root,
        &BuildOptions {
            plugins,
            trusted: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let notes: Vec<String> = map.diagnostics.iter().map(|d| d.message.clone()).collect();
    // Reading the tree and writing the response works.
    assert!(
        map.symbols.iter().any(|s| s.kind == SymbolKind::HttpRoute),
        "{notes:#?}"
    );
    assert!(map.built_with.providers.contains_key("routes"));
    // Writing into the tree does not.
    assert!(!root.join("escaped").exists());
    assert!(
        !map.built_with.providers.contains_key("writer"),
        "{notes:#?}"
    );
    // Nor does the network.
    assert!(
        !map.built_with.providers.contains_key("network"),
        "{notes:#?}"
    );
    assert_eq!(
        map.diagnostics
            .iter()
            .filter(|d| d.kind == "provider-failed")
            .count(),
        2,
        "{notes:#?}"
    );
}
