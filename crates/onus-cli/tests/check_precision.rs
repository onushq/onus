//! What `onus_check` and pull request reports say about common changes:
//! precise contract rows, first uses of third-party APIs, users left behind
//! by a breaking change, and where a large component changed.

use std::path::Path;

use onus_core::{SemanticChange, SemanticReport};

fn write(root: &Path, files: &[(String, String)]) {
    for (path, text) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

/// A two-package workspace: `@r/a` exports from `src/index.ts`, `@r/b`
/// uses it.
fn workspace(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = vec![
        (
            "package.json".into(),
            r#"{ "name": "r", "private": true, "workspaces": ["packages/*"] }"#.into(),
        ),
        (
            "packages/a/package.json".into(),
            r#"{ "name": "@r/a", "main": "src/index.ts", "dependencies": { "effect": "2.4.19" } }"#
                .into(),
        ),
        (
            "packages/b/package.json".into(),
            r#"{ "name": "@r/b", "main": "src/index.ts", "dependencies": { "@r/a": "*" } }"#.into(),
        ),
    ];
    files.extend(extra.iter().map(|(p, t)| (p.to_string(), t.to_string())));
    files
}

fn report(base: &[(String, String)], head: &[(String, String)]) -> SemanticReport {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("base"), base);
    write(&dir.path().join("head"), head);
    onus_cli::diff_dirs(
        &dir.path().join("base"),
        &dir.path().join("head"),
        &onus_cli::DiffOptions {
            base_label: "base".into(),
            head_label: "head".into(),
            ..Default::default()
        },
    )
    .unwrap()
    .report
}

fn row<'a>(r: &'a SemanticReport, subkind: &str) -> &'a SemanticChange {
    r.changes
        .iter()
        .find(|c| c.subkind == subkind)
        .unwrap_or_else(|| {
            let all: Vec<&str> = r.changes.iter().map(|c| c.subkind.as_str()).collect();
            panic!("no {subkind} row in {all:?}")
        })
}

const USER: (&str, &str) = (
    "packages/b/src/index.ts",
    "import { PublisherNode, Key } from '@r/a';\nexport const shape = PublisherNode;\nexport const k: Key = 'A';\n",
);

#[test]
fn unannotated_initializers_and_unions_are_compared_precisely() {
    let base = workspace(&[
        (
            "packages/a/src/index.ts",
            "import { Schema } from 'effect';\nexport const PublisherNode = Schema.struct({\n  id: Schema.string,\n  slug: Schema.string,\n});\nexport type Key = 'A' | 'B';\nexport function size() {\n  return 1;\n}\n",
        ),
        USER,
    ]);
    let head = workspace(&[
        (
            "packages/a/src/index.ts",
            "import { Schema } from 'effect';\nexport const PublisherNode = Schema.struct({\n  id: Schema.string,\n  slug: Schema.string,\n  linkedinUrl: Schema.optional(Schema.string),\n});\nexport type Key = 'A' | 'B' | 'C';\nexport function size() {\n  return 2;\n}\n",
        ),
        USER,
    ]);
    let r = report(&base, &head);
    let key = row(&r, "contract-key-added");
    assert_eq!(key.title, "`PublisherNode` gains a `linkedinUrl` key");
    assert!(!key.hints.needs_person);
    let union = row(&r, "contract-union-widened");
    assert_eq!(union.title, "`Key` accepts `\"C\"` too");
    // A changed body with an inferred return type is one quiet note, not a
    // "may have changed" row per symbol.
    let inferred = row(&r, "contract-inferred-changed");
    assert!(inferred.title.contains("`size`"), "{}", inferred.title);
    assert!(!inferred.hints.needs_person);
    assert!(
        !r.changes
            .iter()
            .any(|c| c.subkind == "contract-changed-unverified")
    );
}

#[test]
fn first_uses_of_a_package_api_name_the_pinned_version() {
    let base = workspace(&[
        (
            "packages/a/src/index.ts",
            "import { Effect } from 'effect';\nexport const one = () => Effect.succeed(1);\n",
        ),
        USER,
    ]);
    let head = workspace(&[
        (
            "packages/a/src/index.ts",
            "import { Effect } from 'effect';\nexport const one = () => Effect.succeed(1);\nexport const two = (e: unknown) => Effect.fromEither(e);\n",
        ),
        (
            "packages/a/src/index.spec.ts",
            "import { Effect } from 'effect';\nexport const t = Effect.void;\n",
        ),
        USER,
    ]);
    let r = report(&base, &head);
    let first = row(&r, "external-api-first-use");
    assert_eq!(
        first.title,
        "First use of `Effect.fromEither` from `effect`"
    );
    assert!(
        first.why_it_matters.contains("`effect` 2.4.19"),
        "{}",
        first.why_it_matters
    );
    assert_eq!(first.locations[0].file, "packages/a/src/index.ts");
    assert_eq!(first.locations[0].lines, [3, 3]);
    // Test files are left to the tests; known APIs are not mentioned.
    assert!(!first.title.contains("void") && !first.title.contains("succeed"));
}

#[test]
fn breaking_interface_changes_list_the_users_left_behind() {
    let repo = "export interface Repo {\n  get(): string;\n}\n";
    let repo_with_set =
        "export interface Repo {\n  get(): string;\n  set(value: string): void;\n}\n";
    let users = [
        (
            "packages/b/src/index.ts",
            "import type { Repo } from '@r/a';\nexport const live: Repo = { get: () => 'x' };\n",
        ),
        (
            "packages/b/src/repo.spec.ts",
            "import type { Repo } from '@r/a';\nexport const fake: Repo = { get: () => 'y' };\n",
        ),
    ];
    let mut base_files = vec![("packages/a/src/index.ts", repo)];
    base_files.extend(users);
    let mut head_files = vec![("packages/a/src/index.ts", repo_with_set)];
    head_files.extend(users);
    let r = report(&workspace(&base_files), &workspace(&head_files));
    let breaking = row(&r, "contract-field-added-required");
    assert!(
        breaking.why_it_matters.contains(
            "not changed yet: 1 test file and 1 other file that implement it (`packages/b/src/repo.spec.ts`, `packages/b/src/index.ts`)"
        ),
        "{}",
        breaking.why_it_matters
    );
    assert!(
        breaking
            .locations
            .iter()
            .any(|l| l.file == "packages/b/src/repo.spec.ts")
    );
}

#[test]
fn changes_in_large_components_name_their_folders() {
    let mut base: Vec<(String, String)> = workspace(&[USER]);
    for i in 0..310 {
        let folder = if i % 2 == 0 {
            "orders/core"
        } else {
            "billing/core"
        };
        base.push((
            format!("packages/a/src/{folder}/f{i}.ts"),
            format!("export const v{i} = {i};\n"),
        ));
    }
    let mut head = base.clone();
    for (path, text) in &mut head {
        if path.ends_with("/f0.ts") || path.ends_with("/f2.ts") || path.ends_with("/f1.ts") {
            text.push_str("const extra = 1;\n");
        }
    }
    let r = report(&base, &head);
    let internal = row(&r, "internal-changes");
    assert!(
        internal
            .why_it_matters
            .contains("in `orders/core` (2 files) and `billing/core` (1 file)"),
        "{}",
        internal.why_it_matters
    );
}
