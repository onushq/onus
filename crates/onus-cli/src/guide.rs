//! The user guide, shipped inside the binary so `onus help <topic>` works
//! offline. The Markdown files live in `crates/onus-cli/guide/`; the
//! repository's `docs/guide.md` links to them.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Topic {
    pub name: &'static str,
    pub summary: &'static str,
    pub text: &'static str,
}

pub const TOPICS: &[Topic] = &[
    Topic {
        name: "getting-started",
        summary: "Install Onus, build a map and read your first report",
        text: include_str!("../guide/getting-started.md"),
    },
    Topic {
        name: "commands",
        summary: "Every command and flag, with examples and exit codes",
        text: include_str!("../guide/commands.md"),
    },
    Topic {
        name: "reports",
        summary: "Reading a report: rows, ranking, evidence and the JSON",
        text: include_str!("../guide/reports.md"),
    },
    Topic {
        name: "changes",
        summary: "Every kind and subkind of change Onus reports",
        text: include_str!("../guide/changes.md"),
    },
    Topic {
        name: "configuration",
        summary: "The onus.yaml reference: components, rules, labels, extractors",
        text: include_str!("../guide/configuration.md"),
    },
    Topic {
        name: "intent",
        summary: "Checking a change against its stated intent",
        text: include_str!("../guide/intent.md"),
    },
    Topic {
        name: "ci",
        summary: "Running Onus on every pull request",
        text: include_str!("../guide/ci.md"),
    },
    Topic {
        name: "agents",
        summary: "The map for coding agents: onus mcp, its tools, many agents and worktrees",
        text: include_str!("../guide/agents.md"),
    },
    Topic {
        name: "plugins",
        summary: "New languages and frameworks, SCIP indexes, language servers, trusted mode",
        text: include_str!("../guide/plugins.md"),
    },
    Topic {
        name: "how-it-works",
        summary: "How maps are built and compared",
        text: include_str!("../guide/how-it-works.md"),
    },
    Topic {
        name: "troubleshooting",
        summary: "Map confidence notes, common questions and current limits",
        text: include_str!("../guide/troubleshooting.md"),
    },
];

/// Other names people reach for.
const ALIASES: &[(&str, &str)] = &[
    ("start", "getting-started"),
    ("quickstart", "getting-started"),
    ("config", "configuration"),
    ("onus.yaml", "configuration"),
    ("kinds", "changes"),
    ("faq", "troubleshooting"),
    ("scip", "plugins"),
    ("lsp", "plugins"),
];

pub fn find(name: &str) -> Option<&'static Topic> {
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, target)| target);
    TOPICS.iter().find(|t| t.name == name)
}

/// The topic list shown by `onus help`.
pub fn topic_list() -> String {
    let width = TOPICS.iter().map(|t| t.name.len()).max().unwrap_or(0);
    let mut out = String::from("Guide topics (onus help <topic>):\n");
    for t in TOPICS {
        out.push_str(&format!("  {:<width$}  {}\n", t.name, t.summary));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_a_title_and_aliases_resolve() {
        for t in TOPICS {
            assert!(t.text.starts_with("# "), "{} has no title", t.name);
        }
        for (alias, target) in ALIASES {
            assert_eq!(find(alias).map(|t| t.name), Some(*target));
        }
        assert!(find("nope").is_none());
    }
}
