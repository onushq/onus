//! Module resolution for TypeScript and JavaScript imports.
//!
//! Onus resolves import specifiers itself, without Node or the TypeScript
//! compiler: relative paths, `index` files, `tsconfig` `paths` and `baseUrl`,
//! and workspace package names mapped to their source entrypoints. Anything
//! else is a third-party package (`npm:<name>`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use onus_core::WorkspacePackage;

use crate::jsonc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A file in the tree, relative to the root.
    File(String),
    /// A file in the tree that Onus does not analyze (`.svelte`, `.graphql`,
    /// `.json`, styles, images).
    Asset(String),
    /// A third-party package.
    Npm(String),
    /// A relative or mapped path that matches no file.
    Unresolved,
}

#[derive(Debug, Clone, Default)]
struct TsConfig {
    /// Directory of the tsconfig, relative to the root ("" for the root).
    dir: String,
    /// `baseUrl`, relative to the root.
    base_url: Option<String>,
    /// `paths` patterns with targets relative to the root.
    paths: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, Default)]
pub struct Resolver {
    files: BTreeSet<String>,
    tsconfigs: BTreeMap<String, TsConfig>,
    packages: BTreeMap<String, WorkspacePackage>,
    /// Package name → its `exports` subpaths (`./utils`, `./*`) and their
    /// targets, relative to the root.
    subpaths: BTreeMap<String, Vec<(String, Vec<String>)>>,
}

const TRY_EXTENSIONS: &[&str] = &[
    ".ts", ".tsx", ".mts", ".cts", ".d.ts", ".js", ".jsx", ".mjs", ".cjs",
];

impl Resolver {
    /// Builds a resolver over `files` (every file in the tree, relative
    /// paths), reading `tsconfig.json` files below `root`.
    pub fn new(
        root: &Path,
        files: impl IntoIterator<Item = String>,
        packages: BTreeMap<String, WorkspacePackage>,
    ) -> Self {
        let files: BTreeSet<String> = files.into_iter().collect();
        let mut tsconfigs = BTreeMap::new();
        for f in &files {
            let name = f.rsplit('/').next().unwrap_or(f);
            if name == "tsconfig.json" {
                let dir = parent(f).to_string();
                if let Some(cfg) = read_tsconfig(root, f, 0) {
                    tsconfigs.insert(dir, cfg);
                }
            }
        }
        let subpaths = packages
            .values()
            .filter_map(|p| Some((p.name.clone(), read_subpaths(root, &p.dir)?)))
            .collect();
        Resolver {
            files,
            tsconfigs,
            packages,
            subpaths,
        }
    }

