//! What an environment is made of: `environment:` in onus.yaml, else the
//! repository's `.devcontainer/devcontainer.json`. Both are read from the
//! operator's working tree, never from the commit under test.

use std::path::Path;

use anyhow::{Context, Result, bail};
use onus_core::EnvironmentConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lockfiles whose contents decide when a warm image is rebuilt, unless
/// `lockfiles` names others.
pub const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "bun.lock",
    "Cargo.lock",
    "go.sum",
    "poetry.lock",
    "uv.lock",
    "requirements.txt",
    "Gemfile.lock",
    "composer.lock",
];

pub const EGRESS_IMAGE: &str = "ubuntu/squid";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub traces: Vec<String>,
    pub lockfiles: Vec<String>,
    pub egress_image: String,
}

/// The environment of the repository at `root`: onus.yaml's `environment`
/// (its missing fields filled from the devcontainer), else the devcontainer.
pub fn resolve(root: &Path, config: Option<&EnvironmentConfig>) -> Result<Spec> {
    let dev = devcontainer(root)?;
    let c = config.cloned().unwrap_or_default();
    let image = c
        .image
        .or_else(|| dev.as_ref().and_then(|d| d.image.clone()))
        .with_context(|| {
            format!(
                "{} declares no environment: add `environment: {{ image: … }}` to onus.yaml \
                 or a .devcontainer/devcontainer.json with an image",
                root.display()
            )
        })?;
    Ok(Spec {
        image,
        setup: c.setup.or_else(|| dev.and_then(|d| d.setup)),
        seed: c.seed,
        evidence: c.evidence,
        traces: c.traces,
        lockfiles: if c.lockfiles.is_empty() {
            LOCKFILES.iter().map(|s| s.to_string()).collect()
        } else {
            c.lockfiles
        },
        egress_image: c.egress_image.unwrap_or_else(|| EGRESS_IMAGE.to_string()),
    })
}

#[derive(Debug, Default)]
struct Devcontainer {
    image: Option<String>,
    setup: Option<String>,
}

fn devcontainer(root: &Path) -> Result<Option<Devcontainer>> {
    let path = [".devcontainer/devcontainer.json", ".devcontainer.json"]
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.is_file());
    let Some(path) = path else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(&path)?;
    let v: Value = serde_json::from_str(&strip_jsonc(&text))
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    let image = v.get("image").and_then(Value::as_str).map(str::to_string);
    if image.is_none() && (v.get("build").is_some() || v.get("dockerFile").is_some()) {
        bail!(
            "{} builds its image from a Dockerfile; build it and set `environment.image` in onus.yaml",
            path.display()
        );
    }
    // The lifecycle commands that prepare the container, in the order a
    // devcontainer runs them.
    let setup: Vec<String> = [
        "onCreateCommand",
        "updateContentCommand",
        "postCreateCommand",
    ]
    .iter()
    .filter_map(|k| v.get(*k).and_then(command_line))
    .collect();
    Ok(Some(Devcontainer {
        image,
        setup: (!setup.is_empty()).then(|| setup.join(" && ")),
    }))
}

/// A devcontainer command: a string, an argument list, or named commands.
fn command_line(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(
            a.iter()
                .filter_map(Value::as_str)
                .map(quote)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Value::Object(o) => {
            let parts: Vec<String> = o.values().filter_map(command_line).collect();
            (!parts.is_empty()).then(|| parts.join(" && "))
        }
        _ => None,
    }
}

fn quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./=:@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Removes `//` and `/* */` comments and trailing commas (JSON with
/// comments, as devcontainer.json allows).
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas before a closing bracket.
    regex::Regex::new(r",(\s*[\]}])")
        .expect("valid regex")
        .replace_all(&out, "$1")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devcontainer_fills_what_onus_yaml_leaves_out() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".devcontainer")).unwrap();
        std::fs::write(
            dir.path().join(".devcontainer/devcontainer.json"),
            r#"{
  // The team's image.
  "image": "mcr.microsoft.com/devcontainers/typescript-node:22", /* pinned */
  "onCreateCommand": ["npm", "ci"],
  "postCreateCommand": { "db": "npm run db:migrate", "url": "echo 'http://x//y'" },
}"#,
        )
        .unwrap();
        let spec = resolve(dir.path(), None).unwrap();
        assert_eq!(
            spec.image,
            "mcr.microsoft.com/devcontainers/typescript-node:22"
        );
        assert_eq!(
            spec.setup.as_deref(),
            Some("npm ci && npm run db:migrate && echo 'http://x//y'")
        );
        assert!(spec.lockfiles.contains(&"package-lock.json".to_string()));

        let config = EnvironmentConfig {
            image: Some("node:22-alpine".into()),
            seed: Some("node scripts/seed.mjs".into()),
            ..Default::default()
        };
        let spec = resolve(dir.path(), Some(&config)).unwrap();
        assert_eq!(spec.image, "node:22-alpine");
        assert_eq!(
            spec.setup.as_deref().map(|s| s.starts_with("npm ci")),
            Some(true)
        );
        assert_eq!(spec.seed.as_deref(), Some("node scripts/seed.mjs"));
        assert_eq!(spec.egress_image, EGRESS_IMAGE);
    }

    #[test]
    fn no_environment_is_explained() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(dir.path(), None).unwrap_err().to_string();
        assert!(err.contains("declares no environment"), "{err}");
    }
}
