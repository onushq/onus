//! The content addressed evidence store: artifacts under their sha256, and
//! one manifest per run, named by the sha256 of its own contents.
//!
//! ```text
//! <store>/objects/ab/cdef…   artifacts (output logs, JUnit XML, traces)
//! <store>/runs/<id>.json     manifests
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    /// The command's output, stdout and stderr interleaved.
    Log,
    /// Test results (JUnit XML).
    Junit,
    /// OpenTelemetry traces (OTLP JSON).
    Trace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub kind: ArtifactKind,
    /// The path in the environment, relative to the repository root (`-`
    /// for the log).
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// Test counts summed over the JUnit files of a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCounts {
    pub tests: u32,
    pub failures: u32,
    pub errors: u32,
    pub skipped: u32,
}

/// Where a run happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub name: String,
    /// The declared image, and the warm image built from it, if any.
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    /// The hosts the environment could reach through the egress proxy;
    /// empty means no network.
    pub hosts: Vec<String>,
    /// The names (never the values) of the secrets it was given.
    pub secrets: Vec<String>,
}

/// One run of one command in an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u32,
    pub commit: String,
    pub command: String,
    pub exit_code: i32,
    /// Unix seconds.
    pub started_at: u64,
    pub finished_at: u64,
    pub environment: EnvironmentInfo,
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestCounts>,
}

#[derive(Debug)]
pub struct Store {
    root: PathBuf,
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// The store of a repository: `onus/evidence` in its git directory,
    /// shared by its worktrees.
    pub fn for_repo(repo: &Path) -> Result<Store> {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .output()
            .context("cannot run git")?;
        if !out.status.success() {
            bail!("{} is not a git repository", repo.display());
        }
        let dir = PathBuf::from(String::from_utf8(out.stdout)?.trim());
        Ok(Store::open(dir.join("onus").join("evidence")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, sha: &str) -> PathBuf {
        self.root.join("objects").join(&sha[..2]).join(&sha[2..])
    }

    /// Stores `bytes` and returns their sha256.
    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let sha = sha256(bytes);
        let path = self.object_path(&sha);
        if !path.exists() {
            let dir = path.parent().expect("object directory");
            std::fs::create_dir_all(dir)?;
            // Written aside and renamed, so a reader never sees half an object.
            let tmp = tempfile::NamedTempFile::new_in(dir)?;
            std::fs::write(tmp.path(), bytes)?;
            tmp.persist(&path)?;
        }
        Ok(sha)
    }

    /// The artifact with this sha256, checked against it.
    pub fn get(&self, sha: &str) -> Result<Vec<u8>> {
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("`{sha}` is not a sha256");
        }
        let bytes = std::fs::read(self.object_path(sha))
            .with_context(|| format!("artifact {sha} is not in the evidence store"))?;
        if sha256(&bytes) != sha {
            bail!("artifact {sha} has been altered");
        }
        Ok(bytes)
    }

    /// Records a manifest and returns its id, the sha256 of its JSON.
    pub fn record(&self, manifest: &Manifest) -> Result<String> {
        let json = serde_json::to_vec_pretty(manifest)?;
        let id = sha256(&json);
        let dir = self.root.join("runs");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{id}.json")), json)?;
        Ok(id)
    }

    /// The manifest with this id, or the one id it is a prefix of (at
    /// least 8 characters), checked against its id.
    pub fn manifest(&self, id: &str) -> Result<(String, Manifest)> {
        if id.len() < 8 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("`{id}` is not a run id (at least 8 hex characters)");
        }
        let ids: Vec<String> = self
            .ids()?
            .into_iter()
            .filter(|i| i.starts_with(id))
            .collect();
        let full = match ids.as_slice() {
            [one] => one.clone(),
            [] => bail!(
                "run {id} is not in the evidence store {}",
                self.root.display()
            ),
            _ => bail!("run id {id} is ambiguous"),
        };
        let bytes = std::fs::read(self.root.join("runs").join(format!("{full}.json")))?;
        if sha256(&bytes) != full {
            bail!("the manifest of run {full} has been altered");
        }
        Ok((full, serde_json::from_slice(&bytes)?))
    }

    fn ids(&self) -> Result<Vec<String>> {
        let dir = self.root.join("runs");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids: Vec<String> = std::fs::read_dir(dir)?
            .filter_map(|e| {
                let name = e.ok()?.file_name().to_string_lossy().to_string();
                name.strip_suffix(".json").map(str::to_string)
            })
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Every run, oldest first.
    pub fn list(&self) -> Result<Vec<(String, Manifest)>> {
        let mut all = Vec::new();
        for id in self.ids()? {
            all.push(self.manifest(&id)?);
        }
        all.sort_by(|a, b| (a.1.started_at, &a.0).cmp(&(b.1.started_at, &b.0)));
        Ok(all)
    }
}

