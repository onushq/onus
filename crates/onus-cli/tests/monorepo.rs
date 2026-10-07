//! Moves between components and grouped rows, on a small generated
//! monorepo: a refactor that moves shared types into a new library.

use std::path::Path;

use onus_core::SemanticReport;

fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

const TYPES: &str = "export interface Setup { model: string; voice?: string }\nexport interface Message { text: string }\nexport type Role = \"user\" | \"agent\";\n";

fn report() -> SemanticReport {
    report_with(TYPES)
}

fn report_with(head_types: &str) -> SemanticReport {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let head = tmp.path().join("head");
    let common: &[(&str, &str)] = &[
        (
            "package.json",
            r#"{ "private": true, "workspaces": ["packages/*"] }"#,
        ),
        (
            "packages/b/package.json",
            r#"{ "name": "@x/b", "main": "src/index.ts" }"#,
        ),
    ];
    write(&base, common);
    write(&head, common);
    write(
        &base,
        &[
            (
                "packages/a/package.json",
                r#"{ "name": "@x/a", "main": "src/index.ts", "dependencies": { "zod": "^3.0.0", "lodash": "^4.0.0" } }"#,
            ),
            ("packages/a/src/types.ts", TYPES),
            (
                "packages/a/src/index.ts",
                "export * from \"./types\";\nexport function live(): number { return 1; }\n",
            ),
            (
                "packages/b/src/index.ts",
                "import type { Setup } from \"@x/a\";\nexport function start(s: Setup): string { return s.model; }\n",
            ),
        ],
    );
    write(
        &head,
        &[
            (
                "packages/a/package.json",
                r#"{ "name": "@x/a", "main": "src/index.ts" }"#,
            ),
            (
                "packages/a/src/index.ts",
                "export function live(): number { return 1; }\n",
            ),
            (
                "packages/c/package.json",
                r#"{ "name": "@x/c", "main": "src/index.ts", "dependencies": { "zod": "^3.0.0", "lodash": "^4.0.0" } }"#,
            ),
            (
                "packages/c/tsconfig.json",
                "{ \"extends\": \"../../tsconfig.base.json\" }\n",
            ),
            (
                "packages/c/tsconfig.lib.json",
                "{ \"extends\": \"./tsconfig.json\" }\n",
            ),
            ("packages/c/src/types.ts", head_types),
            ("packages/c/src/index.ts", "export * from \"./types\";\n"),
            (
                "packages/b/src/index.ts",
                "import type { Setup } from \"@x/c\";\nexport function start(s: Setup): string { return s.model; }\n",
            ),
        ],
    );
    onus_cli::diff_dirs(&base, &head, &onus_cli::DiffOptions::default())
        .unwrap()
        .report
}

#[test]
fn types_moved_into_a_new_library_are_one_row() {
    let r = report();
    let moved: Vec<_> = r
        .changes
        .iter()
        .filter(|c| c.subkind == "moved-between-components")
        .collect();
    assert_eq!(moved.len(), 1, "{:#?}", r.changes);
    assert_eq!(moved[0].title, "3 symbols moved from `a` to `c`");
    assert!(
        moved[0]
            .why_it_matters
            .contains("`a` no longer exports them")
    );
    // No per-symbol removals or additions for the moved types.
    assert!(
        r.changes
            .iter()
            .all(|c| !matches!(c.subkind.as_str(), "export-removed" | "export-added")),
        "{:#?}",
        r.changes.iter().map(|c| &c.title).collect::<Vec<_>>()
    );
}

#[test]
fn bulk_config_and_dependency_changes_are_grouped() {
    let r = report();
    let titles: Vec<&str> = r.changes.iter().map(|c| c.title.as_str()).collect();
    assert!(
        titles.contains(&"Build config: 2 files changed in `c`"),
        "{titles:#?}"
    );
    assert!(
        titles.contains(&"2 npm packages already used in this repository added in `c`"),
        "{titles:#?}"
    );
    assert!(
        titles.contains(&"2 npm packages dropped in `a`"),
        "{titles:#?}"
    );
}

#[test]
fn types_moved_and_changed_are_still_one_row() {
    let readonly = TYPES
        .replace("{ model", "{ readonly model")
        .replace("{ text", "{ readonly text");
    let r = report_with(&readonly);
    let moved: Vec<_> = r
        .changes
        .iter()
        .filter(|c| c.subkind == "moved-between-components")
        .collect();
    assert_eq!(moved.len(), 1, "{:#?}", r.changes);
    assert_eq!(moved[0].title, "3 symbols moved from `a` to `c`");
    assert!(
        moved[0]
            .why_it_matters
            .contains("1 identical, 2 changed while moving: `Message` (`text` becomes readonly)"),
        "{}",
        moved[0].why_it_matters
    );
    // Becoming readonly can break writers.
    assert_eq!(moved[0].kind, onus_core::ChangeKind::Breaking);
    assert!(r.changes.iter().all(|c| c.subkind != "export-removed"));
}
