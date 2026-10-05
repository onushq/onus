//! Step 6: configuration files. CI workflows, `onus.yaml`, `CODEOWNERS`,
//! policies and test configuration are also flagged as rules of the game.

use std::collections::BTreeSet;

use onus_core::{ChangeKind, ChangeLevel, Location, SemanticChange};
use serde_json::Value;

use crate::ctx::{Ctx, change, join_and};
use crate::packages::LOCKFILES;
use crate::text::{FileChange, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Class {
    pub subkind: &'static str,
    pub label: &'static str,
    pub rules_of_the_game: bool,
    pub why: &'static str,
}

const RULES_WHY: &str = "Changes the rules every change is checked against; needs a person";

/// Classifies a path as configuration, or `None` for code and docs.
pub fn classify(path: &str) -> Option<Class> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    let rules = |subkind, label| {
        Some(Class {
            subkind,
            label,
            rules_of_the_game: true,
            why: RULES_WHY,
        })
    };
    let config = |subkind, label, why| {
        Some(Class {
            subkind,
            label,
            rules_of_the_game: false,
            why,
        })
    };
    if path.starts_with(".github/workflows/")
        || name == ".gitlab-ci.yml"
        || path.starts_with(".circleci/")
        || name == "Jenkinsfile"
        || name == ".buildkite"
        || path.starts_with(".buildkite/")
    {
        return rules("ci-changed", "CI workflow");
    }
    if name == "onus.yaml" {
        return Some(Class {
            subkind: "onus-config-changed",
            label: "Onus configuration",
            rules_of_the_game: true,
            why: "Changes Onus's declared layer (components, labels, contracts or rules); this report used the base version; needs a person",
        });
    }
    if name == "CODEOWNERS" {
        return rules("codeowners-changed", "Code owners");
    }
    if lower.ends_with(".rego")
        || path.starts_with("policy/")
        || path.starts_with("policies/")
        || path.contains("/policies/")
    {
        return rules("policy-changed", "Policy");
    }
    let stem = lower.split('.').next().unwrap_or("");
    if matches!(
        stem,
        "jest" | "vitest" | "playwright" | "cypress" | "karma" | "mocha"
    ) && (lower.contains(".config.") || lower.starts_with(".mocharc"))
    {
        return rules("test-config-changed", "Test configuration");
    }
    if path.starts_with(".github/") {
        return rules("repo-settings-changed", "Repository settings");
    }
    if name == "package.json" {
        return config(
            "package-manifest-changed",
            "Package manifest",
            "Changes how the package is built, run or published",
        );
    }
    if LOCKFILES.contains(&name) {
        return Some(Class {
            subkind: "lockfile-changed",
            label: "Lockfile",
            rules_of_the_game: false,
            why: "Installed third-party versions change without a manifest change",
        });
    }
    if lower.starts_with(".env") {
        return config(
            "env-changed",
            "Environment config",
            "Runtime configuration; behavior can change without a code change",
        );
    }
    if lower.starts_with("dockerfile")
        || lower.ends_with(".dockerfile")
        || lower.starts_with("docker-compose")
        || lower.starts_with("compose.")
    {
        return config(
            "container-changed",
            "Container config",
            "Changes how the service is built or run",
        );
    }
    if lower.ends_with(".prisma") {
        return config(
            "data-schema-changed",
            "Data schema",
            "Changes the shape of stored data",
        );
    }
    if lower.starts_with("tsconfig") && lower.ends_with(".json") {
        return config(
            "build-config-changed",
            "Build config",
            "Changes how the code is compiled or resolved",
        );
    }
    if [".yaml", ".yml", ".json", ".toml", ".ini", ".properties"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        return config(
            "config-changed",
            "Config",
            "Runtime configuration; behavior can change without a code change",
        );
    }
    None
}

fn file_locations(f: &FileChange) -> Vec<Location> {
    let mut out = Vec::new();
    for h in &f.hunks {
        if let (Some(a), Some(b)) = (h.head_lines.first(), h.head_lines.last()) {
            out.push(Location::head(&f.path, *a, *b));
        } else if let (Some(a), Some(b)) = (h.base_lines.first(), h.base_lines.last()) {
            out.push(Location::base(f.base_path(), *a, *b));
        }
    }
    if out.is_empty() {
        out.push(Location::head(&f.path, 1, 1));
    }
    out
}

