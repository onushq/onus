//! The minimal test runner: runs a command against a commit in a throwaway
//! container, to reproduce a failing test given as evidence. This is the one
//! place Onus runs repository code, and only when asked, in a container with
//! no network, no capabilities and bounded resources. The commit's files are
//! copied into the container, never mounted from the host. An optional setup
//! step (installing dependencies) runs first, in its own container, with
//! network; the test itself never has it.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What a run showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRun {
    pub commit: String,
    pub image: String,
    /// The setup command that ran first, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    pub command: String,
    pub exit_code: i32,
    /// The last lines of the command's output.
    pub output_tail: String,
    /// The evidence manifest of the run, when it ran in an environment
    /// (`onus env run`): the judge checks the claim against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
}

impl TestRun {
    pub fn failed(&self) -> bool {
        self.exit_code != 0
    }
}

/// Which container program to use: docker, else podman.
pub fn engine() -> Result<String> {
    for engine in ["docker", "podman"] {
        if Command::new(engine)
            .args(["info", "--format", "{{.ServerVersion}}"])
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return Ok(engine.to_string());
        }
    }
    bail!("no container engine (docker or podman) is running; the test runner needs one")
}

/// Runs `command` (a shell command line) at `reference` of `repo` in
/// `image`. `setup`, when given, runs first with network.
pub fn run(
    repo: &Path,
    reference: &str,
    image: &str,
    setup: Option<&str>,
    command: &str,
) -> Result<TestRun> {
    let engine = engine()?;
    let (commit, work) = checkout(repo, reference)?;
    // The tree is copied in rather than mounted, so it reaches engines that
    // run in a VM (colima, podman machine) or on another host.
    let create = |image: &str, network: bool, script: &str| -> Result<String> {
        let mut c = Command::new(&engine);
        c.args(["create", "-w", "/work"])
            .args(["--cap-drop", "ALL", "--security-opt", "no-new-privileges"])
            .args(["--memory", "4g", "--cpus", "2", "--pids-limit", "1024"]);
        if !network {
            c.args(["--network", "none"]);
        }
        let out = c
            .args(["--entrypoint", "sh", image, "-c", script])
            .output()
            .context("cannot create a container")?;
        if !out.status.success() {
            bail!(
                "cannot create a container: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let remove = |args: &[&str]| {
        let _ = Command::new(&engine).args(args).output();
    };
    let start = |id: &str| -> Result<std::process::Output> {
        Command::new(&engine)
            .args(["start", "-a", id])
            .output()
            .context("cannot start the container")
    };
    let tree = || -> Result<RootTar> {
        let mut tar = RootTar::new();
        tar.tree(work.path(), "work")?;
        Ok(tar)
    };
    // Setup runs with network in its own container, kept as an image the
    // test starts from.
    let mut test_image = image.to_string();
    let mut temp_image = None;
    if let Some(setup) = setup {
        let id = create(image, true, setup)?;
        let done = copy_in(&engine, &id, tree()?).and_then(|()| start(&id));
        let tag = format!("onus-run:{}-{}", &commit[..12], std::process::id());
        let committed = match done {
            Ok(out) if out.status.success() => Command::new(&engine)
                .args(["commit", &id, &tag])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            Ok(out) => {
                remove(&["rm", "-f", &id]);
                let mut text = String::from_utf8_lossy(&out.stdout).to_string();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                bail!("the setup step failed: {}", tail(&text, 20));
            }
            Err(e) => {
                remove(&["rm", "-f", &id]);
                return Err(e);
            }
        };
        remove(&["rm", "-f", &id]);
        if !committed {
            bail!("cannot keep the setup step's result");
        }
        test_image = tag.clone();
        temp_image = Some(tag);
    }
    let result = (|| -> Result<std::process::Output> {
        let id = create(&test_image, false, command)?;
        let out = if temp_image.is_none() {
            copy_in(&engine, &id, tree()?).and_then(|()| start(&id))
        } else {
            start(&id)
        };
        remove(&["rm", "-f", &id]);
        out
    })();
    if let Some(tag) = &temp_image {
        remove(&["rmi", "-f", tag]);
    }
    let out = result?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(TestRun {
        commit,
        image: image.to_string(),
        setup: setup.map(str::to_string),
        command: command.to_string(),
        exit_code: out.status.code().unwrap_or(-1),
        output_tail: tail(&text, 40),
        manifest: None,
    })
}

/// A tar archive whose entries belong to root: copied into a container
/// with no capabilities, files keep their modes and times but must be
/// root's to be written.
pub struct RootTar(tar::Builder<Vec<u8>>);

impl std::fmt::Debug for RootTar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RootTar")
    }
}

