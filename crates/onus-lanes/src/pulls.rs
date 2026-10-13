//! What CI knows about each pull request while it is open: who made it,
//! its lane and verdict, and the rows people approved. When the pull
//! request closes, its latest record becomes an outcome; nobody has to
//! record outcomes by hand.

use std::path::Path;

use anyhow::{Context, Result};
use onus_core::{ApprovalRule, Lane};
use serde::{Deserialize, Serialize};

use crate::learned::Features;
use crate::rollout::RolloutPlan;
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
    /// The components the change touched (from its report): what per-area
    /// history counts reverts and incidents against.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    /// Everyone asked to review so far (`@login`, `@org/team`), so nobody
    /// is asked twice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested: Vec<String>,
    /// Who approved the head commit, from the reviews that counted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviewed_by: Vec<String>,
    /// Whose approval counted (`lanes.approval`, after any fallback), when
    /// a review was needed or given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRule>,
    /// `lanes.auditAfterMerge` when this change was classified; `ci closed`
    /// samples with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_after_merge: Option<f64>,
    /// What the change looked like when it was classified; `ci closed`
    /// copies it onto the outcome, for the learned lane model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Features>,
    /// A person with write access said Onus caught a problem in it
    /// (`/onus caught`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub caught: bool,
    /// The rollout plan for its final lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout: Option<RolloutPlan>,
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
    detect_agent_source(message, branch, author).0
}

/// [`detect_agent`], and what told: `trailer`, `branch`, `bot`,
/// `co-author` or `person`.
pub fn detect_agent_source(
    message: &str,
    branch: &str,
    author: &str,
) -> (AgentSetup, &'static str) {
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
                let model = parts.next().unwrap_or("");
                return (setup(tool, model, parts.next().unwrap_or("")), "trailer");
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
        return (setup(tool, "", ""), "branch");
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
        return (setup(tool, "", ""), "bot");
    }
    if login.ends_with("[bot]") {
        return (setup(bare, "", ""), "bot");
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
                    return (setup(tool, "", ""), "co-author");
                }
            }
        }
    }
    (
        setup(
            "person",
            if author.is_empty() { "unknown" } else { author },
            "",
        ),
        "person",
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
        // And what told.
        let source = |m: &str, b: &str, a: &str| detect_agent_source(m, b, a).1;
        assert_eq!(
            source("x\n\nOnus-Agent: aider", "claude/x", "mike"),
            "trailer"
        );
        assert_eq!(source("Fix", "codex/add-sms", "mike"), "branch");
        assert_eq!(source("Fix", "feature", "renovate[bot]"), "bot");
        assert_eq!(
            source(
                "Fix\n\nCo-authored-by: Claude <noreply@anthropic.com>",
                "feature",
                "mike"
            ),
            "co-author"
        );
        assert_eq!(source("Fix", "feature", "mike"), "person");
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
            components: vec![],
            requested: vec![],
            reviewed_by: vec![],
            approval: None,
            audit_after_merge: None,
            features: None,
            caught: false,
            rollout: None,
        };
        append(&path, &rec(Lane::Human, 1)).unwrap();
        append(&path, &rec(Lane::Judge, 2)).unwrap();
        let all = load(&path).unwrap();
        assert_eq!(latest(&all, "acme/shop#1").unwrap().lane, Lane::Judge);
        assert!(latest(&all, "acme/shop#2").is_none());
        // Review fields are left out when empty, so old lines stay the same.
        let line = serde_json::to_string(&rec(Lane::Human, 1)).unwrap();
        assert!(
            !line.contains("requested")
                && !line.contains("reviewedBy")
                && !line.contains("approval")
        );
        let mut reviewed = rec(Lane::Human, 3);
        reviewed.requested = vec!["@team-growth".into()];
        reviewed.reviewed_by = vec!["@grace".into()];
        reviewed.approval = Some(ApprovalRule::Owner);
        let line = serde_json::to_string(&reviewed).unwrap();
        assert!(line.contains(
            r#""requested":["@team-growth"],"reviewedBy":["@grace"],"approval":"owner""#
        ));
        assert_eq!(serde_json::from_str::<PullRecord>(&line).unwrap(), reviewed);
        // The audit share is left out of older lines and round-trips.
        assert!(!line.contains("auditAfterMerge"));
        let old: PullRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(old.audit_after_merge, None);
        let mut sampled = rec(Lane::AutoMerge, 4);
        sampled.audit_after_merge = Some(0.05);
        let line = serde_json::to_string(&sampled).unwrap();
        assert!(line.contains(r#""auditAfterMerge":0.05"#));
        assert_eq!(serde_json::from_str::<PullRecord>(&line).unwrap(), sampled);
    }
}