const DEPENDENCY_SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// Top-level keys of a `package.json` that changed, other than dependency
/// sections (those are handled as packages).
fn manifest_changes(ctx: &Ctx, f: &FileChange) -> Option<Vec<String>> {
    let read = |root: &std::path::Path, p: &str| -> Option<Value> {
        let text = std::fs::read_to_string(root.join(p)).ok()?;
        serde_json::from_str(&text).ok()
    };
    let base = read(ctx.base_root, f.base_path())?;
    let head = read(ctx.head_root, &f.path)?;
    let (Value::Object(b), Value::Object(h)) = (&base, &head) else {
        return None;
    };
    let keys: BTreeSet<&String> = b.keys().chain(h.keys()).collect();
    Some(
        keys.into_iter()
            .filter(|k| !DEPENDENCY_SECTIONS.contains(&k.as_str()))
            .filter(|k| b.get(*k) != h.get(*k))
            .cloned()
            .collect(),
    )
}

pub fn rows(ctx: &Ctx) -> Vec<SemanticChange> {
    let mut rows = Vec::new();
    for f in &ctx.text.files {
        if f.status == Status::Renamed {
            continue;
        }
        let Some(class) = classify(&f.path) else {
            continue;
        };
        if class.subkind == "lockfile-changed" && ctx.explained.borrow().contains(&f.path) {
            continue;
        }
        let component = ctx.component_of_path(&f.path);
        let mut title = format!("{} `{}` changed", class.label, f.path);
        let mut subkind = class.subkind;
        let mut why = class.why.to_string();
        if f.path.ends_with("package.json") && f.status == Status::Modified {
            match manifest_changes(ctx, f) {
                Some(keys) if keys.is_empty() => {
                    // Only dependencies changed: the package rows explain it.
                    ctx.explain(&f.path);
                    continue;
                }
                Some(keys) => {
                    let keys: Vec<String> = keys.iter().map(|k| format!("`{k}`")).collect();
                    if keys.iter().any(|k| k == "`scripts`") {
                        subkind = "package-scripts-changed";
                        why = "Package scripts run in CI and on developer machines".into();
                    }
                    title = format!("`{}` changes {}", f.path, join_and(&keys));
                }
                None => {}
            }
        }
        match f.status {
            Status::Added => title = format!("{} `{}` added", class.label, f.path),
            Status::Deleted => title = format!("{} `{}` removed", class.label, f.path),
            _ => {}
        }
        let mut row = change(
            if subkind == "lockfile-changed" {
                ChangeKind::Dependency
            } else {
                ChangeKind::Config
            },
            subkind,
            ChangeLevel::Behavior,
            &f.path,
            (component != "root").then_some(component.as_str()),
            class.label,
            title,
            why,
            file_locations(f),
        );
        row.id = format!("{subkind}:{}", f.path);
        row.hints.rules_of_the_game = class.rules_of_the_game;
        row.hints.needs_person = class.rules_of_the_game;
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn classifies_config_and_rules_of_the_game() {
        let sub = |p: &str| classify(p).map(|c| (c.subkind, c.rules_of_the_game));
        assert_eq!(sub(".github/workflows/ci.yml"), Some(("ci-changed", true)));
        assert_eq!(sub("onus.yaml"), Some(("onus-config-changed", true)));
        assert_eq!(
            sub(".github/CODEOWNERS"),
            Some(("codeowners-changed", true))
        );
        assert_eq!(
            sub("services/billing/vitest.config.ts"),
            Some(("test-config-changed", true))
        );
        assert_eq!(
            sub("services/billing/.env.example"),
            Some(("env-changed", false))
        );
        assert_eq!(sub("Dockerfile"), Some(("container-changed", false)));
        assert_eq!(
            sub("prisma/schema.prisma"),
            Some(("data-schema-changed", false))
        );
        assert_eq!(sub("config/app.yaml"), Some(("config-changed", false)));
        assert_eq!(sub("services/billing/src/discount.ts"), None);
        assert_eq!(sub("README.md"), None);
    }
}
