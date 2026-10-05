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
        Resolver {
            files,
            tsconfigs,
            packages,
        }
    }

    /// Resolves `spec` imported from the file `from`.
    pub fn resolve(&self, from: &str, spec: &str) -> Resolution {
        if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
            let joined = join(parent(from), spec);
            return self
                .try_file(&joined)
                .map_or(Resolution::Unresolved, Resolution::File);
        }
        if spec.starts_with('/') {
            return Resolution::Unresolved;
        }
        if let Some(cfg) = self.nearest_tsconfig(from) {
            let mut mapped = false;
            for (pattern, targets) in &cfg.paths {
                if let Some(star) = match_pattern(pattern, spec) {
                    mapped = true;
                    for t in targets {
                        let candidate = t.replace('*', star);
                        if let Some(f) = self.try_file(&candidate) {
                            return Resolution::File(f);
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
            let candidate = join(&pkg.dir, &format!(".{sub}"));
            return self
                .try_file(&candidate)
                .map_or(Resolution::Unresolved, Resolution::File);
        }
        if let Some(cfg) = self.nearest_tsconfig(from)
            && let Some(base) = &cfg.base_url
            && let Some(f) = self.try_file(&join(base, spec))
        {
            return Resolution::File(f);
        }
        Resolution::Npm(package_name(spec).to_string())
    }

    fn is_workspace_package(&self, spec: &str) -> bool {
        self.workspace_package(spec).is_some()
    }

    fn workspace_package(&self, spec: &str) -> Option<&WorkspacePackage> {
        self.packages
            .values()
            .filter(|p| spec == p.name || spec.starts_with(&format!("{}/", p.name)))
            .max_by_key(|p| p.name.len())
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
    fn package_names() {
        assert_eq!(package_name("@acme/sms"), "@acme/sms");
        assert_eq!(package_name("@acme/sms/x/y"), "@acme/sms");
        assert_eq!(package_name("lodash/get"), "lodash");
        assert_eq!(package_name("node:fs"), "node:fs");
    }
}