    /// Resolves `spec` imported from the file `from`.
    pub fn resolve(&self, from: &str, spec: &str) -> Resolution {
        if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
            let joined = join(parent(from), spec);
            return self.try_target(&joined);
        }
        if spec.starts_with('/') {
            return Resolution::Unresolved;
        }
        // SvelteKit's `$lib` alias: `src/lib` next to `svelte.config.*`.
        if let Some(rest) = spec.strip_prefix("$lib")
            && (rest.is_empty() || rest.starts_with('/'))
            && let Some(kit) = self.svelte_kit_root(from)
        {
            return self.try_target(&join(&join(&kit, "src/lib"), &format!(".{rest}")));
        }
        if let Some(cfg) = self.nearest_tsconfig(from) {
            let mut mapped = false;
            for (pattern, targets) in &cfg.paths {
                if let Some(star) = match_pattern(pattern, spec) {
                    mapped = true;
                    for t in targets {
                        let candidate = t.replace('*', star);
                        match self.try_target(&candidate) {
                            Resolution::Unresolved => {}
                            found => return found,
                        }
                    }
                }
            }
            if mapped && !self.is_workspace_package(spec) {
                return Resolution::Unresolved;
            }
        }
        if let Some(pkg) = self.workspace_package(spec) {
            let sub = &spec[pkg.name.len()..];
            if sub.is_empty() {
                for e in &pkg.entrypoints {
                    if let Some(f) = self.try_file(e) {
                        return Resolution::File(f);
                    }
                }
                return Resolution::Unresolved;
            }
            return self.package_subpath(pkg, &format!(".{sub}"));
        }
        if let Some(cfg) = self.nearest_tsconfig(from)
            && let Some(base) = &cfg.base_url
            && let Some(f) = self.try_file(&join(base, spec))
        {
            return Resolution::File(f);
        }
        Resolution::Npm(package_name(spec).to_string())
    }

    /// `pkg/sub`: the source file `exports` names for `./sub` (mapped back
    /// from build output to source), else `./sub` or `./src/sub` in the
    /// package's folder.
    fn package_subpath(&self, pkg: &WorkspacePackage, sub: &str) -> Resolution {
        for (pattern, targets) in self.subpaths.get(&pkg.name).into_iter().flatten() {
            let Some(star) = match_pattern(pattern, sub) else {
                continue;
            };
            for target in targets {
                let target = target.replace('*', star);
                if let Some(f) = self.to_source(&join(&pkg.dir, &target)) {
                    return Resolution::File(f);
                }
            }
        }
        match self.try_target(&join(&pkg.dir, sub)) {
            Resolution::Unresolved => {}
            found => return found,
        }
        match self.try_file(&join(&join(&pkg.dir, "src"), sub)) {
            Some(f) => Resolution::File(f),
            None => Resolution::Unresolved,
        }
    }

    /// A build output path (`dist/utils/index.d.ts`) as the source file it
    /// is built from (`src/utils/index.ts`), or the path itself when it is
    /// source.
    fn to_source(&self, path: &str) -> Option<String> {
        let stem = strip_output_ext(path);
        if let Some(f) = self.try_file(stem) {
            return Some(f);
        }
        let mut parts: Vec<&str> = stem.split('/').collect();
        let out = parts
            .iter()
            .position(|p| matches!(*p, "dist" | "build" | "lib" | "out" | "esm" | "cjs"))?;
        parts[out] = "src";
        self.try_file(&parts.join("/"))
    }

    /// A code file (with extension and index probing), else any existing
    /// file as an asset, else unresolved.
    fn try_target(&self, candidate: &str) -> Resolution {
        if let Some(f) = self.try_file(candidate) {
            return Resolution::File(f);
        }
        let candidate = candidate.trim_end_matches('/');
        if self.files.contains(candidate) {
            return Resolution::Asset(candidate.to_string());
        }
        Resolution::Unresolved
    }

    /// The nearest folder above `from` with a `svelte.config.*` file.
    fn svelte_kit_root(&self, from: &str) -> Option<String> {
        let mut dir = parent(from);
        loop {
            for name in ["svelte.config.js", "svelte.config.ts", "svelte.config.mjs"] {
                let f = if dir.is_empty() {
                    name.to_string()
                } else {
                    format!("{dir}/{name}")
                };
                if self.files.contains(&f) {
                    return Some(dir.to_string());
                }
            }
            if dir.is_empty() {
                return None;
            }
            dir = parent(dir);
        }
    }

    fn is_workspace_package(&self, spec: &str) -> bool {
        self.workspace_package(spec).is_some()
    }

    /// The workspace package `spec` names, the longest name first: `@a/b/c`
    /// tries `@a/b/c`, then `@a/b`, then `@a`.
    fn workspace_package(&self, spec: &str) -> Option<&WorkspacePackage> {
        let mut end = spec.len();
        loop {
            let candidate = &spec[..end];
            if let Some(p) = self.packages.get(candidate)
                && p.name == candidate
            {
                return Some(p);
            }
            end = candidate.rfind('/')?;
        }
    }

    fn nearest_tsconfig(&self, from: &str) -> Option<&TsConfig> {
        let mut dir = parent(from);
        loop {
            if let Some(cfg) = self.tsconfigs.get(dir)
                && (cfg.base_url.is_some() || !cfg.paths.is_empty())
            {
                return Some(cfg);
            }
            if dir.is_empty() {
                return None;
            }
            dir = parent(dir);
        }
    }

    /// Tries `candidate` as a file, with extensions, and as a directory with
    /// an index file. TypeScript ESM imports name `.js` files that exist as
    /// `.ts` in source, so those are tried too.
    pub fn try_file(&self, candidate: &str) -> Option<String> {
        let candidate = candidate.trim_end_matches('/');
        if self.is_code(candidate) {
            return Some(candidate.to_string());
        }
        for (js, ts) in [
            (".js", ".ts"),
            (".js", ".tsx"),
            (".jsx", ".tsx"),
            (".mjs", ".mts"),
            (".cjs", ".cts"),
        ] {
            if let Some(stem) = candidate.strip_suffix(js) {
                let f = format!("{stem}{ts}");
                if self.files.contains(&f) {
                    return Some(f);
                }
            }
        }
        for ext in TRY_EXTENSIONS {
            let f = format!("{candidate}{ext}");
            if self.files.contains(&f) {
                return Some(f);
            }
        }
        for ext in TRY_EXTENSIONS {
            let f = if candidate.is_empty() {
                format!("index{ext}")
            } else {
                format!("{candidate}/index{ext}")
            };
            if self.files.contains(&f) {
                return Some(f);
            }
        }
        None
    }

    fn is_code(&self, path: &str) -> bool {
        self.files.contains(path) && TRY_EXTENSIONS.iter().any(|e| path.ends_with(e))
    }
}

