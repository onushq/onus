//! Step 8: notable edits inside matched function bodies: flipped comparison
//! operators, changed constants in conditions, removed guards or throws,
//! removed `await` and swallowed errors. Promoted to security-sensitive in
//! labeled components.

use std::collections::BTreeMap;

use onus_core::{
    BodyFacts, ChangeKind, ChangeLevel, Comparison, FactSite, Location, SemanticChange, SymbolKind,
    SymbolNode,
};

use crate::ctx::{Ctx, change, join_and};
use crate::matching::Pairs;

#[derive(Debug)]
struct Edit {
    subkind: &'static str,
    label: &'static str,
    title: String,
    why: &'static str,
    location: Location,
}

/// Awaits, guards and throws that this change adds to other functions: a
/// body extracted into a helper takes them along, so their disappearance
/// from the original function is a move, not a removal.
#[derive(Debug, Default)]
struct Gained {
    awaits: BTreeMap<String, u32>,
    guards: BTreeMap<String, u32>,
    throws: BTreeMap<String, u32>,
}

impl Gained {
    fn collect(pairs: &Pairs) -> Gained {
        let mut g = Gained::default();
        let empty = BodyFacts::default();
        let mut add = |base: &BodyFacts, head: &BodyFacts| {
            for (into, b, h) in [
                (&mut g.awaits, &base.awaits, &head.awaits),
                (&mut g.guards, &base.guards, &head.guards),
                (&mut g.throws, &base.throws, &head.throws),
            ] {
                for f in multiset_minus(h, b) {
                    *into.entry(f.text.clone()).or_default() += 1;
                }
            }
        };
        for (b, h, _) in &pairs.pairs {
            if b.body_fingerprint != h.body_fingerprint {
                add(
                    b.facts.as_ref().unwrap_or(&empty),
                    h.facts.as_ref().unwrap_or(&empty),
                );
            }
        }
        for a in &pairs.added {
            add(&empty, a.facts.as_ref().unwrap_or(&empty));
        }
        g
    }

    /// Takes one occurrence of `text` from `pool`, if there is one.
    fn take(pool: &mut BTreeMap<String, u32>, text: &str) -> bool {
        match pool.get_mut(text) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        }
    }
}

fn multiset_minus<'a>(a: &'a [FactSite], b: &[FactSite]) -> Vec<&'a FactSite> {
    let mut counts: BTreeMap<&str, i32> = BTreeMap::new();
    for x in b {
        *counts.entry(x.text.as_str()).or_default() += 1;
    }
    a.iter()
        .filter(|x| {
            let c = counts.entry(x.text.as_str()).or_default();
            if *c > 0 {
                *c -= 1;
                false
            } else {
                true
            }
        })
        .collect()
}

fn mirror(op: &str) -> &str {
    match op {
        "<" => ">",
        ">" => "<",
        "<=" => ">=",
        ">=" => "<=",
        other => other,
    }
}

fn key(c: &Comparison) -> (String, String, String) {
    (c.op.clone(), c.left.clone(), c.right.clone())
}

/// Comparisons present on one side only, after removing exact and mirrored
/// matches (`a > b` equals `b < a`).
fn unmatched<'a>(
    base: &'a [Comparison],
    head: &'a [Comparison],
) -> (Vec<&'a Comparison>, Vec<&'a Comparison>) {
    let mut head_left: Vec<&Comparison> = head.iter().collect();
    let mut base_left = Vec::new();
    for b in base {
        let mirrored = (mirror(&b.op).to_string(), b.right.clone(), b.left.clone());
        if let Some(pos) = head_left
            .iter()
            .position(|h| key(h) == key(b) || key(h) == mirrored)
        {
            head_left.remove(pos);
        } else {
            base_left.push(b);
        }
    }
    (base_left, head_left)
}

fn is_constant(text: &str) -> bool {
    let t = text.trim_start_matches('-');
    t.starts_with('"')
        || t.chars().next().is_some_and(|c| c.is_ascii_digit())
        || (t
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
            && t.chars().any(|c| c.is_ascii_uppercase()))
}

