//! Patched third-party packages: files under `patches/` (pnpm, patch-package)
//! and `.yarn/patches/`. A patch changes installed third-party code without
//! a version change, and it stops applying silently when the version moves,
//! so each added, changed or removed patch needs a person.

use onus_core::{ChangeKind, ChangeLevel, Location, SemanticChange};

use crate::ctx::{Ctx, change, join_some};
use crate::text::Status;

/// Whether `path` is a package patch.
pub fn is_patch(path: &str) -> bool {
    let in_dir = path.starts_with("patches/")
        || path.contains("/patches/")
        || path.starts_with(".yarn/patches/");
    in_dir && (path.ends_with(".patch") || path.ends_with(".diff"))
}

/// The package and version a patch file is for, from its name:
/// `@scope__name@1.2.3.patch` (pnpm), `@scope+name+1.2.3.patch`
/// (patch-package), `name-npm-1.2.3-<hash>.patch` (Yarn).
fn package_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name
        .strip_suffix(".patch")
        .or_else(|| name.strip_suffix(".diff"))
        .unwrap_or(name);
    if let Some(at) = stem.rfind('@').filter(|i| *i > 0) {
        let pkg = stem[..at].replacen("__", "/", 1);
        return format!("{pkg}@{}", &stem[at + 1..]);
    }
    if stem.contains('+') {
        let parts: Vec<&str> = stem.split('+').collect();
        let version = parts
            .iter()
            .position(|p| p.starts_with(|c: char| c.is_ascii_digit()));
        if let Some(v) = version {
            return format!("{}@{}", parts[..v].join("/"), parts[v]);
        }
    }
    if let Some((pkg, rest)) = stem.split_once("-npm-") {
        let version = rest.split('-').next().unwrap_or(rest);
        return format!("{pkg}@{version}");
    }
    stem.to_string()
}

/// The files a patch edits (`+++ b/dist/index.js` → `dist/index.js`).
fn patched_files(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix("+++ "))
        .map(|p| p.split('\t').next().unwrap_or(p).trim())
        .filter(|p| *p != "/dev/null")
        .map(|p| p.strip_prefix("b/").unwrap_or(p).to_string())
        .map(|p| format!("`{p}`"))
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn rows(ctx: &Ctx) -> Vec<SemanticChange> {
    let mut rows = Vec::new();
    for f in &ctx.text.files {
        if !is_patch(&f.path) {
            continue;
        }
        let pkg = package_of(&f.path);
        let (root, path) = match f.status {
            Status::Deleted => (ctx.base_root, f.base_path()),
            _ => (ctx.head_root, f.path.as_str()),
        };
        let text =
            std::fs::read_to_string(onus_core::paths::native(root, path)).unwrap_or_default();
        let lines = text.lines().count().max(1) as u32;
        let title = match f.status {
            Status::Added => format!("`{pkg}` is patched"),
            Status::Deleted => format!("`{pkg}` is no longer patched"),
            _ => format!("The patch of `{pkg}` changes"),
        };
        let files = patched_files(&text);
        let what = if files.is_empty() {
            String::new()
        } else {
            format!("it edits {} of the package; ", join_some(&files, 4))
        };
        let why = format!(
            "{}{what}installed third-party code changes without a version change, and the patch \
             stops applying when the version moves",
            match f.status {
                Status::Deleted => "The package's original code runs again; ",
                _ => "",
            }
        );
        let location = match f.status {
            Status::Deleted => Location::base(f.base_path(), 1, lines),
            _ => Location::head(&f.path, 1, lines),
        };
        let component = ctx.component_of_path(&f.path);
        let mut row = change(
            ChangeKind::Dependency,
            "dependency-patched",
            ChangeLevel::Behavior,
            &format!("npm:{pkg}"),
            (component != "root").then_some(component.as_str()),
            "Patched dependency",
            title,
            why,
            vec![location],
        );
        row.id = format!("dependency-patched:{}", f.path);
        row.hints.needs_person = true;
        ctx.explain(&f.path);
        if let Some(old) = &f.old_path {
            ctx.explain(old);
        }
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_patched_package() {
        assert_eq!(
            package_of("patches/@nx__esbuild@23.2.1.patch"),
            "@nx/esbuild@23.2.1"
        );
        assert_eq!(package_of("patches/left-pad@1.3.0.patch"), "left-pad@1.3.0");
        assert_eq!(
            package_of("patches/@scope+name+1.2.3.patch"),
            "@scope/name@1.2.3"
        );
        assert_eq!(
            package_of("patches/react+18.2.0+001+fix.patch"),
            "react@18.2.0"
        );
        assert_eq!(
            package_of(".yarn/patches/lodash-npm-4.17.21-6382451519.patch"),
            "lodash@4.17.21"
        );
        assert!(is_patch("patches/a@1.0.0.patch"));
        assert!(!is_patch("src/patches.ts"));
    }
}
