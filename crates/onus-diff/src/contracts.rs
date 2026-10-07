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
use crate::matching::{PairKind, Pairs};

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
                match (
                    b.type_text.as_deref().and_then(union_members),
                    h.type_text.as_deref().and_then(union_members),
                ) {
                    (Some(bu), Some(hu)) => out.extend(diff_unions(&bu, &hu)),
                    _ => out.push(Delta::unverified(format!(
                        "type changes from `{}` to `{}`",
                        compact(type_or_unknown(&b.type_text)),
                        compact(type_or_unknown(&h.type_text))
                    ))),
                }
            }
            // An unannotated value whose initializer's keys are known.
            if b.type_text.is_none()
                && h.type_text.is_none()
                && (!b.members.is_empty() || !h.members.is_empty())
            {
                out.extend(diff_keys(&b.members, &h.members));
            }
        }
    }
    out
}

/// Type text longer than this is cut in titles.
const MAX_TYPE_TEXT: usize = 80;

fn compact(t: &str) -> String {
    if t.chars().count() <= MAX_TYPE_TEXT {
        t.to_string()
    } else {
        let cut: String = t.chars().take(MAX_TYPE_TEXT - 1).collect();
        format!("{cut}…")
    }
}

/// The members of a union type (`'a' | 'b' | Foo<X | Y>`), split at the
/// top level only; `None` when the type is not a union.
fn union_members(t: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for ch in t.chars() {
        match quote {
            Some(q) => {
                cur.push(ch);
                if ch == q && prev != '\\' {
                    quote = None;
                }
            }
            None => match ch {
                '\'' | '"' | '`' => {
                    quote = Some(ch);
                    cur.push(ch);
                }
                '<' | '(' | '[' | '{' => {
                    depth += 1;
                    cur.push(ch);
                }
                // `=>` closes nothing.
                '>' if prev == '=' => cur.push(ch),
                '>' | ')' | ']' | '}' => {
                    depth -= 1;
                    cur.push(ch);
                }
                '|' if depth == 0 => {
                    parts.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(ch),
            },
        }
        prev = ch;
    }
    parts.push(cur.trim().to_string());
    parts.retain(|p| !p.is_empty());
    (parts.len() >= 2).then_some(parts)
}

/// Members a union gained or lost; reordering is no change.
fn diff_unions(base: &[String], head: &[String]) -> Vec<Delta> {
    let added: Vec<String> = head
        .iter()
        .filter(|m| !base.contains(m))
        .map(|m| format!("`{}`", compact(m)))
        .collect();
    let removed: Vec<String> = base
        .iter()
        .filter(|m| !head.contains(m))
        .map(|m| format!("`{}`", compact(m)))
        .collect();
    let mut out = Vec::new();
    if !added.is_empty() {
        out.push(Delta::additive(
            "contract-union-widened",
            format!("accepts {} too", listed(&added)),
        ));
    }
    if !removed.is_empty() {
        out.push(Delta::breaking(
            "contract-union-narrowed",
            format!("no longer accepts {}", listed(&removed)),
        ));
    }
    out
}

/// Keys an unannotated value's initializer gained, lost or changed.
fn diff_keys(base: &[Member], head: &[Member]) -> Vec<Delta> {
    let b: BTreeMap<&str, &Member> = base.iter().map(|m| (m.name.as_str(), m)).collect();
    let h: BTreeMap<&str, &Member> = head.iter().map(|m| (m.name.as_str(), m)).collect();
    let added: Vec<String> = head
        .iter()
        .filter(|m| !b.contains_key(m.name.as_str()))
        .map(|m| format!("`{}`", m.name))
        .collect();
    let removed: Vec<String> = base
        .iter()
        .filter(|m| !h.contains_key(m.name.as_str()))
        .map(|m| format!("`{}`", m.name))
        .collect();
    let mut out = Vec::new();
    if !added.is_empty() {
        out.push(Delta::additive(
            "contract-key-added",
            if added.len() == 1 {
                format!("gains a {} key", added[0])
            } else {
                format!("gains keys {}", listed(&added))
            },
        ));
    }
    if !removed.is_empty() {
        out.push(Delta::breaking(
            "contract-key-removed",
            if removed.len() == 1 {
                format!("loses its {} key", removed[0])
            } else {
                format!("loses keys {}", listed(&removed))
            },
        ));
    }
    for m in head {
        let Some(old) = b.get(m.name.as_str()) else {
            continue;
        };
        if old.type_text == m.type_text {
            continue;
        }
        let shown = |t: &Option<String>| {
            t.as_deref()
                .filter(|t| !t.starts_with('#'))
                .map(str::to_string)
        };
        out.push(Delta::unverified(
            match (shown(&old.type_text), shown(&m.type_text)) {
                (Some(a), Some(z)) => format!("`{}` changes from `{a}` to `{z}`", m.name),
                _ => format!("changes its `{}` value", m.name),
            },
        ));
    }
    out
}

/// `a`, `b`, `c` and 4 more.
fn listed(items: &[String]) -> String {
    const SHOWN: usize = 4;
    if items.len() <= SHOWN + 1 {
        join_and(items)
    } else {
        let mut head: Vec<String> = items[..SHOWN].to_vec();
        head.push(format!("{} more", items.len() - SHOWN));
        join_and(&head)
    }
}

/// `(breaking, phrase)` for each difference between two shapes.
pub(crate) fn shape_changes(
    kind: SymbolKind,
    b: &ContractShape,
    h: &ContractShape,
) -> Vec<(bool, String)> {
    diff_shapes(kind, b, h)
        .into_iter()
        .map(|d| (d.breaking, d.phrase))
        .collect()
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
    let mut opaque: Vec<&SymbolNode> = Vec::new();
    for (b, h, kind) in &pairs.pairs {
        if h.kind == SymbolKind::Method {
            continue;
        }
        // Moves between components are reported, with their contract
        // changes, by the move row.
        if matches!(kind, PairKind::Move | PairKind::MoveWithChanges)
            && b.component_id != h.component_id
        {
            continue;
        }
        let mut deltas = Vec::new();
        match (b.visibility, h.visibility) {
            (Visibility::Public, Visibility::Internal) => deltas.push(Delta::breaking(
                "export-removed",
                format!(
                    "is no longer exported by `{}`",
                    b.component_id.as_deref().unwrap_or("")
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
                    // Nothing to compare: noted once for all such exports,
                    // not flagged one by one (they are mostly unchanged).
                    if inferred && deltas.is_empty() {
                        opaque.push(h);
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
    if !opaque.is_empty() {
        rows.push(opaque_row(&opaque));
    }
    for r in &pairs.removed {
        if r.visibility != Visibility::Public || r.kind == SymbolKind::Method {
            continue;
        }
        let deltas = vec![Delta::breaking(
            "export-removed",
            if r.kind == SymbolKind::HttpRoute {
                format!(
                    "is no longer served by `{}`",
                    r.component_id.as_deref().unwrap_or("")
                )
            } else {
                format!(
                    "is removed from `{}`",
                    r.component_id.as_deref().unwrap_or("")
                )
            },
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
                if a.kind == SymbolKind::HttpRoute {
                    format!("is a new HTTP route of `{component}`")
                } else {
                    format!("is a new export of `{component}`")
                },
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

/// Who a breaking change can still break: the files outside this change
/// that use the symbol, or, when the only breaking change is a new required
/// member, the ones that implement it (code that merely reads the type is
/// unaffected).
#[derive(Debug, Default)]
struct Reach {
    /// Files outside the change and the line of use, test files first.
    left: Vec<(String, u32)>,
    /// How many of `left` are test files.
    tests_left: usize,
    /// Files in the change that use or implement it.
    updated: Vec<String>,
    /// `use it` or `implement it`.
    noun: &'static str,
    /// Why users Onus cannot see may exist.
    unseen: Option<String>,
}

/// Files with the line where they use a symbol.
type Uses = Vec<(String, u32)>;

fn reach(ctx: &Ctx, s: &SymbolNode, deltas: &[Delta], removed: bool) -> Reach {
    let changed: std::collections::BTreeSet<&str> =
        ctx.text.files.iter().map(|f| f.path.as_str()).collect();
    let sites = if removed {
        ctx.base_dependent_sites(&s.id)
    } else {
        ctx.dependent_sites(&s.id)
    };
    let only_new_required = !removed
        && deltas
            .iter()
            .filter(|d| d.breaking)
            .all(|d| d.subkind == "contract-field-added-required");
    let (sites, noun) = if only_new_required {
        let implementing = sites
            .iter()
            .filter_map(|(f, _)| implementation_line(ctx, f, &s.name).map(|l| (f.clone(), l)))
            .collect();
        (implementing, "implement it")
    } else {
        (sites, "use it")
    };
    let (updated, left): (Uses, Uses) = sites
        .into_iter()
        .partition(|(f, _)| changed.contains(f.as_str()));
    let (mut tests, code): (Uses, Uses) = left.into_iter().partition(|(f, _)| ctx.is_test_file(f));
    let tests_left = tests.len();
    tests.extend(code);
    Reach {
        left: tests,
        tests_left,
        updated: updated.into_iter().map(|(f, _)| f).collect(),
        noun,
        unseen: s.component_id.as_deref().and_then(|c| unseen_users(ctx, c)),
    }
}

/// Why a component may have users the map does not show: it is published,
/// or imports of its package could not be resolved.
fn unseen_users(ctx: &Ctx, component: &str) -> Option<String> {
    let c = ctx.components.get(component)?;
    let name = c.package_name.as_deref()?;
    if ctx.is_published(component) {
        return Some(format!(
            "`{name}` is published, so code outside this repository may depend on it"
        ));
    }
    let unresolved = format!("?{name}");
    let missed = ctx.head.files.iter().any(|f| {
        f.imports.iter().any(|i| {
            i.strip_prefix(&unresolved)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
    });
    missed.then(|| {
        format!("some imports of `{name}` could not be resolved, so Onus may not see every user")
    })
}

impl Reach {
    /// `; not changed yet: 1 test file and 2 other files that use it (…)`
    fn left_sentence(&self) -> String {
        const LISTED: usize = 6;
        let code = self.left.len() - self.tests_left;
        let what = match (self.tests_left, code) {
            (0, c) => plural(c as u32, "file", "files"),
            (t, 0) => plural(t as u32, "test file", "test files"),
            (t, c) => format!(
                "{} and {}",
                plural(t as u32, "test file", "test files"),
                plural(c as u32, "other file", "other files")
            ),
        };
        let mut shown: Vec<String> = self
            .left
            .iter()
            .take(LISTED)
            .map(|(f, _)| format!("`{f}`"))
            .collect();
        if self.left.len() > LISTED {
            shown.push(format!("{} more", self.left.len() - LISTED));
        }
        format!(
            "; not changed yet: {what} that {} ({})",
            self.noun,
            shown.join(", ")
        )
    }

    fn locations(&self) -> Vec<Location> {
        self.left
            .iter()
            .take(6)
            .map(|(f, l)| Location::head(f, *l, *l))
            .collect()
    }
}

/// The line where `file` (in the head tree) implements or builds a value of
/// `name`, if it does: `implements X`, `satisfies X`, `: X = {`, a function
/// declared to return `X`, Effect's `Layer.succeed(X, …)` and `X.of({…})`.
/// A cast (`as X`) does not count: it compiles whatever members `X` gains.
fn implementation_line(ctx: &Ctx, file: &str, name: &str) -> Option<u32> {
    let text = std::fs::read_to_string(onus_core::paths::native(ctx.head_root, file)).ok()?;
    let n = regex::escape(name);
    let pattern = format!(
        r"implements[^{{]*\b{n}\b|satisfies\s+{n}\b|:\s*{n}\s*(?:\[\]\s*)?=\s*[\{{\[]|\)\s*:\s*(?:Promise<\s*)?{n}\s*>?\s*(?:=>|\{{)|Layer\.(?:succeed|effect|scoped|sync)\(\s*{n}\b|\b{n}\.of\("
    );
    let re = regex::Regex::new(&pattern).ok()?;
    let m = re.find(&text)?;
    Some(text[..m.start()].matches('\n').count() as u32 + 1)
}

/// One row for public symbols whose bodies changed but whose types are
/// inferred, so there is nothing Onus can compare.
fn opaque_row(symbols: &[&SymbolNode]) -> SemanticChange {
    let names: Vec<String> = symbols.iter().map(|s| format!("`{}`", s.name)).collect();
    let mut row = change(
        ChangeKind::Internal,
        "contract-inferred-changed",
        ChangeLevel::Structure,
        "inferred-contracts",
        None,
        "Inferred types",
        format!(
            "{} with inferred types changed: {}",
            plural(symbols.len() as u32, "public symbol", "public symbols"),
            listed(&names)
        ),
        "Their types are not written down, so Onus cannot compare them; this only matters if \
         their shapes changed"
            .into(),
        symbols
            .iter()
            .filter_map(|s| signature_location(s, true))
            .collect(),
    );
    row.id = "contract-inferred-changed".into();
    row.hints.confidence = Confidence::Low;
    row
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
    let removed = !ctx.head_syms.contains_key(s.id.as_str());
    let (files, comps) = if removed {
        ctx.base_dependents(&s.id)
    } else {
        ctx.dependents(&s.id)
    };
    let used = if removed && files == 0 {
        format!("Public contract of `{component}` that nothing else in this repository used")
    } else if removed {
        format!(
            "Shared contract that was used in {} across {}",
            plural(files, "file", "files"),
            plural(comps.len() as u32, "component", "components")
        )
    } else if files == 0 {
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
    let reach = if breaking {
        reach(ctx, s, &deltas, removed)
    } else {
        Reach::default()
    };
    let mut locations = locations;
    let (kind, kind_label, why) = if !breaking {
        (
            ChangeKind::Additive,
            "Contract change, additive",
            format!("{used}; existing callers unaffected{invariants}"),
        )
    } else if reach.left.is_empty() && reach.unseen.is_none() {
        // Nothing outside this change can break.
        let after = if !reach.updated.is_empty() {
            let mut files: Vec<String> = reach
                .updated
                .iter()
                .take(4)
                .map(|f| format!("`{f}`"))
                .collect();
            if reach.updated.len() > 4 {
                files.push(format!("{} more", reach.updated.len() - 4));
            }
            format!(
                "every file that {} was updated in this change ({})",
                reach.noun,
                join_and(&files)
            )
        } else if reach.noun == "implement it" {
            "nothing in this repository implements it, and code that only reads it is unaffected"
                .to_string()
        } else {
            "nothing outside this change uses it".to_string()
        };
        (
            ChangeKind::Additive,
            if reach.updated.is_empty() {
                "Contract change, no users affected"
            } else {
                "Contract change, users updated"
            },
            format!("{used}; {after}{invariants}"),
        )
    } else if subkind == "contract-changed-unverified" {
        let unseen = reach
            .unseen
            .as_ref()
            .map(|u| format!("; {u}"))
            .unwrap_or_default();
        (
            ChangeKind::Breaking,
            "Contract change, unverified",
            format!(
                "{used}; Onus cannot prove the change is compatible, so it is treated as breaking{unseen}{invariants}"
            ),
        )
    } else {
        let left = if reach.left.is_empty() {
            String::new()
        } else {
            reach.left_sentence()
        };
        let unseen = reach
            .unseen
            .as_ref()
            .map(|u| format!("; {u}"))
            .unwrap_or_default();
        locations.extend(reach.locations());
        (
            ChangeKind::Breaking,
            "Contract change, breaking",
            format!("{used}; existing callers may break{left}{unseen}{invariants}"),
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
