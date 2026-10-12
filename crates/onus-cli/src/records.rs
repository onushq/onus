//! Where CI keeps outcome records: files on a branch of the repository
//! itself (`onus/records` by default), so every workflow run, every
//! machine and `onus ui` read the same history, and nothing outside the
//! repository is needed. The files only grow by whole lines, so concurrent
//! runs merge by taking every line from both sides, keeping each line as
//! many times as the side that holds it most.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// The files kept on the branch.
pub const FILES: &[&str] = &["outcomes.jsonl", "pulls.jsonl"];

const README: &str = "# Onus records\n\nOutcome records (`outcomes.jsonl`) and pull request records (`pulls.jsonl`)\nkept by the Onus GitHub Action. Each line is one record; the files only grow.\nRead them with `onus outcomes summary --file outcomes.jsonl` or in `onus ui`.\n";

fn git(repo: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(repo).args(args);
    c
}

fn git_ok(repo: &Path, args: &[&str]) -> Option<String> {
    let out = git(repo, args).stderr(Stdio::null()).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

fn git_with_input(repo: &Path, args: &[&str], input: &str) -> Result<String> {
    use std::io::Write;
    let mut child = git(repo, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot run git")?;
    child
        .stdin
        .take()
        .context("no stdin")?
        .write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Fetches the branch; returns its commit, or `None` when it does not exist yet.
fn fetch(repo: &Path, remote: &str, branch: &str) -> Option<String> {
    let refspec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
    // A missing branch is the first run, not an error.
    let _ = git_ok(repo, &["fetch", "--no-tags", "--quiet", remote, &refspec]);
    git_ok(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}/{branch}^{{commit}}"),
        ],
    )
    .map(|s| s.trim().to_string())
}

fn read_at(repo: &Path, commit: Option<&str>, file: &str) -> String {
    commit
        .and_then(|c| git_ok(repo, &["show", &format!("{c}:{file}")]))
        .unwrap_or_default()
}

/// Every line of `theirs`, then the lines of `ours` that `theirs` does not
/// already hold. Lines are counted, not just compared: one run can write
/// the same line twice in a second (an approval and the run it triggers),
/// and both are kept. A line of `ours` matches one copy in `theirs`, so
/// lines both sides pulled from the branch are not doubled, and two runs
/// that each wrote the same line keep one copy: a record that is byte for
/// byte the same, change, time and all, says the same thing, and counting
/// it twice would count a re-delivered event twice.
pub fn union(theirs: &str, ours: &str) -> String {
    let mut left: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut out = String::new();
    for line in theirs.lines().filter(|l| !l.trim().is_empty()) {
        *left.entry(line).or_default() += 1;
        out.push_str(line);
        out.push('\n');
    }
    for line in ours.lines().filter(|l| !l.trim().is_empty()) {
        match left.get_mut(line) {
            Some(n) if *n > 0 => *n -= 1,
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// Copies the branch's files into `dir` (empty files when it does not exist).
pub fn pull(repo: &Path, remote: &str, branch: &str, dir: &Path) -> Result<bool> {
    std::fs::create_dir_all(dir)?;
    let tip = fetch(repo, remote, branch);
    for f in FILES {
        std::fs::write(dir.join(f), read_at(repo, tip.as_deref(), f))?;
    }
    Ok(tip.is_some())
}

/// Commits `dir`'s files to the branch, merged with whatever it holds now,
/// and pushes. Retries when another run pushed first.
pub fn push(repo: &Path, remote: &str, branch: &str, dir: &Path, message: &str) -> Result<bool> {
    for attempt in 0..6 {
        let tip = fetch(repo, remote, branch);
        let mut entries = String::new();
        for f in FILES {
            let ours = std::fs::read_to_string(dir.join(f)).unwrap_or_default();
            let merged = union(&read_at(repo, tip.as_deref(), f), &ours);
            let blob = git_with_input(repo, &["hash-object", "-w", "--stdin"], &merged)?;
            entries.push_str(&format!("100644 blob {blob}\t{f}\n"));
        }
        let readme = git_with_input(repo, &["hash-object", "-w", "--stdin"], README)?;
        entries.push_str(&format!("100644 blob {readme}\tREADME.md\n"));
        let tree = git_with_input(repo, &["mktree"], &entries)?;
        if let Some(t) = &tip
            && git_ok(repo, &["rev-parse", &format!("{t}^{{tree}}")])
                .is_some_and(|s| s.trim() == tree)
        {
            return Ok(false);
        }
        let mut args = vec!["commit-tree", tree.as_str()];
        if let Some(t) = &tip {
            args.extend(["-p", t.as_str()]);
        }
        args.extend(["-m", message]);
        let commit = {
            let out = git(repo, &args)
                .env("GIT_AUTHOR_NAME", "onus")
                .env("GIT_AUTHOR_EMAIL", "onus@users.noreply.github.com")
                .env("GIT_COMMITTER_NAME", "onus")
                .env("GIT_COMMITTER_EMAIL", "onus@users.noreply.github.com")
                .output()
                .context("cannot run git")?;
            if !out.status.success() {
                bail!(
                    "git commit-tree failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let pushed = git(
            repo,
            &[
                "push",
                "--quiet",
                remote,
                &format!("{commit}:refs/heads/{branch}"),
            ],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("cannot run git push")?;
        if pushed.status.success() {
            return Ok(true);
        }
        if attempt == 5 {
            bail!(
                "cannot push the records to {remote}/{branch}: {}",
                String::from_utf8_lossy(&pushed.stderr).trim()
            );
        }
        // Another run pushed first: merge with its records and try again.
        std::thread::sleep(std::time::Duration::from_millis(400 * (attempt + 1)));
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_merge_by_line() {
        assert_eq!(union("a\nb\n", "a\nb\nc\n"), "a\nb\nc\n");
        assert_eq!(union("a\nb\nx\n", "a\nb\nc\n"), "a\nb\nx\nc\n");
        assert_eq!(union("", ""), "");
    }

    #[test]
    fn records_written_twice_stay_twice() {
        // An approval and the run it triggers, in the same second.
        assert_eq!(union("", "a\nb\nb\n"), "a\nb\nb\n");
        assert_eq!(union("a\nb\n", "a\nb\nb\n"), "a\nb\nb\n");
        assert_eq!(union("a\nb\nb\n", "a\nb\nb\n"), "a\nb\nb\n");
        // Another run added a line meanwhile: both runs' lines are kept.
        assert_eq!(union("a\nb\nx\n", "a\nb\nb\n"), "a\nb\nx\nb\n");
    }

    #[test]
    fn the_same_record_from_two_runs_is_kept_once() {
        // Two runs from the same tip each wrote line `x` (a re-delivered
        // event): one copy, as before.
        assert_eq!(union("a\nx\n", "a\nx\n"), "a\nx\n");
        // One of them wrote it twice: the run's two copies are kept.
        assert_eq!(union("a\nx\n", "a\nx\nx\n"), "a\nx\nx\n");
    }
}