fn edits<'a>(
    sym: &SymbolNode,
    b: &'a BodyFacts,
    h: &BodyFacts,
    base_file: &str,
    head_file: &str,
    moved: &mut Gained,
) -> Vec<Edit> {
    let mut out = Vec::new();
    let name = &sym.name;
    let (mut gone, mut new) = unmatched(&b.comparisons, &h.comparisons);
    // Flipped operators: same operands, different operator.
    let mut i = 0;
    while i < gone.len() {
        let bc = gone[i];
        if let Some(pos) = new.iter().position(|hc| {
            (hc.left == bc.left && hc.right == bc.right)
                || (hc.left == bc.right && hc.right == bc.left)
        }) {
            let hc = new.remove(pos);
            gone.remove(i);
            out.push(Edit {
                subkind: "boundary-condition-changed",
                label: "Boundary condition changed",
                title: format!(
                    "Boundary condition changed in `{name}`: `{} {} {}` is now `{} {} {}`",
                    bc.left, bc.op, bc.right, hc.left, hc.op, hc.right
                ),
                why: "Values exactly at the boundary now take the other branch",
                location: Location::head(head_file, hc.line, hc.line),
            });
            continue;
        }
        i += 1;
    }
    // Changed constants: same operator and one side, the other side a
    // different constant.
    let mut i = 0;
    while i < gone.len() {
        let bc = gone[i];
        if let Some(pos) = new.iter().position(|hc| {
            hc.op == bc.op
                && ((hc.left == bc.left && is_constant(&hc.right) && is_constant(&bc.right))
                    || (hc.right == bc.right && is_constant(&hc.left) && is_constant(&bc.left)))
        }) {
            let hc = new.remove(pos);
            gone.remove(i);
            out.push(Edit {
                subkind: "condition-constant-changed",
                label: "Condition changed",
                title: format!(
                    "Condition changed in `{name}`: `{} {} {}` is now `{} {} {}`",
                    bc.left, bc.op, bc.right, hc.left, hc.op, hc.right
                ),
                why: "The threshold or value a decision depends on changed",
                location: Location::head(head_file, hc.line, hc.line),
            });
            continue;
        }
        i += 1;
    }
    // Only a lower count means something was removed; a guard whose
    // condition was edited is still a guard.
    let fewer = |base: &'a [FactSite], head: &[FactSite]| -> Vec<&'a FactSite> {
        let n = base.len().saturating_sub(head.len());
        multiset_minus(base, head).into_iter().take(n).collect()
    };
    let throws: Vec<&FactSite> = fewer(&b.throws, &h.throws)
        .into_iter()
        .filter(|f| !Gained::take(&mut moved.throws, &f.text))
        .collect();
    let guards: Vec<&FactSite> = fewer(&b.guards, &h.guards)
        .into_iter()
        .filter(|f| !Gained::take(&mut moved.guards, &f.text))
        .collect();
    if !throws.is_empty() || !guards.is_empty() {
        let mut what: Vec<String> = guards
            .iter()
            .map(|g| format!("the early exit `if {}`", g.text))
            .collect();
        what.extend(throws.iter().map(|t| format!("`{}`", t.text)));
        let first = guards
            .first()
            .or(throws.first())
            .map(|s| s.line)
            .unwrap_or(1);
        out.push(Edit {
            subkind: "guard-removed",
            label: "Guard removed",
            title: format!("`{name}` no longer has {}", join_and(&what)),
            why: "Inputs that used to be rejected now reach the rest of the function",
            location: Location::base(base_file, first, first),
        });
    }
    let awaits = multiset_minus(&b.awaits, &h.awaits);
    let new_awaits = multiset_minus(&h.awaits, &b.awaits);
    let removed_awaits: Vec<&&FactSite> = awaits
        .iter()
        .filter(|a| !new_awaits.iter().any(|n| n.text == a.text))
        .filter(|a| !Gained::take(&mut moved.awaits, &a.text))
        .collect();
    if !removed_awaits.is_empty() && h.awaits.len() < b.awaits.len() {
        let what: Vec<String> = removed_awaits
            .iter()
            .map(|a| format!("`{}`", a.text))
            .collect();
        out.push(Edit {
            subkind: "await-removed",
            label: "Await removed",
            title: format!("`{name}` no longer awaits {}", join_and(&what)),
            why: "The work may now run unawaited: errors can go unhandled and ordering can change",
            location: Location::base(base_file, removed_awaits[0].line, removed_awaits[0].line),
        });
    }
    let swallowed = multiset_minus(&h.empty_catches, &b.empty_catches);
    if let Some(first) = swallowed.first() {
        out.push(Edit {
            subkind: "error-swallowed",
            label: "Error swallowed",
            title: format!("`{name}` now catches and ignores errors"),
            why: "Failures in this code path are no longer reported",
            location: Location::head(head_file, first.line, first.line),
        });
    }
    out
}

