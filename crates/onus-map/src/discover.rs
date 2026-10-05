//! Component discovery: `onus.yaml`, then npm/pnpm/yarn workspaces, then
//! top-level folders under `services/`, `packages/` and `apps/`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use onus_core::{Component, ComponentKind, Confidence, OnusConfig, PackageDep};
use onus_lang_ts::component_dir;
use onus_lang_ts::lang;
use serde_json::Value;

/// Folders whose immediate subfolders are components when nothing else
/// declares them.
pub const FALLBACK_PARENTS: &[&str] = &["services", "packages", "apps"];

/// A component before its files are known.
#[derive(Debug, Clone, PartialEq)]
pub struct Discovered {
    pub components: Vec<Component>,
    /// How components were found: `onus.yaml`, `workspaces` or `folders`.
    pub source: &'static str,
}

pub fn discover(root: &Path, files: &[String], config: Option<&OnusConfig>) -> Discovered {
    let owners = Codeowners::load(root, files);
    if let Some(cfg) = config.filter(|c| !c.components.is_empty()) {
        let mut components = Vec::new();
        for (id, cc) in &cfg.components {
            let roots = cc.path.to_vec();
            let dir = component_dir(&roots);
            let pkg = read_package_json(root, &dir);
            let entrypoints = if cc.entrypoints.is_empty() {
                infer_entrypoints(&dir, pkg.as_ref(), files)
            } else {
                cc.entrypoints
                    .iter()
                    .map(|e| join_dir(&dir, e.trim_start_matches("./")))
                    .collect()
            };
            components.push(Component {
                id: id.clone(),
                kind: cc.kind.unwrap_or_else(|| kind_for(&dir)),
                roots,
                public_entrypoints: entrypoints,
                owners: if cc.owners.is_empty() {
                    owners.owners_of(&dir)
                } else {
                    cc.owners.clone()
                },
                labels: sorted(cc.labels.clone()),
                package_name: pkg.as_ref().and_then(package_name),
                confidence: Confidence::Declared,
            });
        }
        components.sort_by(|a, b| a.id.cmp(&b.id));
        return Discovered {
            components,
            source: "onus.yaml",
        };
    }
    let (dirs, source) = match workspace_dirs(root, files) {
        Some(d) if !d.is_empty() => (d, "workspaces"),
        _ => (fallback_dirs(files), "folders"),
    };
    let ids = component_ids(&dirs);
    let mut components: Vec<Component> = dirs
        .iter()
        .zip(ids)
        .map(|(dir, id)| {
            let pkg = read_package_json(root, dir);
            Component {
                id,
                kind: kind_for(dir),
                roots: vec![format!("{dir}/**")],
                public_entrypoints: infer_entrypoints(dir, pkg.as_ref(), files),
                owners: owners.owners_of(dir),
                labels: vec![],
                package_name: pkg.as_ref().and_then(package_name),
                confidence: Confidence::Inferred,
            }
        })
        .collect();
    components.sort_by(|a, b| a.id.cmp(&b.id));
    Discovered { components, source }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

fn kind_for(dir: &str) -> ComponentKind {
    let top = dir.split('/').next().unwrap_or("");
    match top {
        "services" | "apps" => ComponentKind::Service,
        "packages" | "libs" => ComponentKind::Package,
        _ => ComponentKind::Module,
    }
}

/// Component ids from directory names; parent names disambiguate clashes.
fn component_ids(dirs: &[String]) -> Vec<String> {
    let base = |d: &str| d.rsplit('/').next().unwrap_or(d).to_string();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for d in dirs {
        *counts.entry(base(d)).or_default() += 1;
    }
    dirs.iter()
        .map(|d| {
            let b = base(d);
            if counts[&b] > 1 {
                onus_core::ids::slug(d)
            } else {
                b
            }
        })
        .collect()
}

pub fn read_json(root: &Path, rel: &str) -> Option<Value> {
    let text = std::fs::read_to_string(onus_core::paths::native(root, rel)).ok()?;
    onus_lang_ts::jsonc::parse(&text)
}

fn read_package_json(root: &Path, dir: &str) -> Option<Value> {
    read_json(root, &join_dir(dir, "package.json"))
}

fn package_name(pkg: &Value) -> Option<String> {
    pkg.get("name").and_then(|n| n.as_str()).map(str::to_string)
}

pub fn join_dir(dir: &str, rel: &str) -> String {
    if dir.is_empty() {
        rel.to_string()
    } else {
        format!("{dir}/{rel}")
    }
}

/// Workspace package directories from `package.json` `workspaces` or
/// `pnpm-workspace.yaml`.
pub fn workspace_dirs(root: &Path, files: &[String]) -> Option<Vec<String>> {
    let mut patterns: Vec<String> = Vec::new();
    if let Some(pkg) = read_json(root, "package.json") {
        let ws = pkg.get("workspaces");
        let list = ws.and_then(|w| w.as_array()).or_else(|| {
            ws.and_then(|w| w.get("packages"))
                .and_then(|p| p.as_array())
        });
        if let Some(list) = list {
            patterns.extend(list.iter().filter_map(|v| v.as_str()).map(str::to_string));
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("pnpm-workspace.yaml"))
        && let Ok(v) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
        && let Some(list) = v.get("packages").and_then(|p| p.as_sequence())
    {
        patterns.extend(list.iter().filter_map(|v| v.as_str()).map(str::to_string));
    }
    if patterns.is_empty() {
        return None;
    }
    let mut include = GlobSetBuilder::new();
    let mut exclude = GlobSetBuilder::new();
    for p in &patterns {
        let (neg, p) = match p.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, p.as_str()),
        };
        let p = p.trim_start_matches("./").trim_end_matches('/');
        if let Ok(g) = GlobBuilder::new(p).literal_separator(true).build() {
            if neg {
                exclude.add(g);
            } else {
                include.add(g);
            }
        }
    }
    let include = include.build().ok()?;
    let exclude = exclude.build().ok()?;
    let mut dirs = BTreeSet::new();
    for f in files {
        if let Some(dir) = f.strip_suffix("/package.json")
            && include.is_match(dir)
            && !exclude.is_match(dir)
        {
            dirs.insert(dir.to_string());
        }
    }
    Some(dirs.into_iter().collect())
}

