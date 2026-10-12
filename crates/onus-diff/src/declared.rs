//! Declared contracts are rules of the game: an edit to the code of a
//! symbol named under `contracts` in onus.yaml goes to a person, whatever
//! kind of change it is (ADR 0014).

use onus_core::{ChangeKind, ChangeLevel, ContractShape, Location, SemanticChange, SymbolNode};

use crate::ctx::{Ctx, change};

pub fn mark(ctx: &Ctx, rows: &mut Vec<SemanticChange>) {
    for (symbol, name) in &ctx.declared {
        let base = ctx.base_syms.get(symbol.as_str()).copied();
        let head = ctx.head_syms.get(symbol.as_str()).copied();
        if !edited(ctx, symbol, base, head) {
            continue;
        }
        let member = format!("{symbol}.");
        let mut matched = false;
        for r in rows
            .iter_mut()
            .filter(|r| r.subject == *symbol || r.subject.starts_with(&member))
        {
            matched = true;
            r.hints.rules_of_the_game = true;
            r.hints.needs_person = true;
            r.why_it_matters.push_str(&format!(
                "; `{name}` is a declared contract (onus.yaml), so a person approves changes to it"
            ));
        }
        if matched {
            continue;
        }
        let Some(s) = head.or(base) else {
            continue;
        };
        let invariants = if s.invariants.is_empty() {
            String::new()
        } else {
            format!(
                "; declared {}: {}",
                if s.invariants.len() == 1 {
                    "invariant"
                } else {
                    "invariants"
                },
                s.invariants.join("; ")
            )
        };
        let locations = match (
            head.and_then(|h| h.loc.as_ref()),
            base.and_then(|b| b.loc.as_ref()),
        ) {
            (Some(l), _) => vec![Location::head(&l.file, l.start, l.end)],
            (None, Some(l)) => vec![Location::base(&l.file, l.start, l.end)],
            (None, None) => vec![],
        };
        let mut row = change(
            ChangeKind::Internal,
            "declared-contract-edited",
            ChangeLevel::Behavior,
            symbol,
            s.component_id.as_deref(),
            "Declared contract",
            format!("Declared contract `{name}` (`{}`) changes", s.name),
            format!(
                "`{name}` is declared in onus.yaml{invariants}; its code changed, so a person approves it"
            ),
            locations,
        );
        row.hints.rules_of_the_game = true;
        rows.push(row);
    }
}

/// Whether the declared symbol or one of its members changed: added,
/// removed, a different body, shape or value. Fingerprints ignore
/// formatting and comments, and shapes are compared without line numbers,
/// so moving or reformatting it is not an edit.
fn edited(ctx: &Ctx, symbol: &str, base: Option<&SymbolNode>, head: Option<&SymbolNode>) -> bool {
    let (Some(b), Some(h)) = (base, head) else {
        return base.is_some() || head.is_some();
    };
    if differs(b, h) {
        return true;
    }
    let member = format!("{symbol}.");
    let members = |syms: &std::collections::HashMap<&str, &SymbolNode>| {
        let mut m: Vec<String> = syms
            .keys()
            .filter(|id| id.starts_with(&member))
            .map(|id| id.to_string())
            .collect();
        m.sort();
        m
    };
    let (bm, hm) = (members(&ctx.base_syms), members(&ctx.head_syms));
    if bm != hm {
        return true;
    }
    bm.iter()
        .any(|id| differs(ctx.base_syms[id.as_str()], ctx.head_syms[id.as_str()]))
}

fn differs(b: &SymbolNode, h: &SymbolNode) -> bool {
    let body = matches!(
        (&b.body_fingerprint, &h.body_fingerprint),
        (Some(x), Some(y)) if x != y
    );
    body || shape(&b.shape) != shape(&h.shape) || b.literal != h.literal
}

