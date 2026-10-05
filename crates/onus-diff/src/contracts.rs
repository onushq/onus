//! Step 3: contract diff on public symbols.
//!
//! Optional member or parameter added → additive. Member or parameter
//! removed, required parameter or member added, export removed → breaking.
//! A type that changed in a way Onus cannot prove compatible → breaking,
//! subkind `contract-changed-unverified`.

use std::collections::BTreeMap;

use onus_core::{
    ChangeKind, ChangeLevel, Confidence, ContractShape, Location, Member, MemberKind, Param,
    SemanticChange, ShapeKind, SymbolKind, SymbolNode, Visibility,
};

use crate::ctx::{Ctx, change, join_and, plural};
use crate::matching::Pairs;

#[derive(Debug, Clone)]
struct Delta {
    breaking: bool,
    subkind: &'static str,
    phrase: String,
    unverified: bool,
}

impl Delta {
    fn additive(subkind: &'static str, phrase: String) -> Self {
        Delta {
            breaking: false,
            subkind,
            phrase,
            unverified: false,
        }
    }

    fn breaking(subkind: &'static str, phrase: String) -> Self {
        Delta {
            breaking: true,
            subkind,
            phrase,
            unverified: false,
        }
    }

    fn unverified(phrase: String) -> Self {
        Delta {
            breaking: true,
            subkind: "contract-changed-unverified",
            phrase,
            unverified: true,
        }
    }
}

fn member_noun(m: &Member, owner: SymbolKind) -> &'static str {
    match (m.kind, owner) {
        (MemberKind::EnumMember, _) => "member",
        (MemberKind::Method, _) => "method",
        (MemberKind::Constructor, _) => "constructor",
        (MemberKind::Property, _) => "field",
    }
}

fn type_or_unknown(t: &Option<String>) -> &str {
    t.as_deref().unwrap_or("an inferred type")
}

fn diff_members(owner: SymbolKind, base: &[Member], head: &[Member]) -> Vec<Delta> {
    let mut out = Vec::new();
    let b: BTreeMap<&str, &Member> = base.iter().map(|m| (m.name.as_str(), m)).collect();
    let h: BTreeMap<&str, &Member> = head.iter().map(|m| (m.name.as_str(), m)).collect();
    // Report in head order so titles read like the source.
    for m in head {
        let noun = member_noun(m, owner);
        match b.get(m.name.as_str()) {
            None => {
                let additive = m.optional
                    || m.kind == MemberKind::EnumMember
                    || (owner == SymbolKind::Class && m.kind != MemberKind::Constructor);
                if additive {
                    let phrase = if m.optional {
                        format!("gains an optional `{}` {noun}", m.name)
                    } else {
                        format!("gains a `{}` {noun}", m.name)
                    };
                    let sub = if m.optional {
                        "contract-field-added-optional"
                    } else {
                        "contract-member-added"
                    };
                    out.push(Delta::additive(sub, phrase));
                } else {
                    out.push(Delta::breaking(
                        "contract-field-added-required",
                        format!("gains a required `{}` {noun}", m.name),
                    ));
                }
            }
            Some(old) => {
                if old.type_text != m.type_text {
                    out.push(Delta::unverified(format!(
                        "`{}` changes type from `{}` to `{}`",
                        m.name,
                        type_or_unknown(&old.type_text),
                        type_or_unknown(&m.type_text)
                    )));
                }
                if old.optional && !m.optional {
                    out.push(Delta::breaking(
                        "contract-field-now-required",
                        format!("`{}` becomes required", m.name),
                    ));
                } else if !old.optional && m.optional {
                    out.push(Delta::breaking(
                        "contract-field-now-optional",
                        format!("`{}` becomes optional; readers may get `undefined`", m.name),
                    ));
                }
                if old.readonly != m.readonly && m.readonly {
                    out.push(Delta::breaking(
                        "contract-field-now-readonly",
                        format!("`{}` becomes readonly", m.name),
                    ));
                }
            }
        }
    }
    for m in base {
        if !h.contains_key(m.name.as_str()) {
            out.push(Delta::breaking(
                "contract-field-removed",
                format!("loses its `{}` {}", m.name, member_noun(m, owner)),
            ));
        }
    }
    out
}

