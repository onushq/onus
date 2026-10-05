//! Step 4 and part of 5: relationship changes between components, events,
//! data and external services, compared at the component level so that
//! moving code inside a component never looks like a new relationship.

use std::collections::{BTreeMap, BTreeSet};

use onus_core::ids;
use onus_core::{
    ChangeKind, ChangeLevel, CodebaseMap, Confidence, Edge, EdgeKind, ExternalService, Location,
    SemanticChange, Visibility,
};

use crate::ctx::{Ctx, change, join_and};

type Key = (String, String);

/// `(from component, to)` → edges, for one edge kind.
fn by_component<'a>(map: &'a CodebaseMap, kind: EdgeKind) -> BTreeMap<Key, Vec<&'a Edge>> {
    let mut out: BTreeMap<Key, Vec<&Edge>> = BTreeMap::new();
    for e in &map.edges {
        if e.kind != kind {
            continue;
        }
        if let Some(c) = ids::component_of(&e.from) {
            out.entry((c.to_string(), e.to.clone()))
                .or_default()
                .push(e);
        }
    }
    out
}

fn head_sites(edges: &[&Edge]) -> Vec<Location> {
    edges
        .iter()
        .flat_map(|e| {
            e.sites
                .iter()
                .map(|s| Location::head(&s.file, s.line, s.line))
        })
        .collect()
}

fn base_sites(edges: &[&Edge]) -> Vec<Location> {
    edges
        .iter()
        .flat_map(|e| {
            e.sites
                .iter()
                .map(|s| Location::base(&s.file, s.line, s.line))
        })
        .collect()
}

fn weakest(edges: &[&Edge]) -> Confidence {
    edges
        .iter()
        .map(|e| e.confidence)
        .max()
        .unwrap_or(Confidence::Static)
}

fn components_with(map: &CodebaseMap, kind: EdgeKind, to: &str) -> BTreeSet<String> {
    map.edges
        .iter()
        .filter(|e| e.kind == kind && e.to == to)
        .filter_map(|e| ids::component_of(&e.from).map(str::to_string))
        .collect()
}

fn code(list: &BTreeSet<String>) -> Vec<String> {
    list.iter().map(|c| format!("`{c}`")).collect()
}

pub fn rows(ctx: &Ctx, violations: &[SemanticChange]) -> Vec<SemanticChange> {
    let mut rows = Vec::new();
    events(ctx, &mut rows);
    data(ctx, &mut rows);
    externals(ctx, &mut rows);
    cross_component(ctx, violations, &mut rows);
    rows
}

