//! What a lockfile says is installed: package names and versions, read
//! from `pnpm-lock.yaml`, `package-lock.json` and `yarn.lock`. Patch
//! hashes, checksums and peer suffixes are not versions.

use std::collections::{BTreeMap, BTreeSet};

/// Package name → installed versions.
pub type Installed = BTreeMap<String, BTreeSet<String>>;

/// The packages a lockfile installs, or `None` for a format Onus does not
/// read.
pub fn installed(file_name: &str, text: &str) -> Option<Installed> {
    match file_name {
        "pnpm-lock.yaml" => Some(pnpm(text)),
        "package-lock.json" | "npm-shrinkwrap.json" => npm(text),
        "yarn.lock" => Some(yarn(text)),
        _ => None,
    }
}

/// `name 1.0.0 → 1.1.0` for every package whose installed versions differ.
pub fn changes(base: &Installed, head: &Installed) -> Vec<String> {
    let names: BTreeSet<&String> = base.keys().chain(head.keys()).collect();
    let empty = BTreeSet::new();
    let mut out = Vec::new();
    for name in names {
        let b = base.get(name).unwrap_or(&empty);
        let h = head.get(name).unwrap_or(&empty);
        if b == h {
            continue;
        }
        let list = |s: &BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>().join(", ");
        out.push(match (b.is_empty(), h.is_empty()) {
            (true, _) => format!("`{name}` {} added", list(h)),
            (_, true) => format!("`{name}` {} removed", list(b)),
            _ => format!("`{name}` {} → {}", list(b), list(h)),
        });
    }
    out
}

/// `name@1.2.3(peer@1)(patch_hash=…)` or `/name/1.2.3` → (name, version).
fn split_spec(spec: &str) -> Option<(String, String)> {
    let quote = |c: char| c == '\'' || c == '"';
    let spec = spec.trim().trim_end_matches(':').trim_matches(quote);
    let spec = spec.split('(').next()?.trim_matches(quote);
    if let Some(v6) = spec.strip_prefix('/') {
        let (name, version) = v6.rsplit_once('/')?;
        return Some((name.to_string(), version.to_string()));
    }
    let at = spec.rfind('@').filter(|i| *i > 0)?;
    let (name, version) = (&spec[..at], &spec[at + 1..]);
    version
        .starts_with(|c: char| c.is_ascii_digit())
        .then(|| (name.to_string(), version.to_string()))
}

fn pnpm(text: &str) -> Installed {
    let mut out = Installed::new();
    let mut section = "";
    for line in text.lines() {
        if !line.starts_with(' ') && line.ends_with(':') {
            section = line.trim_end_matches(':');
            continue;
        }
        if !matches!(section, "packages" | "snapshots") {
            continue;
        }
        let Some(key) = line.strip_prefix("  ") else {
            continue;
        };
        if key.starts_with(' ') {
            continue;
        }
        let key = key.split(": ").next().unwrap_or(key);
        if let Some((name, version)) = split_spec(key) {
            out.entry(name).or_default().insert(version);
        }
    }
    out
}

fn npm(text: &str) -> Option<Installed> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut out = Installed::new();
    if let Some(packages) = json.get("packages").and_then(|p| p.as_object()) {
        for (path, entry) in packages {
            let Some(name) = path
                .rsplit("node_modules/")
                .next()
                .filter(|n| !n.is_empty())
            else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            if let Some(v) = entry.get("version").and_then(|v| v.as_str()) {
                out.entry(name.to_string())
                    .or_default()
                    .insert(v.to_string());
            }
        }
    } else if let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) {
        for (name, entry) in deps {
            if let Some(v) = entry.get("version").and_then(|v| v.as_str()) {
                out.entry(name.clone()).or_default().insert(v.to_string());
            }
        }
    }
    Some(out)
}

fn yarn(text: &str) -> Installed {
    let mut out = Installed::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if !line.starts_with(' ') && line.ends_with(':') && !line.starts_with('#') {
            let first = line.trim_end_matches(':').split(',').next().unwrap_or("");
            let first = first.trim().trim_matches('"');
            current = first
                .rfind('@')
                .filter(|i| *i > 0)
                .map(|i| first[..i].to_string());
            continue;
        }
        let trimmed = line.trim();
        let version = trimmed
            .strip_prefix("version ")
            .or_else(|| trimmed.strip_prefix("version: "));
        if let (Some(name), Some(v)) = (&current, version) {
            out.entry(name.clone())
                .or_default()
                .insert(v.trim().trim_matches('"').to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_versions_without_patch_hashes_or_peers() {
        let pnpm_lock = "lockfileVersion: '9.0'\n\npackages:\n  '@a/b@1.0.0':\n    resolution: {integrity: x}\n  c@2.0.0:\n    resolution: {integrity: y}\n\nsnapshots:\n  '@a/b@1.0.0(react@18.0.0)': {}\n  c@2.0.0(patch_hash=abc): {}\n";
        let got = installed("pnpm-lock.yaml", pnpm_lock).unwrap();
        assert_eq!(got["@a/b"].iter().collect::<Vec<_>>(), ["1.0.0"]);
        assert_eq!(got["c"].iter().collect::<Vec<_>>(), ["2.0.0"]);

        let yarn_lock = "\"left-pad@^1.0.0\", \"left-pad@^1.1.0\":\n  version \"1.3.0\"\n\n\"@s/x@npm:2\":\n  version: 2.1.0\n";
        let got = installed("yarn.lock", yarn_lock).unwrap();
        assert!(got["left-pad"].contains("1.3.0"));
        assert!(got["@s/x"].contains("2.1.0"));

        let npm_lock = r#"{ "packages": { "": {}, "node_modules/a": { "version": "1.0.0" }, "node_modules/b/node_modules/a": { "version": "2.0.0" } } }"#;
        let got = installed("package-lock.json", npm_lock).unwrap();
        assert_eq!(got["a"].len(), 2);
    }

    #[test]
    fn lists_version_changes() {
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let base: Installed = [("a".into(), set(&["1.0.0"])), ("b".into(), set(&["1.0.0"]))].into();
        let head: Installed = [("a".into(), set(&["1.1.0"])), ("c".into(), set(&["3.0.0"]))].into();
        assert_eq!(
            changes(&base, &head),
            ["`a` 1.0.0 → 1.1.0", "`b` 1.0.0 removed", "`c` 3.0.0 added"]
        );
    }
}
