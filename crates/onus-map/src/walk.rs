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
    // Walked on several threads (large monorepos have tens of thousands of
    // folders), then sorted, so the result does not depend on scheduling.
    let out = std::sync::Mutex::new(Vec::new());
    WalkBuilder::new(root)
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
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                if let Ok(entry) = entry
                    && let Some(rel) = listed(root, &entry)
                    && let Ok(mut out) = out.lock()
                {
                    out.push(rel);
                }
                ignore::WalkState::Continue
            })
        });
    let mut out = out.into_inner().unwrap_or_default();
    out.sort();
    out
}

/// The relative path of `entry` if the map reads it.
fn listed(root: &Path, entry: &ignore::DirEntry) -> Option<String> {
    if !entry.file_type().is_some_and(|t| t.is_file()) {
        return None;
    }
    if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
        return None;
    }
    let rel = entry.path().strip_prefix(root).ok()?;
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    (!rel.ends_with(".min.js")).then_some(rel)
}