fn shape(s: &Option<ContractShape>) -> Option<ContractShape> {
    s.clone().map(|mut s| {
        for m in &mut s.members {
            m.line = 0;
        }
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{component, ctx, file, function, map};
    use crate::text::TreeDiff;
    use onus_core::OnusConfig;
    use serde_json::json;

    const LOGGER: &str = "logger:src/logger.ts#createLogger";

    fn config() -> OnusConfig {
        serde_json::from_value(json!({
            "version": 1,
            "contracts": {
                "Logger": { "symbol": LOGGER },
                "Prefs": { "symbol": "logger:src/types.ts#Prefs" },
            },
        }))
        .unwrap()
    }

    fn prefs(start: u32, members: &[&str]) -> serde_json::Value {
        let members: Vec<serde_json::Value> = members
            .iter()
            .enumerate()
            .map(|(i, m)| {
                json!({ "name": m, "kind": "property", "optional": false, "line": start + 1 + i as u32 })
            })
            .collect();
        json!({
            "id": "logger:src/types.ts#Prefs", "componentId": "logger", "kind": "interface",
            "name": "Prefs", "visibility": "public",
            "shape": { "kind": "object", "members": members, "unverified": false },
            "loc": { "file": "packages/logger/src/types.ts", "start": start, "end": start + 3 },
        })
    }

    fn side(
        logger: serde_json::Value,
        prefs: serde_json::Value,
        other: &str,
    ) -> onus_core::CodebaseMap {
        map(json!({
            "components": [component("logger")],
            "files": [file("logger", "src/logger.ts"), file("logger", "src/types.ts")],
            "symbols": [logger, prefs, function("logger:src/logger.ts#helper", 20, other)],
        }))
    }

    fn rows_for(
        base: &onus_core::CodebaseMap,
        head: &onus_core::CodebaseMap,
        rows: &mut Vec<SemanticChange>,
    ) {
        let text = TreeDiff::default();
        let cfg = config();
        let c = ctx(base, head, &text, Some(&cfg));
        mark(&c, rows);
    }

    #[test]
    fn a_declared_function_body_edit_adds_a_row() {
        let base = side(function(LOGGER, 1, "a"), prefs(1, &["a"]), "x");
        let head = side(function(LOGGER, 1, "b"), prefs(1, &["a"]), "x");
        let mut rows = vec![];
        rows_for(&base, &head, &mut rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].subkind, "declared-contract-edited");
        assert_eq!(
            rows[0].title,
            "Declared contract `Logger` (`createLogger`) changes"
        );
        assert!(rows[0].hints.rules_of_the_game);
        assert_eq!(
            rows[0].locations,
            [Location::head("packages/logger/src/logger.ts", 1, 3)]
        );
    }

    #[test]
    fn a_declared_shape_change_flags_its_contract_row() {
        let base = side(function(LOGGER, 1, "a"), prefs(1, &["a"]), "x");
        let head = side(function(LOGGER, 1, "a"), prefs(1, &["a", "b"]), "x");
        let mut rows = vec![change(
            ChangeKind::Additive,
            "contract-field-added-optional",
            ChangeLevel::Structure,
            "logger:src/types.ts#Prefs",
            Some("logger"),
            "",
            "`Prefs` gains a field".into(),
            "Shared contract".into(),
            vec![],
        )];
        rows_for(&base, &head, &mut rows);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].hints.rules_of_the_game && rows[0].hints.needs_person);
        assert!(
            rows[0]
                .why_it_matters
                .contains("`Prefs` is a declared contract")
        );
    }

    #[test]
    fn moves_formatting_and_undeclared_edits_are_not_edits() {
        // Same fingerprints and members, other lines.
        let base = side(function(LOGGER, 1, "a"), prefs(1, &["a"]), "x");
        let head = side(function(LOGGER, 10, "a"), prefs(30, &["a"]), "y");
        let mut rows = vec![];
        rows_for(&base, &head, &mut rows);
        assert!(rows.is_empty(), "{rows:?}");
    }
}
