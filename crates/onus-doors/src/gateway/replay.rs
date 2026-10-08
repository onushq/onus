//! Replaying accepted commits onto the real repository. Each mirror commit
//! becomes a real commit with the same changed files, author, committer and
//! message, on top of the real counterpart of its parent (the snapshot's
//! counterpart is the real base). The mapping is kept as refs in the real
//! repository, so a later push continues where the last one ended.

use anyhow::{Context, Result, bail};

use super::{Gateway, TaskState, ZERO, changes, git, git_str, git_with};

fn map_ref(task: &str, mirror_commit: &str) -> String {
    format!("refs/onus/{task}/map/{mirror_commit}")
}

fn mapped(gw: &Gateway, state: &TaskState, mirror_commit: &str) -> Option<String> {
    if mirror_commit == state.root {
        return Some(state.base.clone());
    }
    git_str(
        &gw.repo,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &map_ref(&state.task, mirror_commit),
        ],
    )
    .ok()
    .filter(|s| !s.is_empty())
}

/// Replays the commits of `refname` (already in the mirror) and pushes the
/// result to the same ref on the real remote. Returns the real commit.
pub fn replay_and_push(gw: &Gateway, state: &TaskState, refname: &str) -> Result<String> {
    let mirror = gw.mirror_path(&state.task);
    let mirror_s = mirror.to_string_lossy().to_string();
    let incoming = format!("refs/onus/{}/incoming", state.task);
    git(
        &gw.repo,
        &[
            "fetch",
            "-q",
            "--no-tags",
            &mirror_s,
            &format!("+{refname}:{incoming}"),
        ],
    )?;
    let head = git_str(&gw.repo, &["rev-parse", &incoming])?;
    let commits = git_str(
        &gw.repo,
        &[
            "rev-list",
            "--reverse",
            "--topo-order",
            &head,
            "--not",
            &state.root,
        ],
    )?;
    for c in commits.lines().filter(|l| !l.is_empty()) {
        if mapped(gw, state, c).is_some() {
            continue;
        }
        let parent = git_str(&gw.repo, &["rev-parse", &format!("{c}^")])?;
        let real_parent = mapped(gw, state, &parent)
            .with_context(|| format!("the parent of {c} was never replayed"))?;
        let real = replay_one(gw, state, c, &parent, &real_parent)?;
        git(&gw.repo, &["update-ref", &map_ref(&state.task, c), &real])?;
    }
    let real = mapped(gw, state, &head).context("nothing to push")?;
    git(
        &gw.repo,
        &["push", "-q", &gw.remote, &format!("{real}:{refname}")],
    )
    .with_context(|| format!("cannot push to `{}`", gw.remote))?;
    Ok(real)
}

fn replay_one(
    gw: &Gateway,
    state: &TaskState,
    commit: &str,
    parent: &str,
    real_parent: &str,
) -> Result<String> {
    let index = gw.state.join(format!("{}.replay-index", state.task));
    let index_s = index.to_string_lossy().to_string();
    let env_index = [("GIT_INDEX_FILE", index_s.as_str())];
    let _ = std::fs::remove_file(&index);
    git_with(&gw.repo, &["read-tree", real_parent], &env_index, None)?;
    let mut input = String::new();
    for ch in changes(&gw.repo, parent, commit)? {
        // The pre-receive hook already refused anything else; check again,
        // since the real repository is what matters.
        if !state.scope.can_write(&ch.path) || matches!(ch.new_mode.as_str(), "120000" | "160000") {
            bail!("`{}` cannot be replayed: outside the write scope", ch.path);
        }
        if ch.new_mode == "000000" {
            input.push_str(&format!("0 {ZERO}\t{}\n", ch.path));
        } else {
            input.push_str(&format!("{} {}\t{}\n", ch.new_mode, ch.new_sha, ch.path));
        }
    }
    git_with(
        &gw.repo,
        &["update-index", "--index-info"],
        &env_index,
        Some(input.as_bytes()),
    )?;
    let tree = String::from_utf8(git_with(&gw.repo, &["write-tree"], &env_index, None)?)?
        .trim()
        .to_string();
    let _ = std::fs::remove_file(&index);
    // Author, committer and message of the agent's commit.
    let raw = git(&gw.repo, &["cat-file", "commit", commit])?;
    let text = String::from_utf8_lossy(&raw);
    let (headers, message) = text.split_once("\n\n").unwrap_or((&text, ""));
    let person = |key: &str| -> Option<(String, String, String)> {
        let line = headers
            .lines()
            .find(|l| l.starts_with(&format!("{key} ")))?;
        let rest = &line[key.len() + 1..];
        let lt = rest.find('<')?;
        let gt = rest.find('>')?;
        let name = rest[..lt].trim().to_string();
        let email = rest[lt + 1..gt].to_string();
        let date = rest[gt + 1..].trim().to_string();
        Some((name, email, format!("@{date}")))
    };
    let (an, ae, ad) = person("author").context("the commit has no author")?;
    let (cn, ce, cd) = person("committer").context("the commit has no committer")?;
    let env = [
        ("GIT_AUTHOR_NAME", an.as_str()),
        ("GIT_AUTHOR_EMAIL", ae.as_str()),
        ("GIT_AUTHOR_DATE", ad.as_str()),
        ("GIT_COMMITTER_NAME", cn.as_str()),
        ("GIT_COMMITTER_EMAIL", ce.as_str()),
        ("GIT_COMMITTER_DATE", cd.as_str()),
    ];
    let real = String::from_utf8(git_with(
        &gw.repo,
        &["commit-tree", &tree, "-p", real_parent],
        &env,
        Some(message.as_bytes()),
    )?)?
    .trim()
    .to_string();
    Ok(real)
}

/// Deletes a ref on the real remote (an agent deleting its own branch).
pub fn delete_upstream(gw: &Gateway, refname: &str) -> Result<()> {
    git(
        &gw.repo,
        &["push", "-q", &gw.remote, &format!(":{refname}")],
    )?;
    Ok(())
}
