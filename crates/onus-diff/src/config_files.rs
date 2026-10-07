//! Step 6: configuration files. CI workflows, `onus.yaml`, `CODEOWNERS`,
//! policies and test configuration are also flagged as rules of the game.

use std::collections::{BTreeMap, BTreeSet};

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
    let root_level = !path.contains('/');
    if root_level && is_root_build_config(&lower) {
        return config(
            "root-build-config-changed",
            "Root build config",
            "Changes how every package in the repository is built, resolved or run; needs a person",
        );
    }
    if is_infrastructure(path, &lower) {
        return config(
            "infrastructure-changed",
            "Infrastructure",
            "Changes how production runs: resources, scaling, networking or access; needs a person",
        );
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
    if matches!(
        name,
        "pnpm-workspace.yaml" | ".yarnrc.yml" | ".yarnrc" | "lerna.json"
    ) {
        return config(
            "workspace-config-changed",
            "Workspace config",
            "Changes how the workspace installs and links packages",
        );
    }
    if is_build_config(&lower) {
        return config(
            "build-config-changed",
            "Build config",
            "Changes how the code is compiled, bundled, checked or resolved",
        );
    }
    if [".yaml", ".yml", ".json", ".toml", ".ini", ".properties"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        return config(
            "config-changed",
            "Config",
            "A configuration or data file; what it changes depends on the code that reads it",
        );
    }
    None
}

/// Files at the repository root that set how everything is built or run.
fn is_root_build_config(name: &str) -> bool {
    (name.starts_with("tsconfig") && name.ends_with(".json"))
        || name.starts_with("babel.config.")
        || name.starts_with(".babelrc")
        || matches!(
            name,
            "nx.json" | "turbo.json" | ".npmrc" | ".nvmrc" | ".node-version" | ".tool-versions"
        )
}

/// Build, bundling, lint and code-generation settings.
fn is_build_config(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("");
    (name.starts_with("tsconfig") && name.ends_with(".json"))
        || matches!(
            name,
            "project.json" | "angular.json" | "workspace.json" | "biome.json" | "biome.jsonc"
        )
        || name.starts_with(".eslintrc")
        || name.starts_with(".prettierrc")
        || name.starts_with(".swcrc")
        || (name.contains(".config.")
            && matches!(
                stem,
                "vite"
                    | "webpack"
                    | "rollup"
                    | "tsup"
                    | "esbuild"
                    | "svelte"
                    | "next"
                    | "nuxt"
                    | "astro"
                    | "babel"
                    | "postcss"
                    | "tailwind"
                    | "eslint"
                    | "prettier"
                    | "codegen"
                    | "metro"
                    | "remix"
                    | "turbo"
                    | "rspack"
                    | "vue"
            ))
        || (stem == "codegen"
            && [".ts", ".js", ".cjs", ".mjs", ".yml", ".yaml", ".json"]
                .iter()
                .any(|e| name.ends_with(e)))
}

/// Infrastructure as code and deployment manifests.
fn is_infrastructure(path: &str, name: &str) -> bool {
    if [".tf", ".tfvars", ".hcl"].iter().any(|e| name.ends_with(e)) {
        return true;
    }
    if matches!(
        name,
        "chart.yaml"
            | "serverless.yml"
            | "serverless.yaml"
            | "fly.toml"
            | "vercel.json"
            | "netlify.toml"
            | "render.yaml"
            | "procfile"
            | "skaffold.yaml"
            | "kustomization.yaml"
            | "cloudbuild.yaml"
            | "wrangler.toml"
            | "cdk.json"
    ) || (name.starts_with("pulumi") && name.ends_with(".yaml"))
        || (name == "app.yaml" && !path.contains('/'))
    {
        return true;
    }
    let yaml = name.ends_with(".yaml") || name.ends_with(".yml");
    let dirs: Vec<String> = path
        .to_ascii_lowercase()
        .split('/')
        .map(str::to_string)
        .collect();
    yaml && dirs[..dirs.len() - 1].iter().any(|d| {
        matches!(
            d.as_str(),
            "k8s"
                | "kubernetes"
                | "helm"
                | "charts"
                | "deploy"
                | "deployment"
                | "deployments"
                | "manifests"
                | "infra"
                | "infrastructure"
                | "terraform"
                | "kustomize"
        )
    })
}

