//! The minimal test runner: runs a command against a commit in a throwaway
//! container, to reproduce a failing test given as evidence. This is the one
//! place Onus runs repository code, and only when asked, in a container with
//! no network, no capabilities and bounded resources. The commit's files are
//! copied into the container, never mounted from the host. An optional setup
//! step (installing dependencies) runs first, in its own container, with
//! network; the test itself never has it. A run may be given a time limit,
//! after which its container is removed.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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

/// How a run with a time limit ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    Done(TestRun),
    /// The limit passed first; the container was removed.
    TimedOut,
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
    match run_within(repo, reference, image, setup, command, None)? {
        Ran::Done(run) => Ok(run),
        Ran::TimedOut => bail!("the run timed out"),
    }
}

/// [`run`], stopped when `limit` passes first. The limit covers every step
/// in the engine: pulling the image, copying the tree in, the setup and the
/// command.
pub fn run_within(
    repo: &Path,
    reference: &str,
    image: &str,
    setup: Option<&str>,
    command: &str,
    limit: Option<Duration>,
) -> Result<Ran> {
    let deadline = limit.map(|l| Instant::now() + l);
    let engine = engine()?;
    let (commit, work) = checkout(repo, reference)?;
    // Containers and the setup's image are named, so a step stopped part way
    // leaves nothing behind once they are removed by name.
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let unique = format!("{}-{}-{n}", &commit[..12], std::process::id());
    let name = format!("onus-run-{unique}");
    let tag = format!("onus-run:{unique}");
    let remove = |args: &[&str]| {
        let _ = Command::new(&engine).args(args).output();
    };
    // The engine may still finish a step whose client was stopped (a large
    // commit, say), so after a timeout what it would leave is removed again
    // a moment later.
    let sweep = |what: &[&[&str]]| {
        for wait in [0, 1, 2] {
            std::thread::sleep(Duration::from_secs(wait));
            for args in what {
                remove(args);
            }
        }
    };
    // `None` when the deadline passed first.
    let engine_run = |args: &[&str], input: Option<Vec<u8>>| -> Result<Option<Output>> {
        let mut c = Command::new(&engine);
        c.args(args);
        output_until(&mut c, input, deadline).with_context(|| format!("cannot run {engine}"))
    };
    // The tree is copied in rather than mounted, so it reaches engines that
    // run in a VM (colima, podman machine) or on another host.
    let create = |id: &str, image: &str, network: bool, script: &str| -> Result<Option<()>> {
        let mut args = vec!["create", "--name", id, "-w", "/work"];
        args.extend(["--cap-drop", "ALL", "--security-opt", "no-new-privileges"]);
        args.extend(["--memory", "4g", "--cpus", "2", "--pids-limit", "1024"]);
        if !network {
            args.extend(["--network", "none"]);
        }
        args.extend(["--entrypoint", "sh", image, "-c", script]);
        let Some(out) = engine_run(&args, None)? else {
            return Ok(None);
        };
        if !out.status.success() {
            bail!(
                "cannot create a container: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(Some(()))
    };
    let copy = |id: &str| -> Result<Option<()>> {
        let mut tar = RootTar::new();
        tar.tree(work.path(), "work")?;
        copy_until(&engine, id, tar, deadline)
    };
    // Creates the container, copies the tree in unless `image` has it, and
    // runs it to the end. Stopping `start -a` leaves the container running;
    // removing it stops what runs in it.
    let in_container = |id: &str,
                        image: &str,
                        network: bool,
                        script: &str,
                        with_tree: bool|
     -> Result<Option<Output>> {
        if create(id, image, network, script)?.is_none() {
            return Ok(None);
        }
        if with_tree && copy(id)?.is_none() {
            return Ok(None);
        }
        engine_run(&["start", "-a", id], None)
    };
    // Setup runs with network in its own container, kept as an image the
    // test starts from.
    let mut test_image = image.to_string();
    let mut temp_image = None;
    if let Some(setup) = setup {
        let id = format!("{name}-setup");
        let done = in_container(&id, image, true, setup, true);
        let committed = match done {
            Ok(Some(out)) if out.status.success() => {
                engine_run(&["commit", &id, &tag], None).map(|o| o.map(|o| o.status.success()))
            }
            Ok(Some(out)) => {
                remove(&["rm", "-f", &id]);
                let mut text = String::from_utf8_lossy(&out.stdout).to_string();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                bail!("the setup step failed: {}", tail(&text, 20));
            }
            other => other.map(|_| None),
        };
        remove(&["rm", "-f", &id]);
        match committed {
            Ok(Some(true)) => {}
            Ok(Some(false)) => bail!("cannot keep the setup step's result"),
            Ok(None) => {
                sweep(&[&["rm", "-f", &id], &["rmi", "-f", &tag]]);
                return Ok(Ran::TimedOut);
            }
            Err(e) => {
                remove(&["rmi", "-f", &tag]);
                return Err(e);
            }
        }
        test_image = tag.clone();
        temp_image = Some(tag);
    }
    let result = in_container(&name, &test_image, false, command, temp_image.is_none());
    remove(&["rm", "-f", &name]);
    if let Some(tag) = &temp_image {
        remove(&["rmi", "-f", tag]);
    }
    let Some(out) = result? else {
        match &temp_image {
            Some(tag) => sweep(&[&["rm", "-f", &name], &["rmi", "-f", tag]]),
            None => sweep(&[&["rm", "-f", &name]]),
        }
        return Ok(Ran::TimedOut);
    };
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(Ran::Done(TestRun {
        commit,
        image: image.to_string(),
        setup: setup.map(str::to_string),
        command: command.to_string(),
        exit_code: out.status.code().unwrap_or(-1),
        output_tail: tail(&text, 40),
        manifest: None,
    }))
}

/// Runs `c` to the end with `input` on its stdin, and collects its output;
/// `None`, with the process killed, when `deadline` passes first.
fn output_until(
    c: &mut Command,
    input: Option<Vec<u8>>,
    deadline: Option<Instant>,
) -> std::io::Result<Option<Output>> {
    let mut child = c
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Fed and drained on their own threads, so a full pipe never blocks
    // the wait.
    let feed = child.stdin.take().map(|mut pipe| {
        let bytes = input.unwrap_or_default();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        })
    });
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let status = match deadline {
        None => child.wait()?,
        Some(deadline) => loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(100));
        },
    };
    if let Some(feed) = feed {
        let _ = feed.join();
    }
    Ok(Some(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    }))
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
    copy_until(engine, container, tar, None).map(|_| ())
}

/// [`copy_in`]; `None` when `deadline` passes first.
fn copy_until(
    engine: &str,
    container: &str,
    tar: RootTar,
    deadline: Option<Instant>,
) -> Result<Option<()>> {
    let bytes = tar.0.into_inner()?;
    let mut c = Command::new(engine);
    c.args(["cp", "-", &format!("{container}:/")]);
    let Some(out) = output_until(&mut c, Some(bytes), deadline)
        .with_context(|| format!("cannot run {engine}"))?
    else {
        return Ok(None);
    };
    if !out.status.success() {
        bail!(
            "{engine} cp failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(Some(()))
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
