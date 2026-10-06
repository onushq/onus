//! `onus init`: a starter `onus.yaml` inferred from the tree.
//!
//! Components, paths and owners are inferred from workspaces and
//! `CODEOWNERS`. Sensitivity labels are only *suggested*, as comments: Onus
//! never enforces a label nobody confirmed.

use std::fmt::Write as _;
use std::path::Path;

use onus_core::Component;

use crate::discover;
use crate::walk;

/// Name fragments that suggest a sensitivity label.
pub const LABEL_HINTS: &[(&str, &[&str])] = &[
    (
        "payments",
        &[
            "billing", "payment", "checkout", "invoice", "stripe", "wallet",
        ],
    ),
    (
        "auth",
        &["auth", "login", "session", "identity", "oauth", "sso"],
    ),
    (
        "pii",
        &[
            "user",
            "profile",
            "preference",
            "account",
            "customer",
            "contact",
        ],
    ),
];

/// Labels suggested for a component, with the fragment that triggered each.
pub fn suggested_labels(component: &Component) -> Vec<(&'static str, &'static str)> {
    let haystack = format!("{} {}", component.id, component.roots.join(" ")).to_lowercase();
    let mut out = Vec::new();
    for (label, hints) in LABEL_HINTS {
        if let Some(h) = hints.iter().find(|h| haystack.contains(*h)) {
            out.push((*label, *h));
        }
    }
    out
}

/// The text of a starter `onus.yaml` for the tree at `root`.
pub fn init_config(root: &Path) -> String {
    let files = walk::list_files(root);
    let discovered = discover::discover(root, &files, None);
    let mut out = String::new();
    let _ = writeln!(out, "# onus.yaml: the declared layer for Onus.");
    let _ = writeln!(
        out,
        "# Components and owners below were inferred from {} and CODEOWNERS.",
        match discovered.source {
            "nx" => "Nx projects and package workspaces",
            "manifests" => "Cargo and Python project manifests",
            "workspaces" => "package workspaces",
            _ => "top-level folders",
        }
    );
    let _ = writeln!(
        out,
        "# Suggested labels are commented out: confirm them before relying on them."
    );
    let _ = writeln!(out, "version: 1");
    if discovered.components.is_empty() {
        let _ = writeln!(out, "components: {{}}");
    } else {
        let _ = writeln!(out, "components:");
    }
    for c in &discovered.components {
        let _ = writeln!(out, "  {}:", c.id);
        let _ = writeln!(out, "    path: \"{}\"", c.roots.join(","));
        if !c.owners.is_empty() {
            let owners: Vec<String> = c.owners.iter().map(|o| format!("\"{o}\"")).collect();
            let _ = writeln!(out, "    owners: [{}]", owners.join(", "));
        }
        let labels = suggested_labels(c);
        if !labels.is_empty() {
            let names: Vec<&str> = labels.iter().map(|(l, _)| *l).collect();
            let why: Vec<String> = labels.iter().map(|(_, h)| format!("\"{h}\"")).collect();
            let _ = writeln!(
                out,
                "    # labels: [{}]  # suggestion (inferred from {}); uncomment to enforce",
                names.join(", "),
                why.join(", ")
            );
        }
    }
    let _ = writeln!(out, "labels:");
    for l in ["payments", "auth", "pii"] {
        let _ = writeln!(out, "  {l}: {{ sensitivity: high }}");
    }
    let _ = writeln!(out, "# rules:");
    let _ = writeln!(
        out,
        "#   - deny: {{ from: <component>, to: <component>.internal }}"
    );
    let _ = writeln!(out, "extractors:");
    let _ = writeln!(out, "  events:");
    let _ = writeln!(out, "    publish: [\"bus.publish($EVENT, ...)\"]");
    let _ = writeln!(out, "    subscribe: [\"bus.subscribe($EVENT, ...)\"]");
    let _ = writeln!(
        out,
        "  # externals:  # SDKs that are not in Onus's built-in registry"
    );
    let _ = writeln!(
        out,
        "  #   \"@vendor/sdk\": {{ vendor: \"Vendor\", category: sms, egress: [phone] }}"
    );
    out
}