fn events(ctx: &Ctx, rows: &mut Vec<SemanticChange>) {
    for (kind, publish) in [(EdgeKind::Consumes, false), (EdgeKind::Publishes, true)] {
        let base = by_component(ctx.base, kind);
        let head = by_component(ctx.head, kind);
        for ((comp, event_id), edges) in &head {
            if base.contains_key(&(comp.clone(), event_id.clone())) {
                continue;
            }
            let event = ids::name_of(event_id);
            let mut confidence = weakest(edges);
            let (title, label, subkind, why) = if publish {
                let mut consumers = components_with(ctx.head, EdgeKind::Consumes, event_id);
                consumers.remove(comp);
                let why = if consumers.is_empty() {
                    format!("No component consumes `{event}` yet")
                } else {
                    format!(
                        "Additive; {} already {} `{event}` and will now receive it from `{comp}`",
                        join_and(&code(&consumers)),
                        if consumers.len() == 1 {
                            "consumes"
                        } else {
                            "consume"
                        }
                    )
                };
                (
                    format!("`{comp}` now publishes the `{event}` event"),
                    "New event publisher",
                    "new-event-publisher",
                    why,
                )
            } else {
                let mut publishers = components_with(ctx.head, EdgeKind::Publishes, event_id);
                publishers.remove(comp);
                let why = if publishers.is_empty() {
                    confidence = Confidence::Low;
                    format!("No publisher of `{event}` was found in the map")
                } else {
                    let changed: BTreeSet<String> = publishers
                        .iter()
                        .filter(|p| ctx.component_changed(p))
                        .cloned()
                        .collect();
                    if changed.is_empty() {
                        format!(
                            "Additive; {} {} unchanged",
                            join_and(&code(&publishers)),
                            if publishers.len() == 1 { "is" } else { "are" }
                        )
                    } else {
                        format!(
                            "Additive; the publisher {} also changed in this pull request",
                            join_and(&code(&changed))
                        )
                    }
                };
                (
                    format!("`{comp}` subscribes to the `{event}` event"),
                    "New event consumer",
                    "new-event-consumer",
                    why,
                )
            };
            let mut row = change(
                ChangeKind::Additive,
                subkind,
                ChangeLevel::Relationship,
                event_id,
                Some(comp),
                label,
                title,
                why,
                head_sites(edges),
            );
            row.id = format!("{subkind}:{event_id}@{comp}");
            row.hints.confidence = confidence;
            row.hints.blast_radius = ctx.dependents(event_id).0;
            rows.push(row);
        }
        for ((comp, event_id), edges) in &base {
            if head.contains_key(&(comp.clone(), event_id.clone())) {
                continue;
            }
            let event = ids::name_of(event_id);
            let (title, label, subkind, why, kind) = if publish {
                let consumers = components_with(ctx.head, EdgeKind::Consumes, event_id);
                if consumers.is_empty() {
                    (
                        format!("`{comp}` no longer publishes the `{event}` event"),
                        "Event publisher removed",
                        "event-publisher-removed",
                        "No component consumes it".to_string(),
                        ChangeKind::Internal,
                    )
                } else {
                    (
                        format!("`{comp}` no longer publishes the `{event}` event"),
                        "Event publisher removed",
                        "event-publisher-removed",
                        format!(
                            "{} still {} `{event}` and may stop receiving it",
                            join_and(&code(&consumers)),
                            if consumers.len() == 1 {
                                "consumes"
                            } else {
                                "consume"
                            }
                        ),
                        ChangeKind::Breaking,
                    )
                }
            } else {
                (
                    format!("`{comp}` no longer subscribes to the `{event}` event"),
                    "Event consumer removed",
                    "event-consumer-removed",
                    format!("`{comp}` stops reacting to `{event}`"),
                    ChangeKind::Breaking,
                )
            };
            let mut row = change(
                kind,
                subkind,
                ChangeLevel::Relationship,
                event_id,
                Some(comp),
                label,
                title,
                why,
                base_sites(edges),
            );
            row.id = format!("{subkind}:{event_id}@{comp}");
            rows.push(row);
        }
    }
}

fn data(ctx: &Ctx, rows: &mut Vec<SemanticChange>) {
    for (kind, write) in [(EdgeKind::Writes, true), (EdgeKind::Reads, false)] {
        let base = by_component(ctx.base, kind);
        let head = by_component(ctx.head, kind);
        for ((comp, table_id), edges) in &head {
            if base.contains_key(&(comp.clone(), table_id.clone())) {
                continue;
            }
            let table = ids::name_of(table_id);
            let labels = ctx.labels(comp);
            let others: BTreeSet<String> = components_with(ctx.head, kind, table_id)
                .into_iter()
                .filter(|c| c != comp)
                .collect();
            let (subkind, label, title, mut why, kind_out) = if write {
                let sensitive = !labels.is_empty();
                (
                    "new-data-write",
                    "New data write",
                    format!("`{comp}` now writes to the `{table}` table"),
                    if sensitive {
                        format!(
                            "New write path in a {} component; needs a person",
                            labels.join(", ")
                        )
                    } else {
                        "New write path to stored data".to_string()
                    },
                    if sensitive {
                        ChangeKind::SecuritySensitive
                    } else {
                        ChangeKind::Additive
                    },
                )
            } else {
                (
                    "new-data-read",
                    "New data read",
                    format!("`{comp}` now reads the `{table}` table"),
                    "New read dependency on stored data".to_string(),
                    ChangeKind::Additive,
                )
            };
            if !others.is_empty() {
                why.push_str(&format!(
                    "; also {} by {}",
                    if write { "written" } else { "read" },
                    join_and(&code(&others))
                ));
            }
            let mut row = change(
                kind_out,
                subkind,
                ChangeLevel::Relationship,
                table_id,
                Some(comp),
                label,
                title,
                why,
                head_sites(edges),
            );
            row.id = format!("{subkind}:{table_id}@{comp}");
            row.hints.blast_radius = ctx.dependents(table_id).0;
            row.hints.needs_person = kind_out == ChangeKind::SecuritySensitive;
            rows.push(row);
        }
        for ((comp, table_id), edges) in &base {
            if head.contains_key(&(comp.clone(), table_id.clone())) {
                continue;
            }
            let table = ids::name_of(table_id);
            let subkind = if write {
                "data-write-removed"
            } else {
                "data-read-removed"
            };
            let mut row = change(
                ChangeKind::Internal,
                subkind,
                ChangeLevel::Relationship,
                table_id,
                Some(comp),
                if write {
                    "Data write removed"
                } else {
                    "Data read removed"
                },
                format!(
                    "`{comp}` no longer {} the `{table}` table",
                    if write { "writes to" } else { "reads" }
                ),
                "Removes an access path to stored data".to_string(),
                base_sites(edges),
            );
            row.id = format!("{subkind}:{table_id}@{comp}");
            rows.push(row);
        }
    }
}