/// A YAML file that declares a Kubernetes object (`apiVersion:` and
/// `kind:` at the top level).
fn is_kubernetes_manifest(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("apiVersion:"))
        && text.lines().any(|l| l.starts_with("kind:"))
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
        let text = std::fs::read_to_string(onus_core::paths::native(root, p)).ok()?;
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
        if f.status == Status::Renamed || ctx.is_test_data(&f.path) {
            continue;
        }
        let class = if ctx.packs.contains(&f.path)
            || f.old_path.as_ref().is_some_and(|p| ctx.packs.contains(p))
        {
            Class {
                subkind: "onus-pack-changed",
                label: "Onus pack",
                rules_of_the_game: true,
                why: "Changes what Onus sees in the code (this report used the base version); needs a person",
            }
        } else {
            let Some(class) = classify(&f.path) else {
                continue;
            };
            class
        };
        if class.subkind == "lockfile-changed" && ctx.explained.borrow().contains(&f.path) {
            continue;
        }
        let component = ctx.component_of_path(&f.path);
        let mut title = format!("{} `{}` changed", class.label, f.path);
        let mut subkind = class.subkind;
        let mut why = class.why.to_string();
        if subkind == "lockfile-changed"
            && let Some(w) = lockfile_why(ctx, f)
        {
            why = w;
        }
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
        let mut class = class;
        if subkind == "config-changed"
            && (f.path.ends_with(".yaml") || f.path.ends_with(".yml"))
            && is_kubernetes_manifest(&read_side(ctx, f))
        {
            class = classify("infra/x.tf").unwrap_or(class);
            subkind = class.subkind;
            why = class.why.to_string();
        }
        if f.status == Status::Modified
            && !f.path.ends_with("package.json")
            && let Some(keys) = changed_keys(ctx, f)
            && !keys.is_empty()
        {
            title = format!(
                "{} `{}` changes {}",
                class.label,
                f.path,
                crate::ctx::join_some(&keys, 5)
            );
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
        row.hints.needs_person = class.rules_of_the_game
            || matches!(
                subkind,
                "root-build-config-changed" | "infrastructure-changed"
            );
        rows.push(row);
    }
    // Many files of one kind in one component (a new library's tsconfig
    // files, a migration) read as one row.
    crate::ctx::group_rows(
        rows,
        |_| true,
        |r, n, place| format!("{}: {n} files changed {place}", r.kind_label),
        |r| format!("`{}`", r.locations.first().map_or("", |l| l.file.as_str())),
        |r, items| format!("{items}; {}", lower_first(&r.why_it_matters)),
    )
}

/// The head text of a file, or its base text when it was deleted.
fn read_side(ctx: &Ctx, f: &FileChange) -> String {
    let (root, path) = match f.status {
        Status::Deleted => (ctx.base_root, f.base_path()),
        _ => (ctx.head_root, f.path.as_str()),
    };
    std::fs::read_to_string(onus_core::paths::native(root, path)).unwrap_or_default()
}

