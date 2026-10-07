//! Map paths (relative, `/`-separated) to filesystem paths.

use std::path::{Path, PathBuf};

/// Joins a `/`-separated relative path onto `root` one segment at a time,
/// so it also works for Windows verbatim (`\\?\`) roots.
pub fn native(root: &Path, rel: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for seg in rel.split('/').filter(|s| !s.is_empty() && *s != ".") {
        out.push(seg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_segments() {
        let p = native(Path::new("root"), "services/billing/./package.json");
        let parts: Vec<String> = p
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(parts, ["root", "services", "billing", "package.json"]);
    }
}