/// `@scope/name/sub` → `@scope/name`; `name/sub` → `name`.
pub fn package_name(spec: &str) -> &str {
    let mut parts = spec.splitn(3, '/');
    let first = parts.next().unwrap_or(spec);
    if first.starts_with('@') {
        match parts.next() {
            Some(second) => &spec[..first.len() + 1 + second.len()],
            None => spec,
        }
    } else {
        first
    }
}

fn match_pattern<'a>(pattern: &str, spec: &'a str) -> Option<&'a str> {
    match pattern.split_once('*') {
        None => (pattern == spec).then_some(""),
        Some((prefix, suffix)) => {
            if spec.len() >= prefix.len() + suffix.len()
                && spec.starts_with(prefix)
                && spec.ends_with(suffix)
            {
                Some(&spec[prefix.len()..spec.len() - suffix.len()])
            } else {
                None
            }
        }
    }
}

pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Joins a relative path onto a directory and normalizes `.` and `..`.
pub fn join(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// `x.d.ts`, `x.mjs`, `x.js` → `x`.
fn strip_output_ext(path: &str) -> &str {
    for ext in [".d.ts", ".d.mts", ".d.cts", ".js", ".mjs", ".cjs"] {
        if let Some(stem) = path.strip_suffix(ext) {
            return stem;
        }
    }
    path
}

/// The `exports` subpaths of the package in `dir` with every target they
/// name under any condition, in order.
fn read_subpaths(root: &Path, dir: &str) -> Option<Vec<(String, Vec<String>)>> {
    let rel = if dir.is_empty() {
        "package.json".to_string()
    } else {
        format!("{dir}/package.json")
    };
    let text = std::fs::read_to_string(onus_core::paths::native(root, &rel)).ok()?;
    let pkg = jsonc::parse(&text)?;
    let serde_json::Value::Object(exports) = pkg.get("exports")? else {
        return None;
    };
    fn targets(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Object(m) => m.values().for_each(|v| targets(v, out)),
            serde_json::Value::Array(a) => a.iter().for_each(|v| targets(v, out)),
            _ => {}
        }
    }
    let out: Vec<(String, Vec<String>)> = exports
        .iter()
        .filter(|(k, _)| k.starts_with("./"))
        .map(|(k, v)| {
            let mut t = Vec::new();
            targets(v, &mut t);
            (k.clone(), t)
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

fn read_tsconfig(root: &Path, rel: &str, depth: u32) -> Option<TsConfig> {
    if depth > 5 {
        return None;
    }
    let text = std::fs::read_to_string(onus_core::paths::native(root, rel)).ok()?;
    let json = jsonc::parse(&text)?;
    let dir = parent(rel).to_string();
    let mut cfg = TsConfig {
        dir: dir.clone(),
        ..TsConfig::default()
    };
    if let Some(ext) = json.get("extends").and_then(|v| v.as_str())
        && ext.starts_with('.')
    {
        let mut target = join(&dir, ext);
        if !target.ends_with(".json") {
            target.push_str(".json");
        }
        if let Some(parent_cfg) = read_tsconfig(root, &target, depth + 1) {
            cfg.base_url = parent_cfg.base_url;
            cfg.paths = parent_cfg.paths;
        }
    }
    let opts = json.get("compilerOptions");
    let base_url = opts
        .and_then(|o| o.get("baseUrl"))
        .and_then(|v| v.as_str())
        .map(|b| join(&dir, b));
    if base_url.is_some() {
        cfg.base_url = base_url;
    }
    if let Some(paths) = opts
        .and_then(|o| o.get("paths"))
        .and_then(|v| v.as_object())
    {
        let anchor = cfg.base_url.clone().unwrap_or_else(|| cfg.dir.clone());
        let mut out = Vec::new();
        for (pattern, targets) in paths {
            let targets = targets
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str())
                        .map(|t| join(&anchor, t))
                        .collect()
                })
                .unwrap_or_default();
            out.push((pattern.clone(), targets));
        }
        // Longest prefix first, as TypeScript does.
        out.sort_by(|a, b| {
            let pa = a.0.split('*').next().unwrap_or("").len();
            let pb = b.0.split('*').next().unwrap_or("").len();
            pb.cmp(&pa).then_with(|| a.0.cmp(&b.0))
        });
        cfg.paths = out;
    }
    Some(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(dir: &Path, files: &[&str], tsconfig: Option<&str>) -> Resolver {
        let mut all: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        if let Some(cfg) = tsconfig {
            std::fs::write(dir.join("tsconfig.json"), cfg).unwrap();
            all.push("tsconfig.json".into());
        }
        let mut packages = BTreeMap::new();
        packages.insert(
            "@shop/events".to_string(),
            WorkspacePackage {
                name: "@shop/events".into(),
                component: "events".into(),
                dir: "packages/events".into(),
                entrypoints: vec!["packages/events/src/index.ts".into()],
            },
        );
        Resolver::new(dir, all, packages)
    }

    #[test]
    fn resolves_relative_paths_and_index_files() {
        let tmp = tempfile::tempdir().unwrap();
        let r = resolver(
            tmp.path(),
            &[
                "src/a.ts",
                "src/lib/index.ts",
                "src/util/ids.ts",
                "src/view.tsx",
            ],
            None,
        );
        assert_eq!(
            r.resolve("src/a.ts", "./lib"),
            Resolution::File("src/lib/index.ts".into())
        );
        assert_eq!(
            r.resolve("src/lib/index.ts", "../util/ids"),
            Resolution::File("src/util/ids.ts".into())
        );
        assert_eq!(
            r.resolve("src/a.ts", "./util/ids.js"),
            Resolution::File("src/util/ids.ts".into())
        );
        assert_eq!(
            r.resolve("src/a.ts", "./view.js"),
            Resolution::File("src/view.tsx".into())
        );
        assert_eq!(r.resolve("src/a.ts", "./missing"), Resolution::Unresolved);
    }

    #[test]
    fn resolves_workspace_packages_to_source() {
        let tmp = tempfile::tempdir().unwrap();
        let r = resolver(
            tmp.path(),
            &[
                "packages/events/src/index.ts",
                "packages/events/src/bus.ts",
                "services/a/src/x.ts",
            ],
            None,
        );
        assert_eq!(
            r.resolve("services/a/src/x.ts", "@shop/events"),
            Resolution::File("packages/events/src/index.ts".into())
        );
        assert_eq!(
            r.resolve("services/a/src/x.ts", "@shop/events/src/bus"),
            Resolution::File("packages/events/src/bus.ts".into())
        );
        assert_eq!(
            r.resolve("services/a/src/x.ts", "@acme/sms/client"),
            Resolution::Npm("@acme/sms".into())
        );
        assert_eq!(
            r.resolve("services/a/src/x.ts", "date-fns"),
            Resolution::Npm("date-fns".into())
        );
    }

    #[test]
    fn resolves_package_subpaths_through_exports() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("packages/events")).unwrap();
        std::fs::write(
            tmp.path().join("packages/events/package.json"),
            r#"{ "name": "@shop/events", "exports": {
                ".": { "types": "./dist/index.d.ts" },
                "./testing": { "types": "./dist/testing/index.d.ts", "import": "./dist/testing.mjs" },
                "./schemas/*": "./build/schemas/*.js"
            } }"#,
        )
        .unwrap();
        let r = resolver(
            tmp.path(),
            &[
                "packages/events/package.json",
                "packages/events/src/index.ts",
                "packages/events/src/testing/index.ts",
                "packages/events/src/schemas/order.ts",
                "packages/events/src/bus.ts",
                "services/a/src/x.ts",
            ],
            None,
        );
        let from = "services/a/src/x.ts";
        assert_eq!(
            r.resolve(from, "@shop/events/testing"),
            Resolution::File("packages/events/src/testing/index.ts".into())
        );
        assert_eq!(
            r.resolve(from, "@shop/events/schemas/order"),
            Resolution::File("packages/events/src/schemas/order.ts".into())
        );
        // Not in `exports`: the folder, then `src/`.
        assert_eq!(
            r.resolve(from, "@shop/events/bus"),
            Resolution::File("packages/events/src/bus.ts".into())
        );
    }

    #[test]
    fn resolves_tsconfig_paths_and_base_url() {
        let tmp = tempfile::tempdir().unwrap();
        let r = resolver(
            tmp.path(),
            &["src/app/main.ts", "src/shared/log.ts", "lib/money/index.ts"],
            Some(
                r#"{ "compilerOptions": { "baseUrl": "src", // comment
                     "paths": { "@money": ["../lib/money"], "@shared/*": ["shared/*"] } } }"#,
            ),
        );
        assert_eq!(
            r.resolve("src/app/main.ts", "@shared/log"),
            Resolution::File("src/shared/log.ts".into())
        );
        assert_eq!(
            r.resolve("src/app/main.ts", "@money"),
            Resolution::File("lib/money/index.ts".into())
        );
        assert_eq!(
            r.resolve("src/app/main.ts", "shared/log"),
            Resolution::File("src/shared/log.ts".into())
        );
        assert_eq!(
            r.resolve("src/app/main.ts", "@shared/nope"),
            Resolution::Unresolved
        );
    }

    #[test]
    fn resolves_assets_and_the_sveltekit_lib_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let r = resolver(
            tmp.path(),
            &[
                "apps/web/svelte.config.js",
                "apps/web/src/lib/api.ts",
                "apps/web/src/lib/Card.svelte",
                "apps/web/src/routes/page.ts",
                "apps/web/src/routes/query.graphql",
            ],
            None,
        );
        let from = "apps/web/src/routes/page.ts";
        assert_eq!(
            r.resolve(from, "$lib/api"),
            Resolution::File("apps/web/src/lib/api.ts".into())
        );
        assert_eq!(
            r.resolve(from, "$lib/Card.svelte"),
            Resolution::Asset("apps/web/src/lib/Card.svelte".into())
        );
        assert_eq!(
            r.resolve(from, "./query.graphql"),
            Resolution::Asset("apps/web/src/routes/query.graphql".into())
        );
        assert_eq!(r.resolve(from, "./missing.svelte"), Resolution::Unresolved);
        // Outside a SvelteKit app, `$lib` is just a package name.
        assert_eq!(
            r.resolve("other/x.ts", "$lib/api"),
            Resolution::Npm("$lib".into())
        );
    }

    #[test]
    fn package_names() {
        assert_eq!(package_name("@acme/sms"), "@acme/sms");
        assert_eq!(package_name("@acme/sms/x/y"), "@acme/sms");
        assert_eq!(package_name("lodash/get"), "lodash");
        assert_eq!(package_name("node:fs"), "node:fs");
    }
}