fn diff_params(base: &[Param], head: &[Param]) -> Vec<Delta> {
    let mut out = Vec::new();
    for (i, p) in head.iter().enumerate() {
        match base.get(i) {
            None => {
                if p.optional || p.rest {
                    out.push(Delta::additive(
                        "contract-param-added-optional",
                        format!("gains an optional `{}` parameter", p.name),
                    ));
                } else {
                    out.push(Delta::breaking(
                        "contract-param-added-required",
                        format!("gains a required `{}` parameter", p.name),
                    ));
                }
            }
            Some(old) => {
                if old.type_text != p.type_text {
                    out.push(Delta::unverified(format!(
                        "parameter `{}` changes type from `{}` to `{}`",
                        p.name,
                        type_or_unknown(&old.type_text),
                        type_or_unknown(&p.type_text)
                    )));
                }
                if (old.optional || old.rest) && !(p.optional || p.rest) {
                    out.push(Delta::breaking(
                        "contract-param-now-required",
                        format!("parameter `{}` becomes required", p.name),
                    ));
                } else if !(old.optional || old.rest) && (p.optional || p.rest) {
                    out.push(Delta::additive(
                        "contract-param-now-optional",
                        format!("parameter `{}` becomes optional", p.name),
                    ));
                }
            }
        }
    }
    for p in base.iter().skip(head.len()) {
        out.push(Delta::breaking(
            "contract-param-removed",
            format!("loses its `{}` parameter", p.name),
        ));
    }
    out
}

fn diff_shapes(kind: SymbolKind, b: &ContractShape, h: &ContractShape) -> Vec<Delta> {
    let mut out = Vec::new();
    if b.kind != h.kind {
        out.push(Delta::unverified(format!(
            "changes from {} to {}",
            shape_noun(b.kind),
            shape_noun(h.kind)
        )));
        return out;
    }
    if b.type_params != h.type_params {
        out.push(Delta::unverified(format!(
            "changes its type parameters from `{}` to `{}`",
            b.type_params.as_deref().unwrap_or("none"),
            h.type_params.as_deref().unwrap_or("none")
        )));
    }
    match h.kind {
        ShapeKind::Function => {
            out.extend(diff_params(&b.params, &h.params));
            if b.returns != h.returns {
                out.push(Delta::unverified(format!(
                    "return type changes from `{}` to `{}`",
                    type_or_unknown(&b.returns),
                    type_or_unknown(&h.returns)
                )));
            }
        }
        ShapeKind::Object | ShapeKind::Class | ShapeKind::Enum => {
            if b.type_text != h.type_text {
                out.push(Delta::unverified(format!(
                    "changes `{}` to `{}`",
                    b.type_text.as_deref().unwrap_or("no heritage"),
                    h.type_text.as_deref().unwrap_or("no heritage")
                )));
            }
            out.extend(diff_members(kind, &b.members, &h.members));
        }
        ShapeKind::Alias | ShapeKind::Value => {
            if b.type_text != h.type_text {
                out.push(Delta::unverified(format!(
                    "type changes from `{}` to `{}`",
                    type_or_unknown(&b.type_text),
                    type_or_unknown(&h.type_text)
                )));
            }
        }
    }
    out
}

fn shape_noun(k: ShapeKind) -> &'static str {
    match k {
        ShapeKind::Function => "a function",
        ShapeKind::Object => "an object type",
        ShapeKind::Enum => "an enum",
        ShapeKind::Alias => "a type alias",
        ShapeKind::Class => "a class",
        ShapeKind::Value => "a value",
    }
}

fn signature_location(s: &SymbolNode, head: bool) -> Option<Location> {
    let l = s.loc.as_ref()?;
    let end = match s.shape.as_ref().map(|sh| sh.kind) {
        Some(ShapeKind::Function) => l.signature_end.unwrap_or(l.start),
        _ => l.end,
    };
    Some(if head {
        Location::head(&l.file, l.start, end)
    } else {
        Location::base(&l.file, l.start, end)
    })
}

