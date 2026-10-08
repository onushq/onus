//! The minimal test runner: runs a command against a commit in a throwaway
//! container, to reproduce a failing test given as evidence. This is the one
//! place Onus runs repository code, and only when asked, in a container with
//! no network, no capabilities and bounded resources. An optional setup
//! step (installing dependencies) runs first, in its own container, with
//! network; the test itself never has it.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What a run showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRun {
    pub commit: String,
    pub image: String,
    pub command: String,
    pub exit_code: i32,
    /// The last lines of the command's output.
    pub output_tail: String,
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
    let mount = format!("{}:/work", work.path().display());
    let base = |network: bool| {
        let mut c = Command::new(&engine);
        c.args(["run", "--rm", "-v", &mount, "-w", "/work"])
            .args(["--cap-drop", "ALL", "--security-opt", "no-new-privileges"])
            .args(["--memory", "4g", "--cpus", "2", "--pids-limit", "1024"]);
        if !network {
            c.args(["--network", "none"]);
        }
        c
    };
    if let Some(setup) = setup {
        let out = base(true)
            .arg(image)
            .args(["sh", "-c", setup])
            .output()
            .context("cannot start the setup container")?;
        if !out.status.success() {
            bail!(
                "the setup step failed: {}",
                tail(&String::from_utf8_lossy(&out.stderr), 20)
            );
        }
    }
    let out = base(false)
        .arg(image)
        .args(["sh", "-c", command])
        .output()
        .context("cannot start the test container")?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(TestRun {
        commit,
        image: image.to_string(),
        command: command.to_string(),
        exit_code: out.status.code().unwrap_or(-1),
        output_tail: tail(&text, 40),
    })
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}
