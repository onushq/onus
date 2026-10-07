//! Declarative framework packs: tree-sitter queries and what a match means.
//!
//! A pack is YAML:
//!
//! ```yaml
//! pack: nestjs-events
//! rules:
//!   - id: on-event
//!     query: |
//!       ((decorator (call_expression
//!          function: (identifier) @fn
//!          arguments: (arguments . (_) @name)))
//!        (#eq? @fn "OnEvent"))
//!     emit: { event: consumes, name: "@name" }
//! ```
//!
//! A rule emits exactly one of:
//!
//! - `event: publishes|consumes` with `name: "@capture"` (resolved like the
//!   built-in event patterns: literals, constants, enum members);
//! - `data: reads|writes` with `table: "<template>"`;
//! - `config: "<template>"` (a configuration key read);
//! - `host: "@capture"` (an outbound call to the literal URL's host);
//! - `route: "<template>"` (an HTTP route served here, a public contract);
//! - `edge: <kind>` with `to: "<template>"` (any other relationship);
//! - `diagnostic: <kind>` with `message: "<text>"`.
//!
//! Templates replace `{capture}` with the captured text (string literals
//! without their quotes) and accept `|upper` and `|lower`. Packs never run
//! code, so the analyzed repository's `onus.yaml` may list them.

use onus_core::EdgeKind;
use serde::Deserialize;
use tree_sitter::{Language, Query};

use crate::extract::CallPattern;

/// The packs Onus ships with.
pub const BUILT_IN: &[(&str, &str)] = &[
    ("prisma", include_str!("../packs/prisma.yaml")),
    ("process-env", include_str!("../packs/process-env.yaml")),
    ("http-clients", include_str!("../packs/http-clients.yaml")),
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackFile {
    pack: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    rules: Vec<RuleFile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    id: String,
    query: String,
    emit: EmitFile,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmitFile {
    event: Option<String>,
    name: Option<String>,
    data: Option<String>,
    table: Option<String>,
    config: Option<String>,
    host: Option<String>,
    route: Option<String>,
    edge: Option<String>,
    to: Option<String>,
    diagnostic: Option<String>,
    message: Option<String>,
}

/// What a match means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Emit {
    Event { publish: bool, capture: String },
    Data { write: bool, table: String },
    Config { key: String },
    Host { capture: String },
    Route { name: String },
    Edge { kind: EdgeKind, to: String },
    Diagnostic { kind: String, message: String },
}

/// A rule and what its matches mean.
#[derive(Debug, Clone)]
pub struct Rule {
    pub pack: String,
    pub id: String,
    pub emit: Emit,
}

/// Every rule compiled into one query for a grammar, so a file is walked
/// once however many rules there are.
#[derive(Debug)]
pub struct RuleSet {
    pub query: Query,
    pub rules: Vec<Rule>,
    /// Pattern index in `query` → index in `rules`.
    pub pattern_rule: Vec<usize>,
}

/// Rules compiled for the TypeScript and TSX grammars.
#[derive(Debug, Default)]
pub struct Compiled {
    pub typescript: Option<RuleSet>,
    pub tsx: Option<RuleSet>,
}

fn capture_name(s: &str, what: &str) -> Result<String, String> {
    s.strip_prefix('@')
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{what} must name a capture, like \"@name\""))
}

fn edge_kind(s: &str) -> Result<EdgeKind, String> {
    Ok(match s {
        "imports" => EdgeKind::Imports,
        "calls" => EdgeKind::Calls,
        "references-type" => EdgeKind::ReferencesType,
        "reads" => EdgeKind::Reads,
        "writes" => EdgeKind::Writes,
        "publishes" => EdgeKind::Publishes,
        "consumes" => EdgeKind::Consumes,
        "calls-external" => EdgeKind::CallsExternal,
        "reads-config" => EdgeKind::ReadsConfig,
        other => return Err(format!("unknown edge kind `{other}`")),
    })
}

