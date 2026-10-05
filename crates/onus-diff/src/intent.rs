//! The intent check (PLAN.md section 5.5): an optional `onus-intent` block
//! says what a change is meant to touch. Every change outside it is a
//! mismatch and goes to the top of the report.

use globset::Glob;
use onus_core::ids;
use onus_core::{IntentCheck, IntentMismatch, SemanticChange};
use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    #[serde(default)]
    pub summary: String,
    /// Component ids or path globs.
    #[serde(default)]
    pub touches: Vec<String>,
    /// Contract names (`UserPreferences`) or symbol ids.
    #[serde(default)]
    pub contracts: Vec<String>,
    /// Vendors, external ids or SDK package names.
    #[serde(default)]
    pub externals: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum IntentError {
    #[error("invalid onus-intent block: {0}")]
    Invalid(String),
}

/// Parses an intent from a YAML file, or from Markdown (such as a pull
/// request body) that contains a fenced `onus-intent` block. Returns `None`
/// when Markdown has no such block.
pub fn parse(text: &str) -> Result<Option<Intent>, IntentError> {
    let block = extract_block(text);
    let yaml = match &block {
        Some(b) => b.as_str(),
        None if looks_like_markdown(text) => return Ok(None),
        None => text,
    };
    serde_yaml_ng::from_str::<Intent>(yaml)
        .map(Some)
        .map_err(|e| IntentError::Invalid(e.to_string()))
}

fn looks_like_markdown(text: &str) -> bool {
    text.contains("```")
        || text
            .lines()
            .any(|l| l.starts_with('#') && !l.starts_with("#!"))
}

fn extract_block(text: &str) -> Option<String> {
    let mut lines = text.lines();
    for line in lines.by_ref() {
        let t = line.trim();
        if (t.starts_with("```") || t.starts_with("~~~"))
            && t.trim_start_matches(['`', '~']).trim() == "onus-intent"
        {
            let fence = &t[..3];
            let mut body = Vec::new();
            for l in lines.by_ref() {
                if l.trim().starts_with(fence) {
                    return Some(body.join("\n"));
                }
                body.push(l);
            }
            return Some(body.join("\n"));
        }
    }
    None
}

fn is_contract_change(c: &SemanticChange) -> bool {
    c.subkind.starts_with("contract-") || c.subkind.starts_with("export-")
}

fn is_external_change(c: &SemanticChange) -> bool {
    c.subkind.starts_with("new-external")
}

/// Why `change` is outside `intent`, if it is.
fn mismatch(
    intent: &Intent,
    change: &SemanticChange,
    externals: &[onus_core::ExternalService],
) -> Option<String> {
    let files: Vec<&str> = change.locations.iter().map(|l| l.file.as_str()).collect();
    let touched = match change.component.as_deref() {
        Some(c) => intent.touches.iter().any(|t| t == c),
        None => false,
    } || (!files.is_empty()
        && files.iter().all(|f| {
            intent.touches.iter().any(|t| {
                (t.contains('/') || t.contains('*'))
                    && Glob::new(t).is_ok_and(|g| g.compile_matcher().is_match(f))
            })
        }));
    if !touched {
        let declared = if intent.touches.is_empty() {
            "nothing".to_string()
        } else {
            intent.touches.join(", ")
        };
        return Some(match change.component.as_deref() {
            Some(c) => format!("`{c}` is not in `touches` ({declared})"),
            None => format!("repository-level change outside `touches` ({declared})"),
        });
    }
    if is_contract_change(change) {
        let name = ids::name_of(&change.subject);
        if !intent
            .contracts
            .iter()
            .any(|c| c == name || c == &change.subject)
        {
            return Some(format!("contract `{name}` is not in `contracts`"));
        }
    }
    if is_external_change(change) {
        let service = externals.iter().find(|e| e.id == change.subject);
        let names: Vec<String> = match service {
            Some(s) => {
                let mut v = vec![s.vendor.to_lowercase(), ids::name_of(&s.id).to_lowercase()];
                v.extend(s.packages.iter().map(|p| p.to_lowercase()));
                v
            }
            None => vec![ids::name_of(&change.subject).to_lowercase()],
        };
        if !intent
            .externals
            .iter()
            .any(|e| names.contains(&e.to_lowercase()))
        {
            let vendor = service.map_or(ids::name_of(&change.subject).to_string(), |s| {
                s.vendor.clone()
            });
            return Some(format!("external `{vendor}` is not in `externals`"));
        }
    }
    None
}

/// Marks mismatching changes and returns the check for the report.
pub fn check(
    intent: &Intent,
    changes: &mut [SemanticChange],
    externals: &[onus_core::ExternalService],
) -> IntentCheck {
    let mut mismatches = Vec::new();
    for c in changes.iter_mut() {
        if let Some(reason) = mismatch(intent, c, externals) {
            c.hints.intent_mismatch = true;
            c.hints.needs_person = true;
            mismatches.push(IntentMismatch {
                change_id: c.id.clone(),
                reason,
            });
        }
    }
    IntentCheck {
        stated: intent.summary.clone(),
        touches: intent.touches.clone(),
        contracts: intent.contracts.clone(),
        externals: intent.externals.clone(),
        mismatches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_block_from_a_pull_request_body() {
        let body = "Adds SMS alerts.\n\n```onus-intent\nsummary: Add SMS alerts on OrderShipped\ntouches: [notifications]\ncontracts: [UserPreferences]      # contracts I expect to change\nexternals: [twilio]\n```\n";
        let intent = parse(body).unwrap().unwrap();
        assert_eq!(intent.touches, ["notifications"]);
        assert_eq!(intent.contracts, ["UserPreferences"]);
        assert_eq!(intent.externals, ["twilio"]);
    }

    #[test]
    fn reads_plain_yaml_and_ignores_bodies_without_a_block() {
        let intent = parse("summary: x\ntouches: [logger]\n").unwrap().unwrap();
        assert_eq!(intent.touches, ["logger"]);
        assert_eq!(parse("# Title\n\nJust prose.\n").unwrap(), None);
        assert!(parse("summary: x\nunknown: 1\n").is_err());
    }
}