fn category_phrase(category: &str) -> String {
    match category {
        "sms" => "SMS provider".into(),
        "email" => "email provider".into(),
        "payments" => "payment provider".into(),
        "ai" => "AI provider".into(),
        "analytics" => "analytics service".into(),
        "monitoring" => "monitoring service".into(),
        "storage" => "storage service".into(),
        "chat" => "chat service".into(),
        "http" => "HTTP service".into(),
        other => format!("{other} service"),
    }
}

fn egress_phrase(egress: &str) -> String {
    match egress {
        "phone" => "Customer phone numbers".into(),
        "email" => "Customer email addresses".into(),
        "payment" => "Payment details".into(),
        "bank-account" => "Bank account details".into(),
        "pii" => "Personal data".into(),
        "text" => "Text content".into(),
        "files" => "Files".into(),
        "errors" => "Error reports".into(),
        other => format!("{} data", other),
    }
}

fn egress_of(ctx_map: &CodebaseMap, comp: &str) -> BTreeSet<String> {
    let services: BTreeMap<&str, &ExternalService> = ctx_map
        .externals
        .iter()
        .map(|e| (e.id.as_str(), e))
        .collect();
    ctx_map
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::CallsExternal && ids::component_of(&e.from) == Some(comp))
        .filter_map(|e| services.get(e.to.as_str()))
        .flat_map(|s| s.egress.iter().cloned())
        .collect()
}

fn externals(ctx: &Ctx, rows: &mut Vec<SemanticChange>) {
    let base = by_component(ctx.base, EdgeKind::CallsExternal);
    let head = by_component(ctx.head, EdgeKind::CallsExternal);
    let base_ids: BTreeSet<&str> = ctx.base.externals.iter().map(|e| e.id.as_str()).collect();
    for ((comp, ext_id), edges) in &head {
        if base.contains_key(&(comp.clone(), ext_id.clone())) {
            continue;
        }
        let Some(service) = ctx.head.externals.iter().find(|e| &e.id == ext_id) else {
            continue;
        };
        let new_vendor = !base_ids.contains(ext_id.as_str());
        let base_egress = egress_of(ctx.base, comp);
        let new_egress: Vec<&String> = service
            .egress
            .iter()
            .filter(|e| !base_egress.contains(*e))
            .collect();
        let mut novelty = Vec::new();
        if new_vendor {
            novelty.push(format!("new-vendor:{}", service.vendor));
        }
        for e in &new_egress {
            novelty.push(format!("new-data-egress:{e}"));
        }
        let sensitive = new_vendor || !new_egress.is_empty();
        let mut locations = head_sites(edges);
        for e in &ctx.head.edges {
            if e.kind == EdgeKind::Imports
                && ids::component_of(&e.from) == Some(comp.as_str())
                && service.packages.iter().any(|p| e.to == ids::npm_id(p))
            {
                locations.extend(
                    e.sites
                        .iter()
                        .map(|s| Location::head(&s.file, s.line, s.line)),
                );
            }
        }
        let vendor_note = if new_vendor {
            format!("{}, a new vendor", service.vendor)
        } else {
            let users: BTreeSet<String> =
                components_with(ctx.base, EdgeKind::CallsExternal, ext_id);
            format!(
                "{} (already used by {})",
                service.vendor,
                join_and(&code(&users))
            )
        };
        let why = if !new_egress.is_empty() {
            let what: Vec<String> = new_egress.iter().map(|e| egress_phrase(e)).collect();
            format!(
                "{} leave the system via {vendor_note}; needs a person",
                join_and(&what)
            )
        } else if new_vendor {
            format!("Data leaves the system via {vendor_note}; needs a person")
        } else {
            format!("Another component now calls {vendor_note}")
        };
        let mut row = change(
            if sensitive {
                ChangeKind::SecuritySensitive
            } else {
                ChangeKind::Additive
            },
            if sensitive {
                "new-external-service"
            } else {
                "new-external-call"
            },
            ChangeLevel::Relationship,
            ext_id,
            Some(comp),
            "New external dependency",
            format!(
                "`{comp}` now calls an external {} ({})",
                category_phrase(&service.category),
                service.vendor
            ),
            why,
            locations,
        );
        row.id = format!("{}:{ext_id}@{comp}", row.subkind);
        row.hints.novelty = novelty;
        row.hints.needs_person = sensitive;
        row.hints.confidence = match weakest(edges) {
            Confidence::Static => service.confidence,
            other => other,
        };
        row.hints.blast_radius = ctx.dependents(ext_id).0;
        rows.push(row);
    }
    for ((comp, ext_id), edges) in &base {
        if head.contains_key(&(comp.clone(), ext_id.clone())) {
            continue;
        }
        let vendor = ctx
            .base
            .externals
            .iter()
            .find(|e| &e.id == ext_id)
            .map_or(ids::name_of(ext_id).to_string(), |e| e.vendor.clone());
        let mut row = change(
            ChangeKind::Internal,
            "external-service-removed",
            ChangeLevel::Relationship,
            ext_id,
            Some(comp),
            "External dependency removed",
            format!("`{comp}` no longer calls {vendor}"),
            "One less place where data leaves the system".into(),
            base_sites(edges),
        );
        row.id = format!("external-service-removed:{ext_id}@{comp}");
        rows.push(row);
    }
}