fn emit(e: &EmitFile) -> Result<Emit, String> {
    let forms = [
        e.event.is_some(),
        e.data.is_some(),
        e.config.is_some(),
        e.host.is_some(),
        e.route.is_some(),
        e.edge.is_some(),
        e.diagnostic.is_some(),
    ]
    .iter()
    .filter(|f| **f)
    .count();
    if forms != 1 {
        return Err(
            "emit exactly one of event, data, config, host, route, edge, diagnostic".into(),
        );
    }
    if let Some(ev) = &e.event {
        let publish = match ev.as_str() {
            "publishes" => true,
            "consumes" => false,
            other => {
                return Err(format!(
                    "event must be publishes or consumes, not `{other}`"
                ));
            }
        };
        let name = e.name.as_deref().ok_or("event needs a `name` capture")?;
        return Ok(Emit::Event {
            publish,
            capture: capture_name(name, "name")?,
        });
    }
    if let Some(d) = &e.data {
        let write = match d.as_str() {
            "writes" => true,
            "reads" => false,
            other => return Err(format!("data must be reads or writes, not `{other}`")),
        };
        return Ok(Emit::Data {
            write,
            table: e.table.clone().ok_or("data needs a `table`")?,
        });
    }
    if let Some(k) = &e.config {
        return Ok(Emit::Config { key: k.clone() });
    }
    if let Some(h) = &e.host {
        return Ok(Emit::Host {
            capture: capture_name(h, "host")?,
        });
    }
    if let Some(r) = &e.route {
        return Ok(Emit::Route { name: r.clone() });
    }
    if let Some(k) = &e.edge {
        let to = e.to.clone().ok_or("edge needs `to`")?;
        return Ok(Emit::Edge {
            kind: edge_kind(k)?,
            to,
        });
    }
    let kind = e.diagnostic.clone().unwrap_or_default();
    Ok(Emit::Diagnostic {
        message: e.message.clone().unwrap_or_else(|| kind.clone()),
        kind,
    })
}