fn fallback_dirs(files: &[String]) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    for f in files {
        let mut parts = f.split('/');
        if let (Some(top), Some(name), Some(_)) = (parts.next(), parts.next(), parts.next())
            && FALLBACK_PARENTS.contains(&top)
        {
            dirs.insert(format!("{top}/{name}"));
        }
    }
    dirs.into_iter().collect()
}

/// Public entrypoints from `package.json` (`exports`, `types`, `module`,
/// `main`), mapped back from build output to source, else `src/index.*`.
pub fn infer_entrypoints(dir: &str, pkg: Option<&Value>, files: &[String]) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(pkg) = pkg {
        if let Some(exports) = pkg.get("exports") {
            collect_exports(exports, &mut candidates);
        }
        for key in ["types", "typings", "module", "main"] {
            if let Some(v) = pkg.get(key).and_then(|v| v.as_str()) {
                candidates.push(v.to_string());
            }
        }
    }
    let files: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let mut out = BTreeSet::new();
    for c in &candidates {
        if let Some(src) = to_source(dir, c, &files) {
            out.insert(src);
        }
    }
    if out.is_empty() {
        for c in ["src/index", "index", "src/main", "src/server"] {
            if let Some(src) = with_source_ext(&join_dir(dir, c), &files) {
                out.insert(src);
                break;
            }
        }
    }
    out.into_iter().collect()
}

fn collect_exports(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Object(map) => {
            // Subpath map: only the root entry defines the main surface,
            // but every subpath is public.
            for (k, v) in map {
                if k.starts_with('.')
                    || k == "import"
                    || k == "require"
                    || k == "default"
                    || k == "types"
                    || k == "node"
                {
                    collect_exports(v, out);
                }
            }
        }
        Value::Array(items) => {
            for i in items {
                collect_exports(i, out);
            }
        }
        _ => {}
    }
}