pub fn rows(ctx: &Ctx, pairs: &Pairs) -> Vec<SemanticChange> {
    let mut rows = Vec::new();
    for (b, h, _) in &pairs.pairs {
        if h.kind == SymbolKind::Method {
            continue;
        }
        let mut deltas = Vec::new();
        match (b.visibility, h.visibility) {
            (Visibility::Public, Visibility::Internal) => deltas.push(Delta::breaking(
                "export-removed",
                format!(
                    "is no longer exported by `{}`",
                    h.component_id.as_deref().unwrap_or("")
                ),
            )),
            (Visibility::Internal, Visibility::Public) => deltas.push(Delta::additive(
                "export-added",
                format!(
                    "is now exported by `{}`",
                    h.component_id.as_deref().unwrap_or("")
                ),
            )),
            (Visibility::Public, Visibility::Public) => {
                if let (Some(bs), Some(hs)) = (&b.shape, &h.shape) {
                    deltas.extend(diff_shapes(h.kind, bs, hs));
                    let inferred = hs.unverified
                        && matches!(hs.kind, ShapeKind::Function | ShapeKind::Value)
                        && (hs.returns.is_none() || hs.kind == ShapeKind::Value)
                        && hs.type_text.is_none()
                        && b.body_fingerprint != h.body_fingerprint;
                    if inferred && deltas.is_empty() {
                        let mut d = Delta::unverified(
                            "may have changed its inferred type (no type annotation to compare)"
                                .into(),
                        );
                        d.subkind = "contract-changed-unverified";
                        deltas.push(d);
                    }
                }
            }
            (Visibility::Internal, Visibility::Internal) => {}
        }
        if deltas.is_empty() {
            continue;
        }
        let mut locations: Vec<Location> = signature_location(h, true).into_iter().collect();
        if deltas.iter().any(|d| d.subkind == "contract-field-removed") {
            locations.extend(signature_location(b, false));
        }
        rows.push(contract_row(ctx, h, deltas, locations));
    }
    for r in &pairs.removed {
        if r.visibility != Visibility::Public || r.kind == SymbolKind::Method {
            continue;
        }
        let deltas = vec![Delta::breaking(
            "export-removed",
            format!(
                "is removed from `{}`",
                r.component_id.as_deref().unwrap_or("")
            ),
        )];
        rows.push(contract_row(
            ctx,
            r,
            deltas,
            signature_location(r, false).into_iter().collect(),
        ));
    }
    // New exports: one row per symbol, or one per component when several.
    let mut new_exports: BTreeMap<&str, Vec<&SymbolNode>> = BTreeMap::new();
    for a in &pairs.added {
        if a.visibility == Visibility::Public && a.kind != SymbolKind::Method {
            new_exports
                .entry(a.component_id.as_deref().unwrap_or(""))
                .or_default()
                .push(a);
        }
    }
    for (component, symbols) in new_exports {
        if let [a] = symbols.as_slice() {
            let deltas = vec![Delta::additive(
                "export-added",
                format!("is a new export of `{component}`"),
            )];
            rows.push(contract_row(
                ctx,
                a,
                deltas,
                signature_location(a, true).into_iter().collect(),
            ));
            continue;
        }
        let names: Vec<String> = symbols
            .iter()
            .take(5)
            .map(|s| format!("`{}`", s.name))
            .collect();
        let more = if symbols.len() > 5 {
            format!(" and {} more", symbols.len() - 5)
        } else {
            String::new()
        };
        let mut row = change(
            ChangeKind::Additive,
            "export-added",
            ChangeLevel::Structure,
            component,
            Some(component),
            "Contract change, additive",
            format!("`{component}` exports {} new symbols", symbols.len()),
            format!(
                "New public surface: {}{more}; existing callers unaffected",
                names.join(", ")
            ),
            symbols
                .iter()
                .filter_map(|s| signature_location(s, true))
                .collect(),
        );
        row.id = format!("export-added:{component}");
        rows.push(row);
    }
    rows
}

