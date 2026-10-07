//! Step 9: high-confidence secret patterns in added lines. The matched
//! values never leave this module.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::secrets::find_secrets;
use onus_core::{ChangeKind, ChangeLevel, Location, SemanticChange};

use crate::ctx::{Ctx, change, join_and};

pub fn rows(ctx: &Ctx) -> Vec<SemanticChange> {
    let mut by_file: BTreeMap<&str, (BTreeSet<&'static str>, Vec<Location>)> = BTreeMap::new();
    for f in &ctx.text.files {
        for (line, text) in &f.added_lines {
            let found = find_secrets(text);
            if found.is_empty() {
                continue;
            }
            let entry = by_file.entry(f.path.as_str()).or_default();
            for m in found {
                entry.0.insert(m.description);
            }
            entry.1.push(Location::head(&f.path, *line, *line));
        }
    }
    let mut rows = Vec::new();
    for (file, (kinds, locations)) in by_file {
        let kinds: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
        let component = ctx.component_of_path(file);
        let mut row = change(
            ChangeKind::SecuritySensitive,
            "secret-committed",
            ChangeLevel::Behavior,
            file,
            (component != "root").then_some(component.as_str()),
            "Secret committed",
            format!("{} committed in `{file}`", capitalize(&join_and(&kinds))),
            "Anyone with access to the repository can use it; revoke it, then load it from a secret store (value not shown); needs a person".into(),
            locations,
        );
        row.id = format!("secret-committed:{file}");
        row.hints.needs_person = true;
        row.hints.novelty.push("secret".into());
        rows.push(row);
    }
    rows
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