fn to_source(dir: &str, target: &str, files: &BTreeSet<&str>) -> Option<String> {
    let rel = target.trim_start_matches("./");
    if rel.contains('*') {
        return None;
    }
    let direct = join_dir(dir, rel);
    if lang::is_source(&direct) && files.contains(direct.as_str()) {
        return Some(direct);
    }
    let stem = strip_ext(rel);
    if let Some(src) = with_source_ext(&join_dir(dir, stem), files) {
        return Some(src);
    }
    for out_dir in ["dist/", "build/", "lib/", "out/", "esm/", "cjs/"] {
        if let Some(rest) = stem.strip_prefix(out_dir)
            && let Some(src) = with_source_ext(&join_dir(dir, &format!("src/{rest}")), files)
        {
            return Some(src);
        }
    }
    None
}

fn strip_ext(path: &str) -> &str {
    for ext in [
        ".d.ts", ".d.mts", ".d.cts", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".mts", ".cts",
    ] {
        if let Some(stem) = path.strip_suffix(ext) {
            return stem;
        }
    }
    path
}

fn with_source_ext(stem: &str, files: &BTreeSet<&str>) -> Option<String> {
    for ext in [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"] {
        let f = format!("{stem}{ext}");
        if files.contains(f.as_str()) {
            return Some(f);
        }
    }
    None
}

/// Assigns files to components by their root globs; the most specific
/// component wins.
#[derive(Debug)]
pub struct ComponentMatcher {
    sets: Vec<(String, usize, GlobSet)>,
}

impl ComponentMatcher {
    pub fn new(components: &[Component]) -> Self {
        let mut sets = Vec::new();
        for c in components {
            let mut b = GlobSetBuilder::new();
            for r in &c.roots {
                if let Ok(g) = Glob::new(r) {
                    b.add(g);
                }
                // `services/orders/**` should also match the folder itself.
                if let Some(dir) = r.strip_suffix("/**")
                    && let Ok(g) = Glob::new(dir)
                {
                    b.add(g);
                }
            }
            if let Ok(set) = b.build() {
                sets.push((c.id.clone(), component_dir(&c.roots).len(), set));
            }
        }
        ComponentMatcher { sets }
    }

    pub fn component_of(&self, path: &str) -> Option<String> {
        self.sets
            .iter()
            .filter(|(_, _, set)| set.is_match(path))
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .map(|(id, _, _)| id.clone())
    }
}

/// `CODEOWNERS` rules: last match wins.
#[derive(Debug, Default)]
pub struct Codeowners {
    rules: Vec<(GlobSet, Vec<String>)>,
}

pub const CODEOWNERS_PATHS: &[&str] = &[".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

impl Codeowners {
    pub fn load(root: &Path, files: &[String]) -> Self {
        let Some(path) = CODEOWNERS_PATHS
            .iter()
            .find(|p| files.iter().any(|f| f == *p))
        else {
            return Codeowners::default();
        };
        let text =
            std::fs::read_to_string(onus_core::paths::native(root, path)).unwrap_or_default();
        Codeowners::parse(&text)
    }

    pub fn parse(text: &str) -> Self {
        let mut rules = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(pattern) = parts.next() else {
                continue;
            };
            let owners: Vec<String> = parts
                .take_while(|p| !p.starts_with('#'))
                .map(str::to_string)
                .collect();
            let mut b = GlobSetBuilder::new();
            for g in codeowners_globs(pattern) {
                if let Ok(glob) = GlobBuilder::new(&g).literal_separator(true).build() {
                    b.add(glob);
                }
            }
            if let Ok(set) = b.build() {
                rules.push((set, owners));
            }
        }
        Codeowners { rules }
    }

    /// Owners of a directory: the last rule matching a file inside it.
    pub fn owners_of(&self, dir: &str) -> Vec<String> {
        let probe = join_dir(dir, "package.json");
        self.rules
            .iter()
            .rev()
            .find(|(set, _)| set.is_match(&probe))
            .map(|(_, o)| o.clone())
            .unwrap_or_default()
    }
}

fn codeowners_globs(pattern: &str) -> Vec<String> {
    let anchored = pattern.starts_with('/') || pattern.trim_end_matches('/').contains('/');
    let p = pattern.trim_start_matches('/');
    let prefix = if anchored { "" } else { "**/" };
    if p == "*" {
        return vec!["**".into()];
    }
    if let Some(dir) = p.strip_suffix('/') {
        return vec![format!("{prefix}{dir}/**")];
    }
    vec![format!("{prefix}{p}"), format!("{prefix}{p}/**")]
}

