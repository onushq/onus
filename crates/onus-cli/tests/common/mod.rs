//! Helpers shared by the integration tests: the shop fixture and its
//! scenario overlays.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

pub fn fixture() -> PathBuf {
    repo_root().join("fixtures/shop")
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Files at the root of a scenario folder that are not part of the overlay.
pub const SCENARIO_META: &[&str] = &["README.md", "deleted.txt", "intent.yaml"];

/// Applies a scenario overlay (changed and added files, `deleted.txt`) to
/// the tree at `head`.
pub fn apply_overlay(scenario: &Path, head: &Path) {
    fn walk(dir: &Path, rel: &Path, head: &Path, top: bool) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if top && SCENARIO_META.contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            let rel = rel.join(&name);
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), &rel, head, false);
            } else {
                let target = head.join(&rel);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    walk(scenario, Path::new(""), head, true);
    if let Ok(list) = std::fs::read_to_string(scenario.join("deleted.txt")) {
        for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            std::fs::remove_file(head.join(line)).unwrap();
        }
    }
}

/// A base tree and the head tree of one scenario, in a temporary directory.
pub struct Scenario {
    pub dir: tempfile::TempDir,
    pub id: String,
}

impl Scenario {
    pub fn new(id: &str) -> Scenario {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixture().join("base"), &dir.path().join("base"));
        copy_dir(&fixture().join("base"), &dir.path().join("head"));
        apply_overlay(
            &fixture().join("scenarios").join(id),
            &dir.path().join("head"),
        );
        Scenario {
            dir,
            id: id.to_string(),
        }
    }

    pub fn base(&self) -> PathBuf {
        self.dir.path().join("base")
    }

    pub fn head(&self) -> PathBuf {
        self.dir.path().join("head")
    }

    pub fn intent_path(&self) -> Option<PathBuf> {
        let p = fixture()
            .join("scenarios")
            .join(&self.id)
            .join("intent.yaml");
        p.exists().then_some(p)
    }

    pub fn run(&self) -> onus_cli::Outcome {
        let intent = self
            .intent_path()
            .and_then(|p| onus_cli::read_intent(&p).unwrap());
        onus_cli::diff_dirs(
            &self.base(),
            &self.head(),
            &onus_cli::DiffOptions {
                intent,
                base_label: "base".into(),
                head_label: "head".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }
}