fn contract_row(
    ctx: &Ctx,
    s: &SymbolNode,
    deltas: Vec<Delta>,
    locations: Vec<Location>,
) -> SemanticChange {
    let breaking = deltas.iter().any(|d| d.breaking);
    let unverified = deltas.iter().any(|d| d.unverified);
    let subkind = if !breaking {
        deltas[0].subkind
    } else {
        deltas
            .iter()
            .find(|d| d.breaking && !d.unverified)
            .map_or("contract-changed-unverified", |d| d.subkind)
    };
    let title = if deltas.len() == 1 {
        format!("`{}` {}", s.name, deltas[0].phrase)
    } else {
        let phrases: Vec<String> = deltas.iter().take(3).map(|d| d.phrase.clone()).collect();
        let more = if deltas.len() > 3 {
            format!(" (+{} more)", deltas.len() - 3)
        } else {
            String::new()
        };
        format!("`{}` {}{more}", s.name, join_and(&phrases))
    };
    let component = s.component_id.as_deref().unwrap_or("");
    let (files, comps) = ctx.dependents(&s.id);
    let used = if files == 0 {
        format!("Public contract of `{component}` with no other users in this repository yet")
    } else {
        format!(
            "Shared contract used in {} across {}",
            plural(files, "file", "files"),
            plural(comps.len() as u32, "component", "components")
        )
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
    let (kind, kind_label, why) = if !breaking {
        (
            ChangeKind::Additive,
            "Contract change, additive",
            format!("{used}; existing callers unaffected{invariants}"),
        )
    } else if subkind == "contract-changed-unverified" {
        (
            ChangeKind::Breaking,
            "Contract change, unverified",
            format!(
                "{used}; Onus cannot prove the change is compatible, so it is treated as breaking{invariants}"
            ),
        )
    } else {
        (
            ChangeKind::Breaking,
            "Contract change, breaking",
            format!("{used}; existing callers may break{invariants}"),
        )
    };
    let mut row = change(
        kind,
        subkind,
        ChangeLevel::Structure,
        &s.id,
        Some(component),
        kind_label,
        title,
        why,
        locations,
    );
    row.hints.blast_radius = files;
    if unverified
        && deltas.iter().all(|d| d.unverified)
        && s.shape.as_ref().is_some_and(|sh| sh.unverified)
    {
        row.hints.confidence = Confidence::Low;
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, ty: &str, optional: bool) -> Member {
        Member {
            name: name.into(),
            kind: MemberKind::Property,
            type_text: Some(ty.into()),
            optional,
            readonly: false,
            line: 1,
        }
    }

    fn param(name: &str, ty: &str, optional: bool) -> Param {
        Param {
            name: name.into(),
            type_text: Some(ty.into()),
            optional,
            rest: false,
        }
    }

    fn subkinds(d: &[Delta]) -> Vec<(&str, bool)> {
        d.iter().map(|d| (d.subkind, d.breaking)).collect()
    }

    #[test]
    fn optional_member_added_is_additive() {
        let base = vec![member("phone", "string", true)];
        let head = vec![
            member("phone", "string", true),
            member("phoneVerified", "boolean", true),
        ];
        let d = diff_members(SymbolKind::Interface, &base, &head);
        assert_eq!(subkinds(&d), [("contract-field-added-optional", false)]);
        assert_eq!(d[0].phrase, "gains an optional `phoneVerified` field");
    }

    #[test]
    fn required_member_added_or_removed_is_breaking() {
        let base = vec![member("a", "string", false), member("b", "string", false)];
        let head = vec![member("a", "string", false), member("c", "string", false)];
        let d = diff_members(SymbolKind::Interface, &base, &head);
        assert_eq!(
            subkinds(&d),
            [
                ("contract-field-added-required", true),
                ("contract-field-removed", true)
            ]
        );
    }

    #[test]
    fn changed_types_are_unverified_breaking() {
        let base = vec![member("a", "string", false)];
        let head = vec![member("a", "string|number", false)];
        let d = diff_members(SymbolKind::Interface, &base, &head);
        assert_eq!(subkinds(&d), [("contract-changed-unverified", true)]);
        assert!(d[0].unverified);
    }

    #[test]
    fn parameters() {
        let base = vec![param("to", "string", false)];
        let opt = vec![param("to", "string", false), param("body", "string", true)];
        let req = vec![param("to", "string", false), param("body", "string", false)];
        assert_eq!(
            subkinds(&diff_params(&base, &opt)),
            [("contract-param-added-optional", false)]
        );
        assert_eq!(
            subkinds(&diff_params(&base, &req)),
            [("contract-param-added-required", true)]
        );
        assert_eq!(
            subkinds(&diff_params(&opt, &base)),
            [("contract-param-removed", true)]
        );
        // Renaming a parameter is not a contract change.
        assert!(diff_params(&base, &[param("recipient", "string", false)]).is_empty());
    }

    #[test]
    fn return_type_change_is_unverified() {
        let shape = |ret: &str| ContractShape {
            kind: ShapeKind::Function,
            type_params: None,
            params: vec![],
            returns: Some(ret.into()),
            members: vec![],
            type_text: None,
            unverified: false,
        };
        let d = diff_shapes(
            SymbolKind::Function,
            &shape("Promise<void>"),
            &shape("Promise<boolean>"),
        );
        assert_eq!(subkinds(&d), [("contract-changed-unverified", true)]);
        assert!(diff_shapes(SymbolKind::Function, &shape("A"), &shape("A")).is_empty());
    }
}