/// Escapes text for a regular expression inside a query string.
fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Writes a regular expression as a query string literal.
fn query_string(regex: &str) -> String {
    format!("\"{}\"", regex.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A callee such as `bus.publish` as a regex that also accepts a receiver
/// before it (`this.bus.publish`, `app.bus.publish`) and whitespace around
/// the dots.
fn callee_regex(callee: &str) -> String {
    let parts: Vec<String> = callee.split('.').map(|p| regex_escape(p.trim())).collect();
    format!("^(?:[\\w$]+\\s*\\.\\s*)*{}$", parts.join("\\s*\\.\\s*"))
}

/// The pack generated from event call patterns.
fn event_rules(publish: &[CallPattern], subscribe: &[CallPattern]) -> Vec<(String, String, Emit)> {
    let mut out = Vec::new();
    for (publish, patterns) in [(true, publish), (false, subscribe)] {
        for p in patterns {
            let skips: String = (0..p.event_arg).map(|_| "(_) . ").collect();
            let callee = query_string(&callee_regex(&p.callee));
            let query = if p.decorator {
                format!(
                    "((decorator (call_expression function: (_) @callee arguments: (arguments . {skips}(_) @name))) (#match? @callee {callee}))"
                )
            } else {
                format!(
                    "((call_expression function: (_) @callee arguments: (arguments . {skips}(_) @name)) (#match? @callee {callee}))"
                )
            };
            out.push((
                format!("{}{}", if p.decorator { "@" } else { "" }, p.callee),
                query,
                Emit::Event {
                    publish,
                    capture: "name".into(),
                },
            ));
        }
    }
    out
}

/// Compiles the built-in packs, the event patterns and `extra` pack texts
/// for both grammars.
pub fn compile(
    publish: &[CallPattern],
    subscribe: &[CallPattern],
    prisma_clients: &[String],
    extra: &[String],
) -> Result<Compiled, String> {
    let clients: Vec<String> = prisma_clients.iter().map(|c| regex_escape(c)).collect();
    let clients = format!(
        "(?:^|\\.\\s*)(?:{})$",
        if clients.is_empty() {
            "prisma".to_string()
        } else {
            clients.join("|")
        }
    );
    let mut rules: Vec<(String, String, String, Emit)> = Vec::new();
    for (callee, query, e) in event_rules(publish, subscribe) {
        rules.push(("events".into(), callee, query, e));
    }
    let texts = BUILT_IN
        .iter()
        .map(|(_, t)| t.to_string())
        .chain(extra.iter().cloned());
    for text in texts {
        let file: PackFile =
            serde_yaml_ng::from_str(&text).map_err(|e| format!("invalid pack: {e}"))?;
        for r in file.rules {
            let e =
                emit(&r.emit).map_err(|e| format!("pack `{}`, rule `{}`: {e}", file.pack, r.id))?;
            // The client pattern goes inside a query string: escape it.
            let escaped = clients.replace('\\', "\\\\");
            let query = r.query.replace("{{prisma_clients}}", &escaped);
            rules.push((file.pack.clone(), r.id, query, e));
        }
    }
    let mut compiled = Compiled::default();
    for (lang, out) in [
        (
            Language::from(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
            &mut compiled.typescript,
        ),
        (
            Language::from(tree_sitter_typescript::LANGUAGE_TSX),
            &mut compiled.tsx,
        ),
    ] {
        // Each rule on its own first, for precise errors.
        let mut source = String::new();
        let mut starts = Vec::new();
        for (pack, id, query, _) in &rules {
            Query::new(&lang, query)
                .map_err(|err| format!("pack `{pack}`, rule `{id}`: invalid query: {err}"))?;
            starts.push(source.len());
            source.push_str(query);
            source.push('\n');
        }
        if rules.is_empty() {
            continue;
        }
        let query = Query::new(&lang, &source).map_err(|err| format!("packs: {err}"))?;
        let pattern_rule = (0..query.pattern_count())
            .map(|i| {
                let at = query.start_byte_for_pattern(i);
                starts.iter().rposition(|s| *s <= at).unwrap_or(0)
            })
            .collect();
        *out = Some(RuleSet {
            query,
            rules: rules
                .iter()
                .map(|(pack, id, _, e)| Rule {
                    pack: pack.clone(),
                    id: id.clone(),
                    emit: e.clone(),
                })
                .collect(),
            pattern_rule,
        });
    }
    Ok(compiled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_packs_compile() {
        let c = compile(
            &[CallPattern::parse("bus.publish($EVENT, ...)").unwrap()],
            &[CallPattern::parse("@OnEvent($EVENT)").unwrap()],
            &["prisma".into(), "db".into()],
            &[],
        )
        .unwrap();
        let ts = c.typescript.as_ref().unwrap();
        assert_eq!(ts.rules.len(), c.tsx.as_ref().unwrap().rules.len());
        assert!(
            ts.rules
                .iter()
                .any(|r| r.pack == "prisma" && r.id == "write")
        );
        assert!(
            ts.rules
                .iter()
                .any(|r| r.pack == "events" && r.id == "@OnEvent")
        );
        // Every pattern maps to a rule.
        assert_eq!(ts.pattern_rule.len(), ts.query.pattern_count());
    }

    #[test]
    fn bad_packs_are_reported_with_their_rule() {
        let bad_query = "pack: p\nrules:\n  - id: r\n    query: \"(call_expression\"\n    emit: { config: \"{x}\" }\n";
        let err = compile(&[], &[], &[], &[bad_query.into()]).unwrap_err();
        assert!(err.contains("pack `p`, rule `r`: invalid query"), "{err}");
        let two_emits = "pack: p\nrules:\n  - id: r\n    query: \"(identifier) @x\"\n    emit: { config: \"{x}\", host: \"@x\" }\n";
        let err = compile(&[], &[], &[], &[two_emits.into()]).unwrap_err();
        assert!(err.contains("emit exactly one"), "{err}");
    }

    #[test]
    fn callee_patterns_accept_receivers_and_whitespace() {
        let pattern = callee_regex("bus.publish");
        let re = regex_lite(&pattern);
        assert!(re("bus.publish"));
        assert!(re("this.bus.publish"));
        assert!(re("app.bus\n  .publish"));
        assert!(!re("bus.publishAll"));
        assert!(!re("omnibus.publish"));
    }

    /// The tree-sitter query engine uses the `regex` crate's syntax; check
    /// the generated pattern by running a query against small sources.
    fn regex_lite(pattern: &str) -> impl Fn(&str) -> bool + use<> {
        let query_src = format!(
            "((call_expression function: (_) @callee) (#match? @callee {}))",
            query_string(pattern)
        );
        move |callee: &str| {
            let src = format!("{callee}(1);");
            let lang = Language::from(tree_sitter_typescript::LANGUAGE_TYPESCRIPT);
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&lang).unwrap();
            let tree = parser.parse(&src, None).unwrap();
            let query = Query::new(&lang, &query_src).unwrap();
            let mut cursor = tree_sitter::QueryCursor::new();
            use tree_sitter::StreamingIterator;
            let mut matches = cursor.matches(&query, tree.root_node(), src.as_bytes());
            matches.next().is_some()
        }
    }
}