type ComponentPairs<'a> = BTreeMap<(String, String), Vec<&'a Edge>>;

fn component_pairs(map: &CodebaseMap) -> ComponentPairs<'_> {
    let mut out: ComponentPairs = BTreeMap::new();
    for e in &map.edges {
        if !e.kind.is_code_dependency() {
            continue;
        }
        if let (Some(f), Some(t)) = (ids::component_of(&e.from), ids::component_of(&e.to)) {
            if f != t {
                out.entry((f.to_string(), t.to_string()))
                    .or_default()
                    .push(e);
            }
        }
    }
    out
}

fn cross_component(ctx: &Ctx, violations: &[SemanticChange], rows: &mut Vec<SemanticChange>) {
    let base = component_pairs(ctx.base);
    let head = component_pairs(ctx.head);
    let violated: BTreeSet<(String, String)> = violations
        .iter()
        .filter_map(|v| {
            let rule = ctx.head.rules.iter().find(|r| r.id == v.subject)?;
            let to = rule
                .deny
                .to
                .strip_suffix(".internal")
                .unwrap_or(&rule.deny.to);
            Some((rule.deny.from.clone(), to.to_string()))
        })
        .collect();
    for ((from, to), edges) in &head {
        if base.contains_key(&(from.clone(), to.clone()))
            || violated.contains(&(from.clone(), to.clone()))
        {
            continue;
        }
        let targets: BTreeSet<String> = edges
            .iter()
            .map(|e| format!("`{}`", ids::name_of(&e.to)))
            .collect();
        let internal: BTreeSet<String> = edges
            .iter()
            .filter(|e| {
                ctx.head_syms
                    .get(e.to.as_str())
                    .is_some_and(|s| s.visibility == Visibility::Internal)
            })
            .map(|e| format!("`{}`", ids::name_of(&e.to)))
            .collect();
        let targets: Vec<String> = targets.into_iter().take(4).collect();
        let mut why = format!("`{from}` now uses {} from `{to}`", join_and(&targets));
        if !internal.is_empty() {
            let internal: Vec<String> = internal.into_iter().collect();
            why.push_str(&format!(
                "; {} internal to `{to}`",
                if internal.len() == 1 {
                    format!("{} is", internal[0])
                } else {
                    format!("{} are", join_and(&internal))
                }
            ));
        }
        let mut row = change(
            ChangeKind::Additive,
            "new-cross-component-dependency",
            ChangeLevel::Relationship,
            to,
            Some(from),
            "New dependency between components",
            format!("`{from}` now depends on `{to}`"),
            why,
            head_sites(edges),
        );
        row.id = format!("new-cross-component-dependency:{from}->{to}");
        row.hints.blast_radius = ctx.dependents(to).0;
        rows.push(row);
    }
}