/// Third-party dependencies declared in each `package.json`, excluding
/// workspace packages.
pub fn package_deps(
    root: &Path,
    files: &[String],
    components: &[Component],
    matcher: &ComponentMatcher,
) -> Vec<PackageDep> {
    let workspace_names: BTreeSet<String> = components
        .iter()
        .filter_map(|c| c.package_name.clone())
        .collect();
    let mut out = Vec::new();
    for f in files {
        if f != "package.json" && !f.ends_with("/package.json") {
            continue;
        }
        let dir = f
            .strip_suffix("package.json")
            .unwrap_or("")
            .trim_end_matches('/');
        let component = if dir.is_empty() {
            onus_lang_ts::ROOT_COMPONENT.to_string()
        } else {
            match matcher.component_of(f) {
                Some(c) => c,
                None => continue,
            }
        };
        // Only the package.json at the component's own root counts.
        if !dir.is_empty()
            && let Some(c) = components.iter().find(|c| c.id == component)
            && component_dir(&c.roots) != dir
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(onus_core::paths::native(root, f)) else {
            continue;
        };
        let Some(json) = onus_lang_ts::jsonc::parse(&text) else {
            continue;
        };
        for section in [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ] {
            let Some(deps) = json.get(section).and_then(|d| d.as_object()) else {
                continue;
            };
            for (name, version) in deps {
                let version = version.as_str().unwrap_or("").to_string();
                if workspace_names.contains(name) || version.starts_with("workspace:") {
                    continue;
                }
                out.push(PackageDep {
                    component_id: component.clone(),
                    name: name.clone(),
                    line: find_dep_line(&text, section, name),
                    version,
                    section: section.to_string(),
                    file: f.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.component_id, &a.name, &a.section).cmp(&(&b.component_id, &b.name, &b.section))
    });
    out
}

/// Line of `"name":` inside the given section of a package.json text.
pub fn find_dep_line(text: &str, section: &str, name: &str) -> u32 {
    let section_key = format!("\"{section}\"");
    let name_key = format!("\"{name}\"");
    let mut in_section = false;
    for (i, line) in text.lines().enumerate() {
        if line.contains(&section_key) {
            in_section = true;
        }
        if in_section && line.trim_start().starts_with(&name_key) {
            return i as u32 + 1;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codeowners_last_match_wins() {
        let c = Codeowners::parse(
            "# owners\n* @everyone\n/services/billing/ @team-payments\npackages/ @team-platform\n",
        );
        assert_eq!(c.owners_of("services/billing"), ["@team-payments"]);
        assert_eq!(c.owners_of("packages/events"), ["@team-platform"]);
        assert_eq!(c.owners_of("services/orders"), ["@everyone"]);
    }

    #[test]
    fn maps_build_output_back_to_source() {
        let files: Vec<String> = [
            "packages/events/src/index.ts",
            "packages/events/package.json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let pkg: Value =
            serde_json::from_str(r#"{"main":"dist/index.js","types":"dist/index.d.ts"}"#).unwrap();
        assert_eq!(
            infer_entrypoints("packages/events", Some(&pkg), &files),
            ["packages/events/src/index.ts"]
        );
        let pkg: Value = serde_json::from_str(r#"{"exports":{".":"./src/index.ts"}}"#).unwrap();
        assert_eq!(
            infer_entrypoints("packages/events", Some(&pkg), &files),
            ["packages/events/src/index.ts"]
        );
        assert_eq!(
            infer_entrypoints("packages/events", None, &files),
            ["packages/events/src/index.ts"]
        );
    }

    #[test]
    fn most_specific_component_wins() {
        let mk = |id: &str, root: &str| Component {
            id: id.into(),
            kind: ComponentKind::Module,
            roots: vec![root.into()],
            public_entrypoints: vec![],
            owners: vec![],
            labels: vec![],
            package_name: None,
            confidence: Confidence::Declared,
        };
        let m = ComponentMatcher::new(&[
            mk("services", "services/**"),
            mk("billing", "services/billing/**"),
        ]);
        assert_eq!(
            m.component_of("services/billing/src/a.ts").as_deref(),
            Some("billing")
        );
        assert_eq!(
            m.component_of("services/orders/src/a.ts").as_deref(),
            Some("services")
        );
        assert_eq!(m.component_of("README.md"), None);
    }
}
