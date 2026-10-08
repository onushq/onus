//! Task plans: what a task needs, written down before it starts.
//!
//! ```yaml
//! task: sms-alerts
//! writes: ["services/notifications/**"]
//! reads: ["services/orders/events/**"]     # optional
//! hosts: ["sms.test.example"]               # optional
//! secrets: ["SMS_TEST_KEY"]                 # optional
//! refs: ["refs/heads/onus/sms-alerts/*"]    # default: refs/heads/onus/<task>/*
//! ttl: 8h                                   # default 8h
//! ```
//!
//! The map suggests reads ([`suggest_reads`]): generous in code the task
//! touches, never in components labeled sensitive. Writes are exactly what
//! the plan says.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use onus_core::{CodebaseMap, ids};
use serde::{Deserialize, Serialize};

use crate::scope::{Kind, Right, glob_regex};
use crate::token::Grant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub task: String,
    #[serde(default)]
    pub writes: Vec<String>,
    #[serde(default)]
    pub reads: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub refs: Vec<String>,
    #[serde(default)]
    pub ttl: Option<String>,
}

impl Plan {
    pub fn parse(text: &str) -> Result<Plan> {
        let plan: Plan = serde_yaml_ng::from_str(text).context("not a task plan")?;
        if plan.task.is_empty()
            || !plan
                .task
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            bail!("the task id must be letters, digits, `-`, `_` or `.`");
        }
        if plan.writes.is_empty() {
            bail!("a plan lists the paths the task writes (`writes`)");
        }
        Ok(plan)
    }

    /// The plan's rights, reads first.
    pub fn rights(&self) -> Result<Vec<Right>> {
        let mut out = Vec::new();
        let mut push = |s: String| -> Result<()> {
            out.push(s.parse::<Right>().map_err(anyhow::Error::msg)?);
            Ok(())
        };
        for r in &self.reads {
            push(format!("read:path:{r}"))?;
        }
        for w in &self.writes {
            push(format!("write:path:{w}"))?;
        }
        for h in &self.hosts {
            push(format!("net:host:{h}"))?;
        }
        for s in &self.secrets {
            push(format!("secret:{s}"))?;
        }
        if self.refs.is_empty() {
            push(format!("push:ref:refs/heads/onus/{}/*", self.task))?;
        }
        for r in &self.refs {
            push(format!("push:ref:{r}"))?;
        }
        Ok(out)
    }

    /// Seconds the token lives (`30m`, `8h`, `2d`; default 8 hours).
    pub fn ttl_seconds(&self) -> Result<u64> {
        let Some(ttl) = &self.ttl else {
            return Ok(8 * 3600);
        };
        let (n, unit) = ttl.split_at(ttl.len().saturating_sub(1));
        let n: u64 = n
            .parse()
            .with_context(|| format!("`{ttl}` is not a duration like 30m, 8h or 2d"))?;
        Ok(n * match unit {
            "s" => 1,
            "m" => 60,
            "h" => 3600,
            "d" => 86_400,
            _ => bail!("`{ttl}` is not a duration like 30m, 8h or 2d"),
        })
    }

    pub fn grant(&self, now: u64) -> Result<Grant> {
        Ok(Grant {
            task: self.task.clone(),
            rights: self.rights()?,
            expires: now + self.ttl_seconds()?,
        })
    }
}

/// Reads the map suggests for a plan's writes, and the components left out
/// because they are labeled sensitive (they must be asked for).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub reads: Vec<String>,
    pub left_out: Vec<String>,
}

/// The components the written paths belong to, the components those use,
/// and the components that use them: their roots become reads. Components
/// labeled with one of `sensitive` are left out.
pub fn suggest_reads(map: &CodebaseMap, writes: &[String], sensitive: &[String]) -> Suggestion {
    let res: Vec<regex::Regex> = writes
        .iter()
        .filter_map(|w| regex::Regex::new(&glob_regex(w)).ok())
        .collect();
    let written: BTreeSet<String> = map
        .files
        .iter()
        .filter(|f| res.iter().any(|re| re.is_match(&f.path)))
        .filter_map(|f| f.component_id.clone())
        .collect();
    let mut related = written.clone();
    for e in &map.edges {
        if !e.kind.is_code_dependency() {
            continue;
        }
        let (Some(from), Some(to)) = (ids::component_of(&e.from), ids::component_of(&e.to)) else {
            continue;
        };
        if written.contains(from) {
            related.insert(to.to_string());
        }
        if written.contains(to) {
            related.insert(from.to_string());
        }
    }
    let mut reads = Vec::new();
    let mut left_out = Vec::new();
    for c in &map.components {
        if !related.contains(&c.id) {
            continue;
        }
        let labels: Vec<&String> = c.labels.iter().filter(|l| sensitive.contains(l)).collect();
        if !labels.is_empty() && !written.contains(&c.id) {
            left_out.push(format!(
                "`{}` ({})",
                c.id,
                labels
                    .iter()
                    .map(|l| l.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            continue;
        }
        reads.extend(c.roots.iter().cloned());
    }
    reads.sort();
    reads.dedup();
    Suggestion { reads, left_out }
}

/// Paths of `writes` that no read right covers are fine: writing implies
/// reading. This checks the converse for callers that build scopes by hand.
pub fn covers(rights: &[Right], kind: Kind, path: &str) -> bool {
    rights.iter().any(|r| r.allows(kind, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_become_rights_with_a_default_ref_and_ttl() {
        let plan = Plan::parse(
            "task: sms-alerts\nwrites: [\"services/notifications/**\"]\nhosts: [sms.test.example]\nsecrets: [SMS_TEST_KEY]\n",
        )
        .unwrap();
        let rights: Vec<String> = plan
            .rights()
            .unwrap()
            .iter()
            .map(|r| r.to_string())
            .collect();
        assert_eq!(
            rights,
            [
                "write:path:services/notifications/**",
                "net:host:sms.test.example",
                "secret:SMS_TEST_KEY",
                "push:ref:refs/heads/onus/sms-alerts/*"
            ]
        );
        assert_eq!(plan.grant(100).unwrap().expires, 100 + 8 * 3600);
        assert!(Plan::parse("task: x\nwrites: []\n").is_err());
        assert!(Plan::parse("task: \"a b\"\nwrites: [x]\n").is_err());
        assert!(
            Plan::parse("task: x\nwrites: [\"../etc\"]\n")
                .unwrap()
                .rights()
                .is_err()
        );
        let mut p = plan.clone();
        p.ttl = Some("30m".into());
        assert_eq!(p.ttl_seconds().unwrap(), 1800);
        p.ttl = Some("soon".into());
        assert!(p.ttl_seconds().is_err());
    }
}
