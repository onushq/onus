//! Line-level text diff of two trees. Line counts in reports come from here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use similar::{Algorithm, DiffOp, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Added,
    Deleted,
    Modified,
    /// Moved with identical bytes.
    Renamed,
}

/// One contiguous change. Line numbers are 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub base_lines: Vec<u32>,
    pub head_lines: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path in the head tree, or in the base tree for deleted files.
    pub path: String,
    /// Path in the base tree when the file was renamed.
    pub old_path: Option<String>,
    pub status: Status,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub added: u32,
    pub removed: u32,
    /// Added lines with their head line numbers.
    pub added_lines: Vec<(u32, String)>,
}

impl FileChange {
    pub fn base_path(&self) -> &str {
        self.old_path.as_deref().unwrap_or(&self.path)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TreeDiff {
    /// Sorted by path.
    pub files: Vec<FileChange>,
}

impl TreeDiff {
    pub fn lines_added(&self) -> u32 {
        self.files.iter().map(|f| f.added).sum()
    }

    pub fn lines_removed(&self) -> u32 {
        self.files.iter().map(|f| f.removed).sum()
    }

    pub fn get(&self, path: &str) -> Option<&FileChange> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Head path → base path for renamed files.
    pub fn renames(&self) -> BTreeMap<String, String> {
        self.files
            .iter()
            .filter_map(|f| f.old_path.clone().map(|o| (f.path.clone(), o)))
            .collect()
    }
}

fn read(root: &Path, rel: &str) -> Option<Vec<u8>> {
    std::fs::read(root.join(rel)).ok()
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

/// Diffs every file listed in either tree.
pub fn diff_trees(
    base_root: &Path,
    base_files: &[String],
    head_root: &Path,
    head_files: &[String],
) -> TreeDiff {
    let base: BTreeSet<&str> = base_files.iter().map(String::as_str).collect();
    let head: BTreeSet<&str> = head_files.iter().map(String::as_str).collect();
    let mut files = Vec::new();

    let mut deleted: Vec<&str> = base.difference(&head).copied().collect();
    let mut added: Vec<&str> = head.difference(&base).copied().collect();

    // Pure renames: a deleted and an added file with identical bytes.
    let mut deleted_by_content: BTreeMap<Vec<u8>, Vec<&str>> = BTreeMap::new();
    for d in &deleted {
        if let Some(bytes) = read(base_root, d) {
            deleted_by_content.entry(bytes).or_default().push(d);
        }
    }
    let mut renamed_from = BTreeSet::new();
    let mut renamed_to = BTreeSet::new();
    for a in &added {
        let Some(bytes) = read(head_root, a) else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        if let Some(candidates) = deleted_by_content.get_mut(&bytes)
            && let Some(pos) = candidates.iter().position(|c| !renamed_from.contains(c))
        {
            let from = candidates[pos];
            renamed_from.insert(from);
            renamed_to.insert(*a);
            files.push(FileChange {
                path: a.to_string(),
                old_path: Some(from.to_string()),
                status: Status::Renamed,
                binary: is_binary(&bytes),
                hunks: vec![],
                added: 0,
                removed: 0,
                added_lines: vec![],
            });
        }
    }
    deleted.retain(|d| !renamed_from.contains(d));
    added.retain(|a| !renamed_to.contains(a));

    for d in deleted {
        let bytes = read(base_root, d).unwrap_or_default();
        files.push(change(d, None, Status::Deleted, &bytes, &[]));
    }
    for a in added {
        let bytes = read(head_root, a).unwrap_or_default();
        files.push(change(a, None, Status::Added, &[], &bytes));
    }
    for p in base.intersection(&head) {
        let b = read(base_root, p).unwrap_or_default();
        let h = read(head_root, p).unwrap_or_default();
        if b != h {
            files.push(change(p, None, Status::Modified, &b, &h));
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    TreeDiff { files }
}

fn change(path: &str, old: Option<&str>, status: Status, base: &[u8], head: &[u8]) -> FileChange {
    let binary = is_binary(base) || is_binary(head);
    let mut fc = FileChange {
        path: path.to_string(),
        old_path: old.map(str::to_string),
        status,
        binary,
        hunks: vec![],
        added: 0,
        removed: 0,
        added_lines: vec![],
    };
    if binary {
        return fc;
    }
    let base_text = String::from_utf8_lossy(base);
    let head_text = String::from_utf8_lossy(head);
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_lines(base_text.as_ref(), head_text.as_ref());
    let head_lines: Vec<&str> = head_text.split_inclusive('\n').collect();
    for op in diff.ops() {
        let (old_range, new_range) = match *op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete {
                old_index, old_len, ..
            } => (old_index..old_index + old_len, 0..0),
            DiffOp::Insert {
                new_index, new_len, ..
            } => (0..0, new_index..new_index + new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (
                old_index..old_index + old_len,
                new_index..new_index + new_len,
            ),
        };
        let hunk = Hunk {
            base_lines: old_range.clone().map(|i| i as u32 + 1).collect(),
            head_lines: new_range.clone().map(|i| i as u32 + 1).collect(),
        };
        fc.removed += hunk.base_lines.len() as u32;
        fc.added += hunk.head_lines.len() as u32;
        for i in new_range {
            let line = head_lines[i].trim_end_matches(['\n', '\r']).to_string();
            fc.added_lines.push((i as u32 + 1, line));
        }
        fc.hunks.push(hunk);
    }
    fc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_and_detects_pure_renames() {
        let base = tempfile::tempdir().unwrap();
        let head = tempfile::tempdir().unwrap();
        std::fs::write(base.path().join("a.ts"), "1\n2\n3\n").unwrap();
        std::fs::write(head.path().join("a.ts"), "1\nTWO\n3\n4\n").unwrap();
        std::fs::write(base.path().join("old.ts"), "same\n").unwrap();
        std::fs::write(head.path().join("new.ts"), "same\n").unwrap();
        std::fs::write(head.path().join("added.ts"), "x\ny\n").unwrap();
        let d = diff_trees(
            base.path(),
            &["a.ts".into(), "old.ts".into()],
            head.path(),
            &["a.ts".into(), "added.ts".into(), "new.ts".into()],
        );
        let summary: Vec<(&str, Status, u32, u32)> = d
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status, f.added, f.removed))
            .collect();
        assert_eq!(
            summary,
            [
                ("a.ts", Status::Modified, 2, 1),
                ("added.ts", Status::Added, 2, 0),
                ("new.ts", Status::Renamed, 0, 0),
            ]
        );
        assert_eq!(
            d.renames().get("new.ts").map(String::as_str),
            Some("old.ts")
        );
        assert_eq!(
            d.get("a.ts").unwrap().added_lines,
            [(2, "TWO".to_string()), (4, "4".to_string())]
        );
    }
}