impl Default for RootTar {
    fn default() -> RootTar {
        RootTar::new()
    }
}

impl RootTar {
    pub fn new() -> RootTar {
        RootTar(tar::Builder::new(Vec::new()))
    }

    fn header(meta: &std::fs::Metadata) -> Result<tar::Header> {
        let mut h = tar::Header::new_gnu();
        h.set_metadata_in_mode(meta, tar::HeaderMode::Complete);
        h.set_uid(0);
        h.set_gid(0);
        h.set_username("root")?;
        h.set_groupname("root")?;
        Ok(h)
    }

    /// Adds the files under `dir` at `prefix`, and returns their paths
    /// relative to `dir`, sorted.
    pub fn tree(&mut self, dir: &Path, prefix: &str) -> Result<Vec<String>> {
        let mut files = Vec::new();
        self.walk(dir, prefix, "", &mut files)?;
        files.sort();
        Ok(files)
    }

    fn walk(&mut self, dir: &Path, prefix: &str, rel: &str, files: &mut Vec<String>) -> Result<()> {
        let mut h = Self::header(&std::fs::symlink_metadata(dir)?)?;
        let at = if rel.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}/{rel}")
        };
        self.0.append_data(&mut h, &at, std::io::empty())?;
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            let path = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let meta = std::fs::symlink_metadata(e.path())?;
            if meta.is_dir() {
                self.walk(&e.path(), prefix, &path, files)?;
                continue;
            }
            let mut h = Self::header(&meta)?;
            let at = format!("{prefix}/{path}");
            if meta.file_type().is_symlink() {
                self.0
                    .append_link(&mut h, &at, std::fs::read_link(e.path())?)?;
            } else {
                self.0
                    .append_data(&mut h, &at, std::fs::File::open(e.path())?)?;
            }
            files.push(path);
        }
        Ok(())
    }

    pub fn file(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(crate::gateway::now());
        h.set_uid(0);
        h.set_gid(0);
        self.0.append_data(&mut h, path, bytes)?;
        Ok(())
    }
}

/// Extracts the archive at the container's root.
pub fn copy_in(engine: &str, container: &str, tar: RootTar) -> Result<()> {
    let bytes = tar.0.into_inner()?;
    let mut child = Command::new(engine)
        .args(["cp", "-", &format!("{container}:/")])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {engine}"))?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().context("no stdin")?;
        stdin.write_all(&bytes)?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "{engine} cp failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// The commit `reference` names, and its tree unpacked into a temporary
/// directory (removed when dropped).
pub fn checkout(repo: &Path, reference: &str) -> Result<(String, tempfile::TempDir)> {
    let commit = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
            .output()?
            .stdout,
    )?
    .trim()
    .to_string();
    if commit.is_empty() {
        bail!("`{reference}` is not a commit");
    }
    let work = tempfile::Builder::new().prefix("onus-run-").tempdir()?;
    let archive = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["archive", "--format=tar", &commit])
        .output()?;
    if !archive.status.success() {
        bail!("cannot extract {reference}");
    }
    let status = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(work.path())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().map(|mut s| s.write_all(&archive.stdout));
            c.wait()
        })
        .context("cannot unpack the commit")?;
    if !status.success() {
        bail!("cannot unpack the commit");
    }
    Ok((commit, work))
}

/// The last `lines` lines of `text`.
pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}
