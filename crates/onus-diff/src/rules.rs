//! Boundary rules from `onus.yaml`, evaluated on the head map. Only
//! violations that are not already present in the base map are reported.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::ids;
use onus_core::{
    BoundaryRule, ChangeKind, ChangeLevel, CodebaseMap, Location, SemanticChange, Site, Visibility,
};

use crate::ctx::{Ctx, change, join_and, plural};
use crate::matching::Pairs;

/// `(rule id, from file, to id)` → sites.
type Violations = BTreeMap<(String, String, String), BTreeSet<Site>>;

fn target_is_internal(map: &CodebaseMap, to: &str, component: &str) -> bool {
    if to.contains('#') {
        return map
            .symbols
            .iter()
            .find(|s| s.id == to)
            .is_none_or(|s| s.visibility == Visibility::Internal);
    }
    // A whole module: internal unless it is one of the component's
    // entrypoints.
    let Some(c) = map.components.iter().find(|c| c.id == component) else {
        return true;
    };
    let path = ids::path_of(to).unwrap_or("");
    !c.public_entrypoints
        .iter()
        .any(|e| e.ends_with(&format!("/{path}")) || e == path)
}

pub fn violations(map: &CodebaseMap, rules: &[BoundaryRule]) -> Violations {
    let mut out: Violations = BTreeMap::new();
    for e in &map.edges {
        if !e.kind.is_code_dependency() {
            continue;
        }
        let (Some(fc), Some(tc)) = (ids::component_of(&e.from), ids::component_of(&e.to)) else {
            continue;
        };
        for r in rules {
            if r.deny.from != fc {
                continue;
            }
            let (to_comp, internal_only) = match r.deny.to.strip_suffix(".internal") {
                Some(c) => (c, true),
                None => (r.deny.to.as_str(), false),
            };
            if to_comp != tc {
                continue;
            }
            if internal_only && !target_is_internal(map, &e.to, tc) {
                continue;
            }
            for s in &e.sites {
                out.entry((r.id.clone(), s.file.clone(), e.to.clone()))
                    .or_default()
                    .insert(s.clone());
            }
        }
    }
    out
}

pub fn rows(ctx: &Ctx, pairs: &Pairs) -> Vec<SemanticChange> {
    let rules = &ctx.head.rules;
    if rules.is_empty() {
        return vec![];
    }
    let moves = ctx.moves.borrow();
    let base_to_head_path: BTreeMap<&str, &str> = moves
        .iter()
        .map(|(h, b)| (b.as_str(), h.as_str()))
        .collect();
    let map_id = |id: &str| -> String {
        if let Some(h) = pairs.renamed.get(id) {
            return h.clone();
        }
        id.to_string()
    };
    let base: BTreeSet<(String, String, String)> = violations(ctx.base, rules)
        .into_keys()
        .map(|(r, file, to)| {
            let file = base_to_head_path
                .get(file.as_str())
                .map(|s| s.to_string())
                .unwrap_or(file);
            (r, file, map_id(&to))
        })
        .collect();
    let head = violations(ctx.head, rules);
    // Group new violations by rule.
    let mut by_rule: BTreeMap<String, Vec<(&String, &BTreeSet<Site>)>> = BTreeMap::new();
    for (key, sites) in &head {
        if base.contains(key) {
            continue;
        }
        by_rule
            .entry(key.0.clone())
            .or_default()
            .push((&key.2, sites));
    }
    let mut rows = Vec::new();
    for (rule_id, hits) in by_rule {
        let Some(rule) = rules.iter().find(|r| r.id == rule_id) else {
            continue;
        };
        let mut targets: BTreeSet<String> = BTreeSet::new();
        let mut locations = Vec::new();
        let mut count = 0u32;
        for (to, sites) in &hits {
            targets.insert(format!("`{}`", ids::name_of(to)));
            for s in *sites {
                count += 1;
                locations.push(Location::head(&s.file, s.line, s.line));
            }
        }
        let to_comp = rule.deny.to.strip_suffix(".internal");
        let targets: Vec<String> = targets.into_iter().collect();
        let title = match to_comp {
            Some(c) => format!(
                "`{}` reaches into `{c}` internals ({})",
                rule.deny.from,
                join_and(&targets)
            ),
            None => format!(
                "`{}` now uses `{}` ({})",
                rule.deny.from,
                rule.deny.to,
                join_and(&targets)
            ),
        };
        let why = format!(
            "Breaks the declared rule \"{} may not use {}\" with {}; needs a person",
            rule.deny.from,
            match to_comp {
                Some(c) => format!("the internals of {c}"),
                None => rule.deny.to.clone(),
            },
            plural(count, "new reference", "new references")
        );
        let mut row = change(
            ChangeKind::Breaking,
            "rule-violation",
            ChangeLevel::Relationship,
            &rule.id,
            Some(&rule.deny.from),
            "Boundary rule violation",
            title,
            why,
            locations,
        );
        row.hints.blast_radius = hits.len() as u32;
        row.hints.rules_of_the_game = true;
        rows.push(row);
    }
    rows
}