/// A `const` or variable whose literal value changed: `LOYALTY_RATE` from
/// `0.1` to `0.15`.
fn constant_edit(b: &SymbolNode, h: &SymbolNode) -> Option<Edit> {
    if !matches!(h.kind, SymbolKind::Const | SymbolKind::Variable) {
        return None;
    }
    let (Some(old), Some(new)) = (&b.literal, &h.literal) else {
        return None;
    };
    if old == new || old.contains("<redacted>") || new.contains("<redacted>") {
        return None;
    }
    let l = h.loc.as_ref()?;
    let show = |v: &str| -> String {
        if v.parse::<f64>().is_ok() || v == "true" || v == "false" {
            v.to_string()
        } else {
            format!("\"{v}\"")
        }
    };
    Some(Edit {
        subkind: "constant-changed",
        label: "Constant changed",
        title: format!(
            "`{}` changes from `{}` to `{}`",
            h.name,
            show(old),
            show(new)
        ),
        why: "Code that reads this value now behaves differently",
        location: Location::head(&l.file, l.start, l.start),
    })
}

pub fn rows(ctx: &Ctx, pairs: &Pairs) -> Vec<SemanticChange> {
    let mut rows = Vec::new();
    let mut moved = Gained::collect(pairs);
    for (b, h, _) in &pairs.pairs {
        if b.body_fingerprint == h.body_fingerprint {
            continue;
        }
        let empty = BodyFacts::default();
        let bf = b.facts.as_ref().unwrap_or(&empty);
        let hf = h.facts.as_ref().unwrap_or(&empty);
        let (Some(bl), Some(hl)) = (&b.loc, &h.loc) else {
            continue;
        };
        let component = h.component_id.clone().unwrap_or_default();
        let labels = ctx.labels(&component);
        let mut all = edits(h, bf, hf, &bl.file, &hl.file, &mut moved);
        all.extend(constant_edit(b, h));
        for e in all {
            let sensitive = !labels.is_empty();
            let why = if sensitive {
                format!(
                    "{} in a {} component; needs a person",
                    e.why,
                    labels.join(", ")
                )
            } else {
                e.why.to_string()
            };
            let mut row = change(
                if sensitive {
                    ChangeKind::SecuritySensitive
                } else {
                    ChangeKind::Internal
                },
                e.subkind,
                ChangeLevel::Behavior,
                &h.id,
                Some(&component),
                e.label,
                e.title,
                why,
                vec![e.location],
            );
            row.hints.needs_person = sensitive;
            row.hints.blast_radius = ctx.dependents(&h.id).0;
            rows.push(row);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(op: &str, l: &str, r: &str) -> Comparison {
        Comparison {
            op: op.into(),
            left: l.into(),
            right: r.into(),
            line: 3,
        }
    }

    fn facts(c: Vec<Comparison>) -> BodyFacts {
        BodyFacts {
            comparisons: c,
            ..BodyFacts::default()
        }
    }

    fn sym() -> SymbolNode {
        SymbolNode {
            id: "billing:src/d.ts#applyDiscount".into(),
            component_id: Some("billing".into()),
            kind: onus_core::SymbolKind::Function,
            name: "applyDiscount".into(),
            visibility: onus_core::Visibility::Public,
            shape: None,
            body_fingerprint: None,
            invariants: vec![],
            loc: None,
            facts: None,
            literal: None,
        }
    }

    #[test]
    fn detects_flipped_operators() {
        let b = facts(vec![cmp(">", "order.subtotalCents", "LIMIT")]);
        let h = facts(vec![cmp(">=", "order.subtotalCents", "LIMIT")]);
        let e = edits(&sym(), &b, &h, "a.ts", "a.ts", &mut Gained::default());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].subkind, "boundary-condition-changed");
        assert!(
            e[0].title
                .contains("`order.subtotalCents > LIMIT` is now `order.subtotalCents >= LIMIT`")
        );
    }

    #[test]
    fn mirrored_comparisons_are_equal() {
        let b = facts(vec![cmp(">", "a", "b")]);
        let h = facts(vec![cmp("<", "b", "a")]);
        assert!(edits(&sym(), &b, &h, "a.ts", "a.ts", &mut Gained::default()).is_empty());
    }

    #[test]
    fn detects_changed_constants_and_removed_guards() {
        let mut b = facts(vec![cmp(">", "total", "10000")]);
        b.throws = vec![FactSite {
            text: "throw new Error(\"x\")".into(),
            line: 2,
        }];
        let h = facts(vec![cmp(">", "total", "5000")]);
        let e = edits(&sym(), &b, &h, "a.ts", "a.ts", &mut Gained::default());
        let kinds: Vec<&str> = e.iter().map(|e| e.subkind).collect();
        assert_eq!(kinds, ["condition-constant-changed", "guard-removed"]);
    }
}