/// Counts the test cases of a JUnit XML file by their `<testcase>`
/// elements, which every reporter writes (suite totals are optional, and
/// some reporters, such as Node's, leave them out).
pub fn junit_counts(xml: &str) -> TestCounts {
    let case =
        regex::Regex::new(r"(?s)<testcase\b[^>]*?(?:/>|>(.*?)</testcase>)").expect("valid regex");
    let mut counts = TestCounts::default();
    for c in case.captures_iter(xml) {
        counts.tests += 1;
        let body = c.get(1).map_or("", |m| m.as_str());
        if body.contains("<failure") {
            counts.failures += 1;
        } else if body.contains("<error") {
            counts.errors += 1;
        } else if body.contains("<skipped") {
            counts.skipped += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            schema: SCHEMA,
            commit: "c0ffee".into(),
            command: "npm test".into(),
            exit_code: 0,
            started_at: 1,
            finished_at: 2,
            environment: EnvironmentInfo {
                name: "t".into(),
                image: "node:22".into(),
                warm_image: None,
                setup: None,
                seed: None,
                hosts: vec![],
                secrets: vec![],
            },
            artifacts: vec![],
            tests: None,
        }
    }

    #[test]
    fn objects_and_manifests_are_content_addressed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path());
        let sha = store.put(b"hello").unwrap();
        assert_eq!(store.put(b"hello").unwrap(), sha);
        assert_eq!(store.get(&sha).unwrap(), b"hello");
        let id = store.record(&manifest()).unwrap();
        let (full, m) = store.manifest(&id[..10]).unwrap();
        assert_eq!((full.as_str(), m), (id.as_str(), manifest()));
        assert_eq!(store.list().unwrap().len(), 1);

        // Editing a manifest or an artifact is detected.
        let run = dir.path().join("runs").join(format!("{id}.json"));
        let text = std::fs::read_to_string(&run).unwrap();
        std::fs::write(&run, text.replace("\"exitCode\": 0", "\"exitCode\": 1")).unwrap();
        assert!(
            store
                .manifest(&id)
                .unwrap_err()
                .to_string()
                .contains("altered")
        );
        std::fs::write(store.object_path(&sha), b"bye").unwrap();
        assert!(store.get(&sha).unwrap_err().to_string().contains("altered"));
    }

    #[test]
    fn junit_counts_count_test_cases() {
        // Suites nested, with totals.
        let xml = r#"<?xml version="1.0"?>
<testsuites tests="5" failures="1">
  <testsuite name="a" tests="3" failures="1" errors="0" skipped="1">
    <testcase name="x"/>
    <testcase name="y"><failure message="boom">at x.ts:3</failure></testcase>
    <testcase name="z"><skipped/></testcase>
  </testsuite>
  <testsuite name="b" tests="2">
    <testsuite name="b1"><testcase name="p"></testcase><testcase name="q"><error/></testcase></testsuite>
  </testsuite>
</testsuites>"#;
        assert_eq!(
            junit_counts(xml),
            TestCounts {
                tests: 5,
                failures: 1,
                errors: 1,
                skipped: 1
            }
        );
        // Node's reporter: test cases without a suite, totals in comments.
        let node = "<testsuites>\n\t<testcase name=\"a\" time=\"0.1\" classname=\"test\"/>\n\t<testcase name=\"b\" time=\"0.1\" classname=\"test\"/>\n\t<!-- tests 2 -->\n</testsuites>";
        assert_eq!(junit_counts(node).tests, 2);
    }
}
