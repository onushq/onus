//! A task's mirror: a bare repository whose one commit holds only what the
//! task may read. Paths outside the read scope are not in its objects or
//! history at all, so neither `git show` nor anything else can reach them.

use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{Gateway, TaskState, git, git_str, git_with, ls_tree};

/// The fixed identity of snapshot commits, so the same base and scope give
/// the same root commit.
const SNAPSHOT_ENV: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Onus gateway"),
    ("GIT_AUTHOR_EMAIL", "gateway@onus.invalid"),
    ("GIT_AUTHOR_DATE", "1970-01-01T00:00:00Z"),
    ("GIT_COMMITTER_NAME", "Onus gateway"),
    ("GIT_COMMITTER_EMAIL", "gateway@onus.invalid"),
    ("GIT_COMMITTER_DATE", "1970-01-01T00:00:00Z"),
];

impl Gateway {
    /// Makes the mirror for a task, or returns the task's state when it
    /// exists. `onus` is the program the mirror's hooks run.
    pub fn ensure_mirror(
        &self,
        token: &crate::token::Verified,
        raw_token: &str,
        onus: &Path,
    ) -> Result<TaskState> {
        let task = token.task.as_str();
        if let Ok(mut state) = self.load_task(task) {
            if state.token != raw_token {
                state.token = raw_token.to_string();
                self.save_task(&state)?;
            }
            return Ok(state);
        }
        let scope = token.scope();
        let now = super::now();
        // An attenuated token reads less than its first block says: check
        // each candidate path with the token itself.
        let readable_by_token = |path: &str| {
            token.attenuations.is_empty()
                || token.authorize(crate::scope::Kind::Read, path, now).is_ok()
        };
        std::fs::create_dir_all(&self.state)?;
        let base = git_str(
            &self.repo,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", self.base),
            ],
        )
        .with_context(|| format!("the gateway's base `{}` is not a commit", self.base))?;
        let readable: Vec<String> = ls_tree(&self.repo, &base)?
            .into_iter()
            // Submodules are never part of a snapshot.
            .filter(|e| e.mode != "160000" && scope.can_read(&e.path) && readable_by_token(&e.path))
            .map(|e| format!("{} {}\t{}", e.mode, e.sha, e.path))
            .collect();
        if readable.is_empty() {
            bail!("the task's read scope matches no file of `{}`", self.base);
        }
        // A tree of the readable files only, built in a temporary index.
        let index = self.state.join(format!("{task}.index"));
        let index_s = index.to_string_lossy().to_string();
        let env_index = [("GIT_INDEX_FILE", index_s.as_str())];
        let _ = std::fs::remove_file(&index);
        git_with(&self.repo, &["read-tree", "--empty"], &env_index, None)?;
        let mut input = readable.join("\n");
        input.push('\n');
        git_with(
            &self.repo,
            &["update-index", "--index-info"],
            &env_index,
            Some(input.as_bytes()),
        )?;
        let tree = String::from_utf8(git_with(&self.repo, &["write-tree"], &env_index, None)?)?
            .trim()
            .to_string();
        let _ = std::fs::remove_file(&index);
        let message = format!(
            "Readable snapshot of {} for task {task}\n\nOnus gateway: only paths the task may read are here.\n",
            &base[..12]
        );
        let root = String::from_utf8(git_with(
            &self.repo,
            &["commit-tree", &tree],
            SNAPSHOT_ENV,
            Some(message.as_bytes()),
        )?)?
        .trim()
        .to_string();
        let root_ref = format!("refs/onus/{task}/root");
        git(&self.repo, &["update-ref", &root_ref, &root])?;

        // The bare mirror, holding the snapshot as its `main`.
        let mirror = self.mirror_path(task);
        if mirror.exists() {
            std::fs::remove_dir_all(&mirror)?;
        }
        let mirror_s = mirror.to_string_lossy().to_string();
        git(&self.state, &["init", "-q", "--bare", &mirror_s])?;
        let repo_s = self.repo.to_string_lossy().to_string();
        git(
            &mirror,
            &[
                "fetch",
                "-q",
                &repo_s,
                &format!("{root_ref}:refs/heads/main"),
            ],
        )?;
        git(&mirror, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
        for (k, v) in [
            ("http.receivepack", "true"),
            ("receive.denyDeletes", "false"),
            ("receive.fsckObjects", "true"),
            ("core.hooksPath", "hooks"),
        ] {
            git(&mirror, &["config", k, v])?;
        }
        install_hooks(&mirror, onus, &self.state, task)?;
        let state = TaskState {
            task: task.to_string(),
            base,
            root,
            scope,
            expires: token.expires,
            token: raw_token.to_string(),
        };
        self.save_task(&state)?;
        Ok(state)
    }
}

/// Hook scripts that call back into Onus with the gateway's settings. They
/// are written by the gateway, never by an agent: the mirror's hooks folder
/// is not part of anything a push can change.
fn install_hooks(mirror: &Path, onus: &Path, state: &Path, task: &str) -> Result<()> {
    let hooks = mirror.join("hooks");
    std::fs::create_dir_all(&hooks)?;
    for entry in std::fs::read_dir(&hooks)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "sample") {
            std::fs::remove_file(path)?;
        }
    }
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let onus_s = onus.to_string_lossy().replace('\\', "/");
    let state_s = state.to_string_lossy().replace('\\', "/");
    for (name, stage) in [
        ("pre-receive", "pre-receive"),
        ("post-receive", "post-receive"),
    ] {
        let script = format!(
            "#!/bin/sh\nexec {} gateway hook {stage} --state {} --task {}\n",
            quote(&onus_s),
            quote(&state_s),
            quote(task)
        );
        let path = hooks.join(name);
        std::fs::write(&path, script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}