/// The settings a modified config file changes, as code spans: changed
/// top-level keys of JSON and YAML (with `compilerOptions` broken down
/// for tsconfig files), and changed instructions of a Dockerfile.
fn changed_keys(ctx: &Ctx, f: &FileChange) -> Option<Vec<String>> {
    let base =
        std::fs::read_to_string(onus_core::paths::native(ctx.base_root, f.base_path())).ok()?;
    let head = std::fs::read_to_string(onus_core::paths::native(ctx.head_root, &f.path)).ok()?;
    let name = f
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&f.path)
        .to_ascii_lowercase();
    let code = |k: &str| format!("`{k}`");
    if name.ends_with(".json") || name.ends_with(".jsonc") || name.starts_with(".babelrc") {
        let b = onus_lang_ts::jsonc::parse(&base)?;
        let h = onus_lang_ts::jsonc::parse(&head)?;
        let (Value::Object(b), Value::Object(h)) = (&b, &h) else {
            return None;
        };
        let mut out = Vec::new();
        let keys: BTreeSet<&String> = b.keys().chain(h.keys()).collect();
        for k in keys {
            if b.get(k) == h.get(k) {
                continue;
            }
            match (k.as_str(), b.get(k), h.get(k)) {
                ("compilerOptions", bo, ho) => {
                    let empty = serde_json::Map::new();
                    let bo = bo.and_then(Value::as_object).unwrap_or(&empty);
                    let ho = ho.and_then(Value::as_object).unwrap_or(&empty);
                    let sub: BTreeSet<&String> = bo.keys().chain(ho.keys()).collect();
                    out.extend(
                        sub.into_iter()
                            .filter(|s| bo.get(*s) != ho.get(*s))
                            .map(|s| code(s)),
                    );
                }
                _ => out.push(code(k)),
            }
        }
        return Some(out);
    }
    if name.ends_with(".yaml") || name.ends_with(".yml") {
        let blocks = |text: &str| -> BTreeMap<String, String> {
            let mut out: BTreeMap<String, String> = BTreeMap::new();
            let mut key = String::new();
            for line in text.lines() {
                if !line.starts_with([' ', '\t', '#', '-']) && line.contains(':') {
                    key = line.split(':').next().unwrap_or("").trim().to_string();
                }
                out.entry(key.clone())
                    .or_default()
                    .push_str(line.trim_end());
                out.entry(key.clone()).or_default().push('\n');
            }
            out
        };
        let (b, h) = (blocks(&base), blocks(&head));
        let keys: BTreeSet<&String> = b.keys().chain(h.keys()).collect();
        return Some(
            keys.into_iter()
                .filter(|k| !k.is_empty() && b.get(*k) != h.get(*k))
                .map(|k| code(k))
                .collect(),
        );
    }
    if name.starts_with("dockerfile") || name.ends_with(".dockerfile") {
        let instructions = |text: &str| -> BTreeMap<String, BTreeSet<String>> {
            let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for line in text.lines().map(str::trim) {
                let Some((op, rest)) = line.split_once(char::is_whitespace) else {
                    continue;
                };
                let op = op.to_ascii_uppercase();
                let key = match op.as_str() {
                    "ENV" | "ARG" | "LABEL" => format!(
                        "{op} {}",
                        rest.trim().split(['=', ' ']).next().unwrap_or("")
                    ),
                    "FROM" | "RUN" | "CMD" | "ENTRYPOINT" | "EXPOSE" | "COPY" | "ADD" | "USER"
                    | "WORKDIR" | "HEALTHCHECK" => op.clone(),
                    _ => continue,
                };
                out.entry(key).or_default().insert(rest.trim().to_string());
            }
            out
        };
        let (b, h) = (instructions(&base), instructions(&head));
        let keys: BTreeSet<&String> = b.keys().chain(h.keys()).collect();
        return Some(
            keys.into_iter()
                .filter(|k| b.get(*k) != h.get(*k))
                .map(|k| match (k.as_str(), b.get(k), h.get(k)) {
                    ("FROM", Some(old), Some(new)) if old.len() == 1 && new.len() == 1 => format!(
                        "the base image (`{}` → `{}`)",
                        old.iter().next().map_or("", String::as_str),
                        new.iter().next().map_or("", String::as_str)
                    ),
                    _ => code(k),
                })
                .collect(),
        );
    }
    None
}

/// What a lockfile change installs differently, when Onus can read it.
fn lockfile_why(ctx: &Ctx, f: &FileChange) -> Option<String> {
    let name = f.path.rsplit('/').next().unwrap_or(&f.path);
    let read = |root: &std::path::Path, p: &str| {
        std::fs::read_to_string(onus_core::paths::native(root, p)).unwrap_or_default()
    };
    let base = crate::lockfiles::installed(name, &read(ctx.base_root, f.base_path()))?;
    let head = crate::lockfiles::installed(name, &read(ctx.head_root, &f.path))?;
    let changes = crate::lockfiles::changes(&base, &head);
    if changes.is_empty() {
        return Some(
            "No installed version changes; only lockfile metadata changed (patch hashes, \
             checksums or how versions resolve)"
                .into(),
        );
    }
    let manifest_changed = ctx.text.files.iter().any(|c| {
        let n = c.path.rsplit('/').next().unwrap_or(&c.path);
        matches!(
            n,
            "package.json" | "pnpm-workspace.yaml" | ".yarnrc.yml" | ".npmrc"
        )
    });
    Some(format!(
        "Installed versions change{}: {}",
        if manifest_changed {
            ""
        } else {
            " without a manifest change"
        },
        crate::ctx::join_some(&changes, 4)
    ))
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
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
        assert_eq!(sub("app.yaml"), Some(("infrastructure-changed", false)));
        assert_eq!(
            sub("infra/main.tf"),
            Some(("infrastructure-changed", false))
        );
        assert_eq!(
            sub("deploy/api.yaml"),
            Some(("infrastructure-changed", false))
        );
        assert_eq!(
            sub("tsconfig.base.json"),
            Some(("root-build-config-changed", false))
        );
        assert_eq!(
            sub("libs/a/tsconfig.json"),
            Some(("build-config-changed", false))
        );
        assert_eq!(
            sub("apps/web/project.json"),
            Some(("build-config-changed", false))
        );
        assert_eq!(
            sub("apps/web/vite.config.ts"),
            Some(("build-config-changed", false))
        );
        assert_eq!(
            sub("pnpm-workspace.yaml"),
            Some(("workspace-config-changed", false))
        );
        assert_eq!(sub("services/billing/src/discount.ts"), None);
        assert_eq!(sub("README.md"), None);
    }
}
