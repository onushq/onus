//! Listing the files of a tree.

use std::path::Path;

use ignore::WalkBuilder;

/// Directories that never contain source worth analyzing.
pub const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    ".turbo",
    ".cache",
    "coverage",
    "target",
    "vendor",
];

/// Files larger than this are skipped (minified bundles, generated code).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Every file under `root` that is not ignored, as sorted relative paths with
/// `/` separators. `.gitignore` files inside the tree are honored; files
/// outside it are not, so the result does not depend on where the tree is.
pub fn list_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .ignore(false)
        .follow_links(false)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_some_and(|t| t.is_dir()) && SKIPPED_DIRS.contains(&name.as_ref()))
        })
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if rel.ends_with(".min.js") {
            continue;
        }
        out.push(rel);
    }
    out.sort();
    out
}
