//! The git gateway (ADR 0008): agents clone from it and push to it with a
//! task token; it holds the real repository and its credentials.
//!
//! - [`mirror`]: each task gets a mirror repository whose single root commit
//!   holds only the readable paths of the base commit.
//! - [`hook`]: the mirror's `pre-receive` hook refuses any pushed commit that
//!   changes a path outside the write scope, with an error that says how to
//!   ask for it; `post-receive` replays accepted commits.
//! - [`replay`]: accepted commits are applied onto the real base commit and
//!   pushed to the real remote.
//! - [`server`]: git smart HTTP through `git http-backend`, with the token as
//!   the password.

pub mod hook;
pub mod mirror;
pub mod replay;
pub mod server;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::scope::Scope;

/// Where the gateway keeps its mirrors and the state of each task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gateway {
    /// The real repository: a clone the gateway owns, with credentials to
    /// push to `remote`.
    pub repo: PathBuf,
    /// The remote accepted commits are pushed to (`origin`).
    pub remote: String,
    /// The commit tasks start from (`main`), resolved when a task's mirror is
    /// made.
    pub base: String,
    /// Mirrors (`<task>.git`) and task state (`<task>.json`).
    pub state: PathBuf,
    /// The root public key tokens are verified with (hex).
    pub public_key: String,
}

/// What the gateway remembers about a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskState {
    pub task: String,
    /// The real commit the task started from.
    pub base: String,
    /// The mirror's root commit standing for `base`.
    pub root: String,
    pub scope: Scope,
    /// Seconds since the Unix epoch when the token stops working.
    pub expires: u64,
    /// The token last presented for this task. The hooks verify it again for
    /// every push, so attenuations are enforced exactly.
    pub token: String,
}

impl Gateway {
    /// Saves the gateway's settings in its state folder, where the mirrors'
    /// hooks read them.
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.state)?;
        std::fs::write(
            self.state.join("gateway.json"),
            serde_json::to_string_pretty(self)?,
        )?;
        Ok(())
    }

    pub fn load(state: &Path) -> Result<Gateway> {
        let text = std::fs::read_to_string(state.join("gateway.json"))
            .with_context(|| format!("no gateway settings in {}", state.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Verifies a task's token with the gateway's key.
    pub fn verify(&self, token: &str) -> Result<crate::token::Verified> {
        crate::token::verify(token, &crate::token::public_key(&self.public_key)?)
    }

    pub fn mirror_path(&self, task: &str) -> PathBuf {
        self.state.join(format!("{task}.git"))
    }

    pub fn task_state_path(&self, task: &str) -> PathBuf {
        self.state.join(format!("{task}.json"))
    }

    pub fn audit_path(&self) -> PathBuf {
        self.state.join("audit.jsonl")
    }

    pub fn load_task(&self, task: &str) -> Result<TaskState> {
        let text = std::fs::read_to_string(self.task_state_path(task))
            .with_context(|| format!("no task `{task}` at this gateway"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save_task(&self, t: &TaskState) -> Result<()> {
        std::fs::create_dir_all(&self.state)?;
        let path = self.task_state_path(&t.task);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(t)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

/// Runs git in `dir` and returns its output; fails with git's message.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    git_with(dir, args, &[], None)
}

pub(crate) fn git_with(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["-c", "core.autocrlf=false", "-c", "core.quotepath=false"])
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Inside a hook, git exports the hook's repository (`GIT_DIR`) and the
    // quarantine holding the pushed objects. Those apply only to commands
    // in that repository; any other repository must not inherit them.
    if !in_hook_repo(dir) {
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_QUARANTINE_PATH",
            "GIT_PREFIX",
            "GIT_NAMESPACE",
        ] {
            cmd.env_remove(var);
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().context("cannot run git; is it installed?")?;
    if let Some(bytes) = input {
        use std::io::Write;
        let mut stdin = child.stdin.take().context("git stdin")?;
        stdin.write_all(bytes)?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Whether `dir` is the repository of the hook this process runs in.
fn in_hook_repo(dir: &Path) -> bool {
    let Some(git_dir) = std::env::var_os("GIT_DIR") else {
        return false;
    };
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    match (
        std::fs::canonicalize(cwd.join(git_dir)),
        std::fs::canonicalize(dir),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub(crate) fn git_str(dir: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(dir, args)?)?.trim().to_string())
}

/// One entry of `git ls-tree -r` or one change of `git diff-tree --raw`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeEntry {
    pub mode: String,
    pub sha: String,
    pub path: String,
}

/// `git ls-tree -r -z <commit>`: every file of a commit.
pub(crate) fn ls_tree(dir: &Path, commit: &str) -> Result<Vec<TreeEntry>> {
    let out = git(dir, &["ls-tree", "-r", "-z", "--full-tree", commit])?;
    let mut entries = Vec::new();
    for rec in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let rec = String::from_utf8_lossy(rec);
        let Some((meta, path)) = rec.split_once('\t') else {
            continue;
        };
        let parts: Vec<&str> = meta.split(' ').collect();
        if parts.len() == 3 {
            entries.push(TreeEntry {
                mode: parts[0].to_string(),
                sha: parts[2].to_string(),
                path: path.to_string(),
            });
        }
    }
    Ok(entries)
}

/// A file a commit changed: its new mode and blob (`000000…` when deleted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    pub old_mode: String,
    pub new_mode: String,
    pub new_sha: String,
    pub path: String,
}

pub(crate) const ZERO: &str = "0000000000000000000000000000000000000000";

/// `git diff-tree -r --raw --no-renames -z <from> <to>`.
pub(crate) fn changes(dir: &Path, from: &str, to: &str) -> Result<Vec<Change>> {
    let out = git(
        dir,
        &["diff-tree", "-r", "--raw", "--no-renames", "-z", from, to],
    )?;
    let mut fields = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut changes = Vec::new();
    while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
        let meta = String::from_utf8_lossy(meta);
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        if parts.len() < 5 {
            continue;
        }
        changes.push(Change {
            old_mode: parts[0].to_string(),
            new_mode: parts[1].to_string(),
            new_sha: parts[3].to_string(),
            path: String::from_utf8_lossy(path).to_string(),
        });
    }
    Ok(changes)
}

/// The seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
