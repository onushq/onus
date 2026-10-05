//! Built-in registry of external service SDKs, merged with the externals
//! declared in `onus.yaml`.

use std::collections::BTreeMap;

use onus_core::ids::slug;
use onus_core::{
    DEFAULT_PUBLISH_PATTERNS, DEFAULT_SUBSCRIBE_PATTERNS, ExternalSpec, OnusConfig,
    ResolvedExternal, ResolvedExtractors,
};

/// `(package, slug, vendor, category, egress, hosts)`.
type Entry = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

pub const BUILT_IN: &[Entry] = &[
    (
        "twilio",
        "twilio",
        "Twilio",
        "sms",
        &["phone"],
        &["api.twilio.com"],
    ),
    (
        "stripe",
        "stripe",
        "Stripe",
        "payments",
        &["payment"],
        &["api.stripe.com"],
    ),
    (
        "@sendgrid/mail",
        "sendgrid",
        "SendGrid",
        "email",
        &["email"],
        &["api.sendgrid.com"],
    ),
    (
        "@sendgrid/client",
        "sendgrid",
        "SendGrid",
        "email",
        &["email"],
        &["api.sendgrid.com"],
    ),
    ("nodemailer", "smtp", "SMTP email", "email", &["email"], &[]),
    (
        "mailgun.js",
        "mailgun",
        "Mailgun",
        "email",
        &["email"],
        &["api.mailgun.net"],
    ),
    (
        "postmark",
        "postmark",
        "Postmark",
        "email",
        &["email"],
        &["api.postmarkapp.com"],
    ),
    (
        "@aws-sdk/client-ses",
        "aws-ses",
        "AWS SES",
        "email",
        &["email"],
        &[],
    ),
    (
        "@aws-sdk/client-sns",
        "aws-sns",
        "AWS SNS",
        "sms",
        &["phone"],
        &[],
    ),
    (
        "@aws-sdk/client-s3",
        "aws-s3",
        "AWS S3",
        "storage",
        &["files"],
        &[],
    ),
    (
        "@slack/web-api",
        "slack",
        "Slack",
        "chat",
        &["text"],
        &["slack.com"],
    ),
    (
        "openai",
        "openai",
        "OpenAI",
        "ai",
        &["text"],
        &["api.openai.com"],
    ),
    (
        "@anthropic-ai/sdk",
        "anthropic",
        "Anthropic",
        "ai",
        &["text"],
        &["api.anthropic.com"],
    ),
    (
        "@segment/analytics-node",
        "segment",
        "Segment",
        "analytics",
        &["pii"],
        &["api.segment.io"],
    ),
    (
        "posthog-node",
        "posthog",
        "PostHog",
        "analytics",
        &["pii"],
        &[],
    ),
    (
        "@sentry/node",
        "sentry",
        "Sentry",
        "monitoring",
        &["errors"],
        &[],
    ),
    (
        "plaid",
        "plaid",
        "Plaid",
        "payments",
        &["bank-account"],
        &[],
    ),
    (
        "braintree",
        "braintree",
        "Braintree",
        "payments",
        &["payment"],
        &[],
    ),
];

/// Merges defaults, the built-in registry and `onus.yaml`.
pub fn resolve_extractors(config: Option<&OnusConfig>) -> ResolvedExtractors {
    let mut externals = BTreeMap::new();
    for (pkg, s, vendor, category, egress, hosts) in BUILT_IN {
        externals.insert(
            pkg.to_string(),
            ResolvedExternal {
                slug: s.to_string(),
                spec: ExternalSpec {
                    vendor: vendor.to_string(),
                    category: category.to_string(),
                    egress: egress.iter().map(|e| e.to_string()).collect(),
                    hosts: hosts.iter().map(|h| h.to_string()).collect(),
                },
                declared: false,
            },
        );
    }
    let mut publish: Vec<String> = DEFAULT_PUBLISH_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut subscribe: Vec<String> = DEFAULT_SUBSCRIBE_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut prisma_clients = vec!["prisma".to_string()];
    if let Some(cfg) = config {
        for (pkg, spec) in &cfg.extractors.externals {
            let mut spec = spec.clone();
            spec.egress.sort();
            spec.hosts.sort();
            externals.insert(
                pkg.clone(),
                ResolvedExternal {
                    slug: slug(&spec.vendor),
                    spec,
                    declared: true,
                },
            );
        }
        if let Some(events) = &cfg.extractors.events {
            publish = events.publish.clone();
            subscribe = events.subscribe.clone();
        }
        if let Some(p) = &cfg.extractors.prisma {
            if !p.clients.is_empty() {
                prisma_clients = p.clients.clone();
            }
        }
    }
    ResolvedExtractors {
        publish_patterns: publish,
        subscribe_patterns: subscribe,
        externals,
        prisma_clients,
    }
}
