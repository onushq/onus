//! What CI knows about each pull request while it is open: who made it,
//! its lane and verdict, and the rows people approved. When the pull
//! request closes, its latest record becomes an outcome; nobody has to
//! record outcomes by hand.

use std::path::Path;

use anyhow::{Context, Result};
use onus_core::Lane;
use serde::{Deserialize, Serialize};

use crate::submission::{AgentSetup, Approval};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRecord {
    /// Unix seconds.
    pub at: u64,
    /// `owner/repo#123`.
    pub change: String,
    pub base: String,
    pub head: String,
    pub agent: AgentSetup,
    pub lane: Lane,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<Approval>,
}

pub fn load(path: &Path) -> Result<Vec<PullRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).with_context(|| {
                format!("line {} of {} is not a pull record", i + 1, path.display())
            })
        })
        .collect()
}

pub fn append(path: &Path, record: &PullRecord) -> Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}", serde_json::to_string(record)?)?;
    Ok(())
}

/// The latest record of a change.
pub fn latest<'a>(records: &'a [PullRecord], change: &str) -> Option<&'a PullRecord> {
    records.iter().rev().find(|r| r.change == change)
}

/// Who made a change, from what git and GitHub show. In order: an
/// `Onus-Agent: tool[/model[/config]]` trailer on the head commit; a
/// branch named after an agent (`claude/…`, `codex/…`); a bot author; a
/// `Co-authored-by` trailer naming an agent; else the person who opened
/// the pull request.
pub fn detect_agent(message: &str, branch: &str, author: &str) -> AgentSetup {
    let setup = |tool: &str, model: &str, config: &str| AgentSetup {
        tool: tool.into(),
        model: model.into(),
        config: config.into(),
        team: String::new(),
    };
    for line in message.lines() {
        if let Some(v) = trailer(line, "onus-agent") {
            let mut parts = v.split('/').map(str::trim);
            let tool = parts.next().unwrap_or("");
            if !tool.is_empty() {
                return setup(tool, parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            }
        }
    }
    const BRANCHES: &[(&str, &str)] = &[
        ("claude/", "claude-code"),
        ("claude-code/", "claude-code"),
        ("codex/", "codex"),
        ("cursor/", "cursor"),
        ("copilot/", "copilot"),
        ("devin/", "devin"),
        ("jules/", "jules"),
        ("aider/", "aider"),
        ("onus/", "onus-gateway"),
    ];
    if let Some((_, tool)) = BRANCHES.iter().find(|(p, _)| branch.starts_with(p)) {
        return setup(tool, "", "");
    }
    let login = author.to_ascii_lowercase();
    const BOTS: &[(&str, &str)] = &[
        ("copilot-swe-agent", "copilot"),
        ("copilot", "copilot"),
        ("devin-ai-integration", "devin"),
        ("cursor", "cursor"),
        ("cursoragent", "cursor"),
        ("claude", "claude-code"),
        ("chatgpt-codex-connector", "codex"),
        ("google-labs-jules", "jules"),
    ];
    let bare = login.trim_end_matches("[bot]");
    if let Some((_, tool)) = BOTS.iter().find(|(b, _)| *b == bare) {
        return setup(tool, "", "");
    }
    if login.ends_with("[bot]") {
        return setup(bare, "", "");
    }
    for line in message.lines() {
        if let Some(v) = trailer(line, "co-authored-by") {
            let who = v.to_ascii_lowercase();
            for (name, tool) in [
                ("claude", "claude-code"),
                ("codex", "codex"),
                ("copilot", "copilot"),
                ("cursor", "cursor"),
                ("devin", "devin"),
                ("aider", "aider"),
            ] {
                if who.contains(name) {
                    return setup(tool, "", "");
                }
            }
        }
    }
    setup(
        "person",
        if author.is_empty() { "unknown" } else { author },
        "",
    )
}

fn trailer<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (k, v) = line.split_once(':')?;
    k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_are_recognised_from_what_git_and_github_show() {
        let key = |m: &str, b: &str, a: &str| detect_agent(m, b, a).key();
        assert_eq!(
            key(
                "Fix\n\nOnus-Agent: claude-code/opus/plan-first\n",
                "fix/x",
                "mike"
            ),
            "claude-code/opus/plan-first"
        );
        assert_eq!(key("Fix", "codex/add-sms", "mike"), "codex//");
        assert_eq!(key("Fix", "feature", "Copilot"), "copilot//");
        assert_eq!(
            key("Fix", "feature", "devin-ai-integration[bot]"),
            "devin//"
        );
        assert_eq!(key("Fix", "feature", "renovate[bot]"), "renovate//");
        assert_eq!(
            key(
                "Fix\n\nCo-authored-by: Claude <noreply@anthropic.com>",
                "feature",
                "mike"
            ),
            "claude-code//"
        );
        assert_eq!(key("Fix", "feature", "mike"), "person/mike/");
        // The trailer wins over everything else.
        assert_eq!(
            key("x\n\nonus-agent: aider", "claude/x", "Copilot"),
            "aider//"
        );
    }

    #[test]
    fn the_latest_record_of_a_change_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pulls.jsonl");
        let rec = |lane: Lane, at: u64| PullRecord {
            at,
            change: "acme/shop#1".into(),
            base: "b".into(),
            head: "h".into(),
            agent: detect_agent("", "codex/x", ""),
            lane,
            verdict: None,
            judge: None,
            approvals: vec![],
        };
        append(&path, &rec(Lane::Human, 1)).unwrap();
        append(&path, &rec(Lane::Judge, 2)).unwrap();
        let all = load(&path).unwrap();
        assert_eq!(latest(&all, "acme/shop#1").unwrap().lane, Lane::Judge);
        assert!(latest(&all, "acme/shop#2").is_none());
    }
}
