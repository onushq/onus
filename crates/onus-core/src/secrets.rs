//! High-confidence secret patterns (in the style of gitleaks rules).
//!
//! Only patterns with a distinctive prefix or context are included, so a
//! match is very likely a real credential. Matched values are never written
//! to any output.

use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug)]
pub struct SecretRule {
    pub id: &'static str,
    pub description: &'static str,
    regex: Regex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    pub rule_id: &'static str,
    pub description: &'static str,
    pub start: usize,
    pub end: usize,
}

const RULES: &[(&str, &str, &str)] = &[
    (
        "aws-access-key-id",
        "AWS access key ID",
        r"\b(?:AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}\b",
    ),
    (
        "aws-secret-access-key",
        "AWS secret access key",
        r#"(?i)aws.{0,20}secret.{0,20}['"`][A-Za-z0-9/+=]{40}['"`]"#,
    ),
    (
        "github-token",
        "GitHub token",
        r"\bgh[pousr]_[A-Za-z0-9]{36,255}\b",
    ),
    (
        "github-fine-grained-token",
        "GitHub fine-grained token",
        r"\bgithub_pat_[A-Za-z0-9_]{82}\b",
    ),
    (
        "stripe-secret-key",
        "Stripe secret key",
        r"\b(?:sk|rk)_live_[0-9a-zA-Z]{24,99}\b",
    ),
    (
        "slack-token",
        "Slack token",
        r"\bxox[baprs]-[0-9A-Za-z-]{10,250}",
    ),
    (
        "google-api-key",
        "Google API key",
        r"\bAIza[0-9A-Za-z_\-]{35}\b",
    ),
    (
        "sendgrid-api-key",
        "SendGrid API key",
        r"\bSG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}\b",
    ),
    ("npm-token", "npm access token", r"\bnpm_[A-Za-z0-9]{36}\b"),
    (
        "private-key",
        "Private key",
        r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY(?: BLOCK)?-----",
    ),
];

pub fn rules() -> &'static [SecretRule] {
    static COMPILED: OnceLock<Vec<SecretRule>> = OnceLock::new();
    COMPILED.get_or_init(|| {
        RULES
            .iter()
            .map(|(id, description, pattern)| SecretRule {
                id,
                description,
                regex: Regex::new(pattern).expect("secret patterns are valid"),
            })
            .collect()
    })
}

/// All secret matches in one line of text.
pub fn find_secrets(line: &str) -> Vec<SecretMatch> {
    let mut out = Vec::new();
    for rule in rules() {
        for m in rule.regex.find_iter(line) {
            out.push(SecretMatch {
                rule_id: rule.id,
                description: rule.description,
                start: m.start(),
                end: m.end(),
            });
        }
    }
    out.sort_by_key(|m| (m.start, m.rule_id));
    out
}

pub fn contains_secret(text: &str) -> bool {
    rules().iter().any(|r| r.regex.is_match(text))
}

/// Replaces every secret in `text` with `<redacted>`.
pub fn redact(text: &str) -> String {
    if !contains_secret(text) {
        return text.to_string();
    }
    let mut out = text.to_string();
    for rule in rules() {
        out = rule.regex.replace_all(&out, "<redacted>").into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_aws_keys_and_redacts_them() {
        let line = r#"const AWS_ACCESS_KEY_ID = "AKIAIOSFODNN7EXAMPLE";"#;
        let found = find_secrets(line);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule_id, "aws-access-key-id");
        assert!(!redact(line).contains("AKIA"));

        let secret = r#"const AWS_SECRET_ACCESS_KEY = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";"#;
        assert_eq!(find_secrets(secret)[0].rule_id, "aws-secret-access-key");
    }

    #[test]
    fn ignores_ordinary_code() {
        for line in [
            "const key = process.env.ACME_SMS_API_KEY;",
            "if (order.subtotalCents > DISCOUNT_THRESHOLD_CENTS) {",
            "const sk = 'sk_test_123';",
        ] {
            assert!(find_secrets(line).is_empty(), "{line}");
        }
    }
}
