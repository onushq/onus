//! Risk classes: changes a reviewer must see whatever their size or shape.
//! Each gets its own row that needs a person, instead of disappearing into
//! a component's "changed lines" row:
//!
//! - database migrations (new, edited or removed), with what they do to
//!   stored data;
//! - public API operations: GraphQL queries, mutations and subscriptions
//!   (schema files and code-first resolvers), especially ones that skip
//!   authentication;
//! - HTTP routes defined by file-based routing or controller decorators;
//! - authentication and authorization code, found by its file names.
//!
//! Everything is read from the text of the changed files with fixed
//! patterns; nothing is run.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use onus_core::{ChangeKind, ChangeLevel, Location, SemanticChange};
use regex::Regex;

use crate::ctx::{Ctx, change, join_and, join_some, plural};
use crate::text::{FileChange, Status};

pub fn rows(ctx: &Ctx) -> Vec<SemanticChange> {
    let mut rows = Vec::new();
    for f in &ctx.text.files {
        if f.status == Status::Renamed || ctx.is_test_data(&f.path) || ctx.is_test_file(&f.path) {
            continue;
        }
        if is_migration(&f.path) {
            rows.extend(migration_row(ctx, f));
            continue;
        }
        if is_migration_metadata(&f.path) {
            // Snapshots and journals a migration tool writes next to the
            // migration it describes.
            ctx.explain(&f.path);
            continue;
        }
        rows.extend(graphql_rows(ctx, f));
        rows.extend(route_rows(ctx, f));
    }
    rows.extend(auth_rows(ctx, &rows));
    rows
}

fn read(root: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(onus_core::paths::native(root, path)).unwrap_or_default()
}

fn base_text(ctx: &Ctx, f: &FileChange) -> String {
    match f.status {
        Status::Added => String::new(),
        _ => read(ctx.base_root, f.base_path()),
    }
}

fn head_text(ctx: &Ctx, f: &FileChange) -> String {
    match f.status {
        Status::Deleted => String::new(),
        _ => read(ctx.head_root, &f.path),
    }
}

/// The changed lines of a file as locations (head lines, else base lines).
fn changed_locations(f: &FileChange) -> Vec<Location> {
    let mut out = Vec::new();
    for h in &f.hunks {
        if let (Some(a), Some(z)) = (h.head_lines.first(), h.head_lines.last()) {
            out.push(Location::head(&f.path, *a, *z));
        }
        if let (Some(a), Some(z)) = (h.base_lines.first(), h.base_lines.last()) {
            out.push(Location::base(f.base_path(), *a, *z));
        }
    }
    if out.is_empty() {
        out.push(Location::head(&f.path, 1, 1));
    }
    out
}

fn component(ctx: &Ctx, path: &str) -> Option<String> {
    let c = ctx.component_of_path(path);
    (c != "root").then_some(c)
}

fn code(items: impl IntoIterator<Item = String>) -> Vec<String> {
    items.into_iter().map(|i| format!("`{i}`")).collect()
}

// ---------------------------------------------------------------------------
// Migrations

/// Whether a path is in a database migrations folder (Prisma, TypeORM,
/// Knex, Supabase, Rails `db/migrate`, Alembic), is a `*.migration.*`
/// file, a Flyway `V1__name.sql`, or a versioned upgrade command
/// (`upgrade…/…-command-<timestamp>-….ts`).
pub fn is_migration(path: &str) -> bool {
    migration_kind(path).is_some()
}

/// Files a migration tool writes beside its migrations: Drizzle's
/// `drizzle/meta/*.json`, Prisma's `migration_lock.toml`.
fn is_migration_metadata(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    (lower.contains("drizzle/meta/") && lower.ends_with(".json"))
        || lower.ends_with("migrations/migration_lock.toml")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrationKind {
    /// One step of the migration history: runs once per database.
    Versioned,
    /// Other code in a migrations folder (a registry, helpers, grants).
    Support,
}

fn migration_kind(path: &str) -> Option<MigrationKind> {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let code_or_sql = [".sql", ".ts", ".js", ".mjs", ".cjs", ".py", ".rb"]
        .iter()
        .any(|e| name.ends_with(e));
    if !code_or_sql || name.ends_with(".d.ts") {
        return None;
    }
    static FLYWAY: OnceLock<Regex> = OnceLock::new();
    let flyway = FLYWAY.get_or_init(|| Regex::new(r"^v\d+(?:[._]\d+)*__.+\.sql$").unwrap());
    static VERSIONED: OnceLock<Regex> = OnceLock::new();
    let versioned = VERSIONED.get_or_init(|| Regex::new(r"^\d{3,}|\d{10,}").unwrap());
    let segments: Vec<&str> = lower.split('/').collect();
    let dirs = &segments[..segments.len() - 1];
    let parent = dirs.last().copied().unwrap_or("");
    if name.contains(".migration.") || flyway.is_match(name) {
        return Some(MigrationKind::Versioned);
    }
    static UPGRADE: OnceLock<Regex> = OnceLock::new();
    let upgrade = UPGRADE.get_or_init(|| Regex::new(r"command.*\d{10,}|\d{10,}.*command").unwrap());
    if lower.contains("upgrade") && upgrade.is_match(name) {
        return Some(MigrationKind::Versioned);
    }
    let in_folder = dirs
        .iter()
        .any(|d| matches!(*d, "migrations" | "migration" | "migrate"))
        || lower.contains("alembic/versions/")
        // Drizzle Kit writes `drizzle/0001_name.sql`.
        || (parent == "drizzle" && name.ends_with(".sql"));
    if !in_folder {
        return None;
    }
    // `20240101_init.sql`, `1790000013000-create.ts`, Prisma's
    // `20240101_init/migration.sql`, any Alembic version.
    if versioned.is_match(name)
        || (name == "migration.sql" && versioned.is_match(parent))
        || lower.contains("alembic/versions/")
    {
        Some(MigrationKind::Versioned)
    } else {
        Some(MigrationKind::Support)
    }
}

/// What a migration's text does to stored data, in order, without
/// duplicates. Text after a `down` step is left out: it undoes the rest.
fn migration_effects(text: &str) -> (Vec<String>, bool) {
    static DOWN: OnceLock<Regex> = OnceLock::new();
    let down = DOWN.get_or_init(|| {
        Regex::new(r"(?m)^\s*(?:public\s+)?(?:async\s+)?(?:down\s*[(:=]|export\s+(?:async\s+)?function\s+down\b|export\s+const\s+down\b|exports\.down\b|def\s+downgrade\b|def\s+down\b)")
            .unwrap()
    });
    let text = match down.find(text) {
        Some(m) => &text[..m.start()],
        None => text,
    };
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        let id = r#"[`"']?([\w.]+?)[`"']?"#;
        [
            (format!(r"(?i)\bcreate\s+table\s+(?:if\s+not\s+exists\s+)?{id}[\s(]"), "creates table"),
            (format!(r"(?i)\bdrop\s+table\s+(?:if\s+exists\s+)?{id}(?:\s|;|$|`)"), "drops table"),
            (format!(r"(?i)\balter\s+table\s+(?:only\s+)?{id}\s+add\s+(?:column\s+)?(?:if\s+not\s+exists\s+)?{id}\s"), "adds column"),
            (format!(r"(?i)\balter\s+table\s+(?:only\s+)?{id}\s+drop\s+(?:column\s+)?(?:if\s+exists\s+)?{id}(?:\s|;|$|`)"), "drops column"),
            (format!(r"(?i)\balter\s+table\s+(?:only\s+)?{id}\s+rename\b"), "renames in table"),
            (format!(r"(?i)\balter\s+table\s+(?:only\s+)?{id}\s+alter\s+(?:column\s+)?{id}\s"), "changes column"),
            (format!(r"(?i)\bdelete\s+from\s+{id}(?:\s|;|$|`)"), "deletes rows from"),
            (format!(r"(?i)\btruncate\s+(?:table\s+)?{id}(?:\s|;|$|`)"), "empties table"),
            (format!(r"(?i)\bupdate\s+{id}\s+set\b"), "updates rows in"),
            (format!(r"(?i)\bcreate\s+(?:unique\s+)?index\s+(?:concurrently\s+)?(?:if\s+not\s+exists\s+)?{id}\s+on\s+(?:only\s+)?{id}[\s(]"), "creates index"),
            (format!(r"(?i)\bgrant\s+[\w\s,]+?\s+on\s+(?:table\s+)?{id}(?:\s|;|$|`)"), "grants access to"),
            (format!(r"(?i)\brevoke\s+[\w\s,]+?\s+on\s+(?:table\s+)?{id}(?:\s|;|$|`)"), "revokes access to"),
            (r#"createTable\(\s*(?:new\s+Table\(\s*\{\s*name:\s*)?['"`]([\w.]+)['"`]"#.to_string(), "creates table"),
            (r#"dropTable\(\s*['"`]([\w.]+)['"`]"#.to_string(), "drops table"),
            (r#"addColumns?\(\s*['"`]([\w.]+)['"`]\s*,\s*(?:new\s+TableColumn\(\s*\{\s*name:\s*)?['"`]?([\w]+)"#.to_string(), "adds column"),
            (r#"dropColumns?\(\s*['"`]([\w.]+)['"`]\s*,\s*['"`]([\w]+)['"`]"#.to_string(), "drops column"),
        ]
        .into_iter()
        .map(|(p, what)| (Regex::new(&p).unwrap(), what))
        .collect()
    });
    let mut found: Vec<(usize, String)> = Vec::new();
    for (re, what) in patterns {
        for c in re.captures_iter(text) {
            let target = match (c.get(1), c.get(2)) {
                (Some(i), Some(table)) if *what == "creates index" => {
                    found.push((
                        c.get(0).map_or(0, |m| m.start()),
                        format!("creates index `{}` on `{}`", i.as_str(), table.as_str()),
                    ));
                    continue;
                }
                (Some(t), Some(col)) => format!("{}.{}", t.as_str(), col.as_str()),
                (Some(t), None) => t.as_str().to_string(),
                _ => continue,
            };
            let start = c.get(0).map_or(0, |m| m.start());
            found.push((start, format!("{what} `{target}`")));
        }
    }
    // `ALTER TABLE t ADD COLUMN a …, ADD COLUMN b …`: every column, not
    // only the first.
    static ALTER: OnceLock<Regex> = OnceLock::new();
    let alter = ALTER.get_or_init(|| {
        Regex::new(
            r#"(?is)\balter\s+table\s+(?:only\s+)?[`"']?([\w.]+?)[`"']?\s+(add\s.*?)(?:;|`|$)"#,
        )
        .unwrap()
    });
    static ADD: OnceLock<Regex> = OnceLock::new();
    let add = ADD.get_or_init(|| {
        Regex::new(r#"(?i)(?:^|,)\s*add\s+(?:column\s+)?(?:if\s+not\s+exists\s+)?[`"']?(\w+)"#)
            .unwrap()
    });
    for c in alter.captures_iter(text) {
        let start = c.get(2).map_or(0, |m| m.start());
        for a in add.captures_iter(&c[2]) {
            let start = start + a.get(0).map_or(0, |m| m.start());
            let col = &a[1];
            if !matches!(
                col.to_ascii_lowercase().as_str(),
                "constraint" | "primary" | "foreign" | "unique" | "check" | "index"
            ) {
                found.push((start, format!("adds column `{}.{col}`", &c[1])));
            }
        }
    }
    found.sort();
    let mut seen = BTreeSet::new();
    let effects = found
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| seen.insert(e.clone()))
        .collect();
    let cascade = text.to_ascii_lowercase().contains("on delete cascade");
    (effects, cascade)
}

/// `creates tables `a` and `b``: effects with the same verb read as one.
fn summarize_effects(effects: &[String]) -> Vec<String> {
    let mut by_verb: Vec<(String, Vec<String>)> = Vec::new();
    for e in effects {
        let Some(tick) = e.find('`') else {
            continue;
        };
        let (verb, target) = (e[..tick].trim().to_string(), e[tick..].to_string());
        match by_verb.iter_mut().find(|(v, _)| *v == verb) {
            Some((_, targets)) => targets.push(target),
            None => by_verb.push((verb, vec![target])),
        }
    }
    by_verb
        .into_iter()
        .map(|(verb, targets)| {
            if targets.len() == 1 {
                format!("{verb} {}", targets[0])
            } else {
                let plural_verb = match verb.rsplit_once(' ') {
                    Some((v, "table")) => format!("{v} tables"),
                    Some((v, "column")) => format!("{v} columns"),
                    _ => verb.clone(),
                };
                format!("{plural_verb} {}", join_some(&targets, 4))
            }
        })
        .collect()
}

fn migration_row(ctx: &Ctx, f: &FileChange) -> Option<SemanticChange> {
    let kind = migration_kind(&f.path)?;
    let text = match f.status {
        Status::Deleted => base_text(ctx, f),
        _ => head_text(ctx, f),
    };
    let (effects, cascade) = migration_effects(&text);
    let summary = summarize_effects(&effects);
    let path = &f.path;
    // Code next to the migrations (a registry, helpers) is ordinary code
    // unless it touches stored data itself.
    if kind == MigrationKind::Support && summary.is_empty() {
        return None;
    }
    let title = match (kind, f.status) {
        (MigrationKind::Support, _) => format!("Migration code `{path}` changed"),
        (_, Status::Added) => match summary.first() {
            Some(first) => format!("Migration `{path}` {first}"),
            None => format!("New migration `{path}`"),
        },
        (_, Status::Deleted) => format!("Migration `{path}` removed"),
        _ => format!("Existing migration `{path}` edited"),
    };
    let mut why: Vec<String> = Vec::new();
    if !summary.is_empty() {
        why.push(capitalize(&join_and(&summary)));
    }
    if cascade {
        why.push(
            "`ON DELETE CASCADE` deletes dependent rows together with the rows they refer to"
                .into(),
        );
    }
    why.push(match (kind, f.status) {
        (MigrationKind::Support, _) => "it changes how stored data is migrated".into(),
        (_, Status::Added) => {
            "it changes stored data: deploy it before code that relies on the new \
                          shape, and check it can be rolled back"
                .into()
        }
        (_, Status::Deleted) => "databases that already ran it keep its changes; databases that \
                                 did not will never get them"
            .into(),
        _ => "databases that already ran this migration will not run the edit, so their data \
              and new databases can drift apart"
            .into(),
    });
    let why = capitalize(&why.join("; "));
    let mut row = change(
        ChangeKind::Config,
        "migration-changed",
        ChangeLevel::Behavior,
        path,
        component(ctx, path).as_deref(),
        "Migration",
        title,
        why,
        changed_locations(f),
    );
    row.id = format!("migration-changed:{path}");
    row.hints.needs_person = true;
    ctx.explain(path);
    Some(row)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// GraphQL operations

/// A root operation: `Query.posts`, with its signature and directives.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Operation {
    root: String,
    name: String,
    signature: String,
    /// Directives or decorators that skip authentication.
    skips_auth: Option<String>,
    line: u32,
}

/// A signature with its arguments sorted by name: GraphQL arguments are
/// named, so their order is no change.
fn canonical_signature(signature: &str) -> String {
    let Some(open) = signature.find('(') else {
        return signature.to_string();
    };
    let mut depth = 0;
    let mut close = None;
    for (i, c) in signature[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return signature.to_string();
    };
    static ARG: OnceLock<Regex> = OnceLock::new();
    let arg = ARG.get_or_init(|| {
        Regex::new(r#"([A-Za-z_]\w*)\s*:\s*([\w\[\]!]+)(\s*=\s*(?:"[^"]*"|[^,\s)]+))?"#).unwrap()
    });
    let mut args: Vec<String> = arg
        .captures_iter(&signature[open + 1..close])
        .map(|c| {
            let default: String = c
                .get(3)
                .map_or(String::new(), |d| d.as_str().split_whitespace().collect());
            format!("{}:{}{default}", &c[1], &c[2])
        })
        .collect();
    args.sort();
    format!(
        "{}({}){}",
        signature[..open].trim(),
        args.join(","),
        signature[close + 1..]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    )
}

/// Directives and decorators that make an operation public.
fn auth_skipping(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"@(?i:(skip_?auth\w*|public|no_?auth\w*|allow_?anonymous|unauthenticated|anonymous|is_?public|allow_?unauthenticated))\b(?:\(\s*\))?").unwrap()
    });
    re.find(text).map(|m| m.as_str().to_string())
}

fn line_of(text: &str, offset: usize) -> u32 {
    text[..offset].matches('\n').count() as u32 + 1
}

/// Operations of `type Query { … }`, `extend type Mutation { … }` and
/// `type Subscription { … }` blocks in schema text.
fn sdl_operations(text: &str) -> Vec<Operation> {
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    let block = BLOCK.get_or_init(|| {
        Regex::new(r"(?m)^\s*(?:extend\s+)?type\s+(Query|Mutation|Subscription)\b[^{]*\{").unwrap()
    });
    static FIELD: OnceLock<Regex> = OnceLock::new();
    let field = FIELD.get_or_init(|| Regex::new(r"^\s*([A-Za-z_]\w*)\s*[(:]").unwrap());
    let mut out = Vec::new();
    for m in block.captures_iter(text) {
        let root = m[1].to_string();
        let start = m.get(0).map_or(0, |g| g.end());
        // The block ends at the matching brace.
        let mut depth = 1;
        let mut end = start;
        for (i, c) in text[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        // Fields can span lines (arguments); split at top-level line starts.
        let body = &text[start..end];
        let mut paren = 0i32;
        let mut current = String::new();
        let mut current_at = start;
        let mut offset = start;
        let mut fields: Vec<(usize, String)> = Vec::new();
        for line in body.split_inclusive('\n') {
            if paren == 0 && !current.trim().is_empty() {
                fields.push((current_at, std::mem::take(&mut current)));
            }
            if paren == 0 {
                current_at = offset;
            }
            for c in line.chars() {
                match c {
                    '(' => paren += 1,
                    ')' => paren -= 1,
                    _ => {}
                }
            }
            if !line.trim_start().starts_with('#') && !line.trim_start().starts_with('"') {
                current.push_str(line);
            }
            offset += line.len();
        }
        if !current.trim().is_empty() {
            fields.push((current_at, current));
        }
        for (at, f) in fields {
            let Some(c) = field.captures(&f) else {
                continue;
            };
            let signature: String = f.split_whitespace().collect::<Vec<_>>().join(" ");
            out.push(Operation {
                root: root.clone(),
                name: c[1].to_string(),
                skips_auth: auth_skipping(&f),
                signature,
                line: line_of(text, at + f.len() - f.trim_start().len()),
            });
        }
    }
    out
}

/// Operations declared with `@Query(…)`, `@Mutation(…)` or
/// `@Subscription(…)` decorators on methods (NestJS, TypeGraphQL).
fn code_first_operations(text: &str) -> Vec<Operation> {
    static DECORATOR: OnceLock<Regex> = OnceLock::new();
    let decorator =
        DECORATOR.get_or_init(|| Regex::new(r"@(Query|Mutation|Subscription)\s*\(").unwrap());
    static METHOD: OnceLock<Regex> = OnceLock::new();
    let method = METHOD.get_or_init(|| {
        Regex::new(r"(?m)^\s*(?:(?:public|private|protected|async|static)\s+)*([A-Za-z_]\w*)\s*\(")
            .unwrap()
    });
    let mut out = Vec::new();
    for m in decorator.captures_iter(text) {
        let at = m.get(0).map_or(0, |g| g.start());
        // The decorated method is the first line after the decorators.
        let after = &text[at..];
        let mut skip_to = 0;
        let mut rest = after;
        // Skip this and any further decorators (which may span lines).
        loop {
            let trimmed = rest.trim_start();
            if !trimmed.starts_with('@') {
                break;
            }
            let lead = rest.len() - trimmed.len();
            let Some(end) = decorator_end(trimmed) else {
                break;
            };
            skip_to += lead + end;
            rest = &after[skip_to..];
        }
        let decorators = &after[..skip_to];
        let Some(c) = method.captures(rest) else {
            continue;
        };
        let name = c[1].to_string();
        if matches!(name.as_str(), "if" | "for" | "while" | "switch" | "return") {
            continue;
        }
        let sig_end = rest.find('{').unwrap_or(rest.len()).min(400);
        let signature: String = rest[..sig_end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        out.push(Operation {
            root: m[1].to_string(),
            name,
            signature,
            skips_auth: auth_skipping(decorators),
            line: line_of(text, at),
        });
    }
    out
}

/// The length of one decorator (`@Name(…)` or `@Name`) at the start of `s`.
fn decorator_end(s: &str) -> Option<usize> {
    let mut i = 1;
    let bytes = s.as_bytes();
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
    {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'(' {
        let mut depth = 0;
        for (j, c) in s[i..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + j + 1);
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    Some(i)
}

fn is_schema_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".graphql") || lower.ends_with(".gql") || lower.ends_with(".graphqls")
}

fn operations(path: &str, text: &str) -> Vec<Operation> {
    if is_schema_file(path) {
        return sdl_operations(text);
    }
    if !is_code(path) {
        return vec![];
    }
    let mut ops = code_first_operations(text);
    // Schemas written in code: gql`…` and graphql`…` templates.
    if text.contains("gql`") || text.contains("graphql`") || text.contains("typeDefs") {
        ops.extend(sdl_operations(text));
    }
    ops
}

fn is_code(path: &str) -> bool {
    [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts"]
        .iter()
        .any(|e| path.ends_with(e))
        && !path.ends_with(".d.ts")
}

fn root_noun(root: &str, n: usize) -> &'static str {
    match (root, n) {
        ("Query", 1) => "query",
        ("Query", _) => "queries",
        ("Mutation", 1) => "mutation",
        ("Mutation", _) => "mutations",
        (_, 1) => "subscription",
        _ => "subscriptions",
    }
}

fn graphql_rows(ctx: &Ctx, f: &FileChange) -> Vec<SemanticChange> {
    let base = operations(f.base_path(), &base_text(ctx, f));
    let head = operations(&f.path, &head_text(ctx, f));
    if base.is_empty() && head.is_empty() {
        return vec![];
    }
    let key = |o: &Operation| (o.root.clone(), o.name.clone());
    let base_by: BTreeMap<(String, String), &Operation> =
        base.iter().map(|o| (key(o), o)).collect();
    let head_by: BTreeMap<(String, String), &Operation> =
        head.iter().map(|o| (key(o), o)).collect();
    let added: Vec<&Operation> = head
        .iter()
        .filter(|o| !base_by.contains_key(&key(o)))
        .collect();
    let removed: Vec<&Operation> = base
        .iter()
        .filter(|o| !head_by.contains_key(&key(o)))
        .collect();
    let now_public: Vec<&Operation> = head
        .iter()
        .filter(|o| {
            o.skips_auth.is_some() && base_by.get(&key(o)).is_some_and(|b| b.skips_auth.is_none())
        })
        .collect();
    let changed: Vec<&Operation> = head
        .iter()
        .filter(|o| {
            base_by.get(&key(o)).is_some_and(|b| {
                canonical_signature(&b.signature) != canonical_signature(&o.signature)
                    && b.skips_auth == o.skips_auth
            })
        })
        .collect();
    let comp = component(ctx, &f.path);
    let path = &f.path;
    let mut rows = Vec::new();
    let mut push = |subkind: &str,
                    kind: ChangeKind,
                    title: String,
                    why: String,
                    ops: &[&Operation],
                    head_side: bool,
                    id: &str| {
        let locations = ops
            .iter()
            .map(|o| {
                if head_side {
                    Location::head(path, o.line, o.line)
                } else {
                    Location::base(f.base_path(), o.line, o.line)
                }
            })
            .collect();
        let mut row = change(
            kind,
            subkind,
            ChangeLevel::Structure,
            path,
            comp.as_deref(),
            "Public API",
            title,
            why,
            locations,
        );
        row.id = format!("{subkind}:{path}#{id}");
        row.hints.needs_person = true;
        rows.push(row);
    };
    let names = |ops: &[&Operation]| -> String {
        join_some(
            &ops.iter()
                .map(|o| format!("`{}`", o.name))
                .collect::<Vec<_>>(),
            5,
        )
    };
    let describe = |ops: &[&Operation]| -> String {
        let mut by_root: BTreeMap<&str, Vec<&Operation>> = BTreeMap::new();
        for o in ops {
            by_root.entry(o.root.as_str()).or_default().push(o);
        }
        let parts: Vec<String> = by_root
            .iter()
            .map(|(root, list)| format!("{} {}", root_noun(root, list.len()), names(list)))
            .collect();
        join_and(&parts)
    };
    let public: Vec<&Operation> = added
        .iter()
        .chain(now_public.iter())
        .copied()
        .filter(|o| o.skips_auth.is_some())
        .collect();
    if !public.is_empty() {
        let markers: BTreeSet<String> =
            public.iter().filter_map(|o| o.skips_auth.clone()).collect();
        push(
            "public-api-changed",
            ChangeKind::SecuritySensitive,
            format!(
                "GraphQL {} now answers without authentication",
                describe(&public)
            ),
            format!(
                "Marked {}: anyone can call {}; check what {} return and that nothing private \
                 leaks",
                join_and(&code(markers)),
                if public.len() == 1 { "it" } else { "them" },
                if public.len() == 1 {
                    "it can"
                } else {
                    "they can"
                },
            ),
            &public,
            true,
            "public",
        );
    }
    let new_ops: Vec<&Operation> = added
        .iter()
        .copied()
        .filter(|o| o.skips_auth.is_none())
        .collect();
    if !new_ops.is_empty() {
        push(
            "public-api-changed",
            ChangeKind::Additive,
            format!("New GraphQL {}", describe(&new_ops)),
            "New public API: check who may call each operation and what it reads or writes".into(),
            &new_ops,
            true,
            "added",
        );
    }
    if !removed.is_empty() {
        push(
            "public-api-changed",
            ChangeKind::Breaking,
            format!("GraphQL {} removed", describe(&removed)),
            "Clients that still call them get errors; check that none do".into(),
            &removed,
            false,
            "removed",
        );
    }
    if !changed.is_empty() {
        push(
            "public-api-changed",
            ChangeKind::Breaking,
            format!(
                "GraphQL {} {}",
                describe(&changed),
                if changed.len() == 1 {
                    "changes its arguments or result"
                } else {
                    "change their arguments or results"
                }
            ),
            "Clients built against the old signature may break".into(),
            &changed,
            true,
            "changed",
        );
    }
    rows
}

// ---------------------------------------------------------------------------
// HTTP routes

/// The URL path of a file-based route, or `None` when the file is not one:
/// SvelteKit `src/routes/**/+server.ts`, Next.js `app/**/route.ts` and
/// `pages/api/**`.
fn route_path(path: &str) -> Option<(String, bool)> {
    let clean = |segments: &[&str]| -> String {
        let kept: Vec<&str> = segments
            .iter()
            .copied()
            .filter(|s| !(s.starts_with('(') && s.ends_with(')')) && !s.starts_with('@'))
            .collect();
        format!("/{}", kept.join("/"))
    };
    let parts: Vec<&str> = path.split('/').collect();
    let name = *parts.last()?;
    let stem = name.split('.').next().unwrap_or(name);
    let dirs = &parts[..parts.len() - 1];
    if stem == "+server" {
        let at = dirs.iter().rposition(|d| *d == "routes")?;
        return Some((clean(&dirs[at + 1..]), false));
    }
    if stem == "route" && is_code(path) {
        let at = dirs.iter().rposition(|d| *d == "app")?;
        return Some((clean(&dirs[at + 1..]), false));
    }
    if is_code(path) {
        let at = dirs.iter().rposition(|d| *d == "pages")?;
        if dirs.get(at + 1) == Some(&"api") {
            let mut segs: Vec<&str> = dirs[at + 1..].to_vec();
            if stem != "index" {
                segs.push(stem);
            }
            return Some((clean(&segs), true));
        }
    }
    None
}

/// HTTP methods a route module exports (`export const GET`, `export async
/// function POST`); a Next.js `pages/api` module answers every method.
fn exported_methods(text: &str, any_method: bool) -> Vec<(String, u32)> {
    static EXPORT: OnceLock<Regex> = OnceLock::new();
    let re = EXPORT.get_or_init(|| {
        Regex::new(r"(?m)^\s*export\s+(?:const\s+|let\s+|(?:async\s+)?function\s+)(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS|fallback)\b").unwrap()
    });
    let mut out: Vec<(String, u32)> = re
        .captures_iter(text)
        .map(|c| {
            let m = c[1].to_string();
            let m = if m == "fallback" {
                "ANY".to_string()
            } else {
                m
            };
            (m, line_of(text, c.get(0).map_or(0, |g| g.start())))
        })
        .collect();
    if out.is_empty() && any_method && text.contains("export default") {
        let at = text.find("export default").unwrap_or(0);
        out.push(("ANY".into(), line_of(text, at)));
    }
    out
}

/// Routes of a controller class: `@Controller('users')` with `@Get(':id')`.
fn controller_routes(text: &str) -> Vec<(String, u32)> {
    static CONTROLLER: OnceLock<Regex> = OnceLock::new();
    let controller = CONTROLLER
        .get_or_init(|| Regex::new(r#"@Controller\(\s*(?:['"`]([^'"`]*)['"`])?"#).unwrap());
    static ROUTE: OnceLock<Regex> = OnceLock::new();
    let route = ROUTE.get_or_init(|| {
        Regex::new(r#"@(Get|Post|Put|Patch|Delete|Head|Options|All)\(\s*(?:['"`]([^'"`]*)['"`])?"#)
            .unwrap()
    });
    let Some(c) = controller.captures(text) else {
        return vec![];
    };
    let prefix = c.get(1).map_or("", |m| m.as_str()).trim_matches('/');
    route
        .captures_iter(text)
        .map(|r| {
            let method = r[1].to_ascii_uppercase();
            let method = if method == "ALL" {
                "ANY".to_string()
            } else {
                method
            };
            let sub = r.get(2).map_or("", |m| m.as_str()).trim_matches('/');
            let path = [prefix, sub]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("/");
            (
                format!("{method} /{path}"),
                line_of(text, r.get(0).map_or(0, |g| g.start())),
            )
        })
        .collect()
}

fn routes(path: &str, text: &str) -> Vec<(String, u32)> {
    if text.is_empty() {
        return vec![];
    }
    if let Some((url, any)) = route_path(path) {
        return exported_methods(text, any)
            .into_iter()
            .map(|(m, l)| (format!("{m} {url}"), l))
            .collect();
    }
    if is_code(path) && text.contains("@Controller(") {
        return controller_routes(text);
    }
    vec![]
}

fn route_rows(ctx: &Ctx, f: &FileChange) -> Vec<SemanticChange> {
    let base = routes(f.base_path(), &base_text(ctx, f));
    let head = routes(&f.path, &head_text(ctx, f));
    let base_set: BTreeSet<&str> = base.iter().map(|(r, _)| r.as_str()).collect();
    let head_set: BTreeSet<&str> = head.iter().map(|(r, _)| r.as_str()).collect();
    let added: Vec<&(String, u32)> = head
        .iter()
        .filter(|(r, _)| !base_set.contains(r.as_str()))
        .collect();
    let removed: Vec<&(String, u32)> = base
        .iter()
        .filter(|(r, _)| !head_set.contains(r.as_str()))
        .collect();
    let comp = component(ctx, &f.path);
    let path = &f.path;
    let mut rows = Vec::new();
    let text = head_text(ctx, f);
    if !added.is_empty() {
        let names = code(added.iter().map(|(r, _)| r.clone()));
        let mut why = vec![
            "New public surface: check who may call it (authentication and \
                            authorization) and what it returns"
                .to_string(),
        ];
        if let Some(cache) = cache_header(&text) {
            why.push(format!("it sets `{cache}`"));
        }
        if let Some(public) = auth_skipping(&text) {
            why.push(format!("it is marked `{}`", public.trim_start_matches('@')));
        }
        let mut row = change(
            ChangeKind::SecuritySensitive,
            "route-added",
            ChangeLevel::Structure,
            path,
            comp.as_deref(),
            "HTTP route",
            format!(
                "New HTTP {} {}{}",
                if added.len() == 1 { "route" } else { "routes" },
                join_some(&names, 5),
                comp.as_ref()
                    .map(|c| format!(" in `{c}`"))
                    .unwrap_or_default()
            ),
            why.join("; "),
            added
                .iter()
                .map(|(_, l)| Location::head(path, *l, *l))
                .collect(),
        );
        row.id = format!("route-added:{path}");
        row.hints.needs_person = true;
        rows.push(row);
    }
    if !removed.is_empty() {
        let names = code(removed.iter().map(|(r, _)| r.clone()));
        let mut row = change(
            ChangeKind::Breaking,
            "route-removed",
            ChangeLevel::Structure,
            path,
            comp.as_deref(),
            "HTTP route",
            format!(
                "HTTP {} {} removed{}",
                if removed.len() == 1 {
                    "route"
                } else {
                    "routes"
                },
                join_some(&names, 5),
                comp.as_ref()
                    .map(|c| format!(" from `{c}`"))
                    .unwrap_or_default()
            ),
            "Clients that still call it get errors; check that none do".into(),
            removed
                .iter()
                .map(|(_, l)| Location::base(f.base_path(), *l, *l))
                .collect(),
        );
        row.id = format!("route-removed:{path}");
        row.hints.needs_person = true;
        rows.push(row);
    }
    rows
}

/// The literal `cache-control` values set in the text.
fn cache_header(text: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)['"]cache-control['"]\s*[:,]\s*['"`]([^'"`]+)['"`]"#).unwrap()
    });
    let mut values: Vec<String> = Vec::new();
    for c in re.captures_iter(text) {
        if !values.iter().any(|v| v == &c[1]) {
            values.push(c[1].to_string());
        }
    }
    match values.as_slice() {
        [] => None,
        [one] => Some(format!("cache-control: {one}")),
        _ => Some(format!(
            "cache-control: {}",
            values.join("` or `cache-control: ")
        )),
    }
}

// ---------------------------------------------------------------------------
// Authentication and authorization code

/// Words in file and folder names that mean authentication or
/// authorization code.
const AUTH_WORDS: &[&str] = &[
    "auth",
    "authn",
    "authz",
    "authorization",
    "authorize",
    "authorizer",
    "authorized",
    "authentication",
    "authenticate",
    "authenticated",
    "permission",
    "permissions",
    "guard",
    "guards",
    "acl",
    "acls",
    "rbac",
    "abac",
    "role",
    "roles",
    "jwt",
    "oauth",
    "oauth2",
    "oidc",
    "saml",
    "sso",
    "csrf",
    "password",
    "passwords",
    "credential",
    "credentials",
    "impersonation",
    "impersonate",
    "mfa",
    "totp",
    "2fa",
    "policy",
    "policies",
    "rls",
];

/// The words of a path: folders and file name split at `/ . - _` and at
/// camelCase boundaries, lower-cased.
fn path_words(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in path.split(['/', '.', '-', '_']) {
        let mut word = String::new();
        let chars: Vec<char> = part.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            let boundary = c.is_uppercase()
                && i > 0
                && (chars[i - 1].is_lowercase()
                    || chars.get(i + 1).is_some_and(|n| n.is_lowercase())
                        && chars[i - 1].is_uppercase());
            if boundary && !word.is_empty() {
                out.push(std::mem::take(&mut word).to_lowercase());
            }
            word.push(*c);
        }
        if !word.is_empty() {
            out.push(word.to_lowercase());
        }
    }
    out
}

pub fn is_auth_path(path: &str) -> bool {
    path_words(path)
        .iter()
        .any(|w| AUTH_WORDS.contains(&w.as_str()))
}

fn auth_rows(ctx: &Ctx, existing: &[SemanticChange]) -> Vec<SemanticChange> {
    let formatting = crate::internal::formatting_only(ctx);
    let moves = ctx.moves.borrow();
    let claimed: BTreeSet<&str> = existing
        .iter()
        .flat_map(|r| r.locations.iter().map(|l| l.file.as_str()))
        .collect();
    let mut by_component: BTreeMap<String, Vec<&FileChange>> = BTreeMap::new();
    for f in &ctx.text.files {
        if f.status == Status::Renamed
            || !is_code(&f.path)
            || ctx.is_test_file(&f.path)
            || ctx.is_test_data(&f.path)
            || formatting.contains(&f.path)
            || moves.contains_key(&f.path)
            || claimed.contains(f.path.as_str())
            || !is_auth_path(&f.path)
        {
            continue;
        }
        by_component
            .entry(ctx.component_of_path(&f.path))
            .or_default()
            .push(f);
    }
    let mut rows = Vec::new();
    for (comp, files) in by_component {
        let names = code(files.iter().map(|f| f.path.clone()));
        let lines: u32 = files
            .iter()
            .flat_map(|f| &f.hunks)
            .map(|h| (h.head_lines.len() + h.base_lines.len()) as u32)
            .sum();
        let place = if comp == "root" {
            String::new()
        } else {
            format!(" in `{comp}`")
        };
        let mut row = change(
            ChangeKind::SecuritySensitive,
            "auth-code-changed",
            ChangeLevel::Behavior,
            &comp,
            (comp != "root").then_some(comp.as_str()),
            "Auth code",
            format!(
                "Authentication or authorization code changed{place}: {}",
                join_some(&names, 3)
            ),
            format!(
                "{} in files whose names say they decide who may do what; even a refactor here \
                 needs a person to check that access stays the same",
                capitalize(&plural(lines, "changed line", "changed lines"))
            ),
            files.iter().flat_map(|f| changed_locations(f)).collect(),
        );
        row.id = format!("auth-code-changed:{comp}");
        row.hints.needs_person = true;
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_migrations() {
        for p in [
            "libs/db/src/migrations/1790000013000-createPosts.ts",
            "prisma/migrations/20240101_init/migration.sql",
            "db/migrate/20240101000000_add_users.rb",
            "src/db/V2__add_index.sql",
            "src/user.migration.ts",
            "src/upgrade-version-command/2-46/2-46-workspace-command-1791322315043-add.command.ts",
            "alembic/versions/abc123_add.py",
        ] {
            assert!(is_migration(p), "{p}");
        }
        for p in [
            "src/migrate.ts",
            "src/commands/upgrade.command.ts",
            "docs/migrations/guide.md",
        ] {
            assert!(!is_migration(p), "{p}");
        }
        assert_eq!(
            migration_kind("libs/db/src/migrations/migrations.ts"),
            Some(MigrationKind::Support)
        );
        assert_eq!(
            migration_kind("apps/core/migrations/src/tables/community.ts"),
            Some(MigrationKind::Support)
        );
    }

    #[test]
    fn reads_what_a_migration_does() {
        let (effects, cascade) = migration_effects(
            "CREATE TABLE IF NOT EXISTS \"posts\" (id uuid, p uuid REFERENCES x ON DELETE CASCADE);\n\
             ALTER TABLE users ADD COLUMN cv bytea, ADD COLUMN cv_name text;\n\
             queryRunner.dropColumn('users', 'legacy');\n\
             async down() { DROP TABLE posts; }",
        );
        assert_eq!(
            effects,
            [
                "creates table `posts`",
                "adds column `users.cv`",
                "adds column `users.cv_name`",
                "drops column `users.legacy`"
            ]
        );
        assert!(cascade);
        assert_eq!(
            summarize_effects(&["creates table `a`".into(), "creates table `b`".into()]),
            ["creates tables `a` and `b`"]
        );
    }

    #[test]
    fn reads_graphql_operations() {
        let ops = sdl_operations(
            "type Post { id: ID! }\nextend type Query {\n  posts(\n    first: Int\n  ): [Post!]!\n  image(id: ID!): String @skipAuth\n}\ntype Mutation { like(id: ID!): Boolean! }\n",
        );
        let names: Vec<(&str, &str, bool)> = ops
            .iter()
            .map(|o| (o.root.as_str(), o.name.as_str(), o.skips_auth.is_some()))
            .collect();
        assert_eq!(
            names,
            [
                ("Query", "posts", false),
                ("Query", "image", true),
                ("Mutation", "like", false)
            ]
        );
        let ops = code_first_operations(
            "@Resolver()\nclass R {\n  @Public()\n  @Mutation(() => Boolean)\n  @UseGuards(\n    A,\n  )\n  async subscribe(@Args('id') id: string) {}\n  @Query(() => [String]) list() {}\n}",
        );
        let names: Vec<&str> = ops.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["subscribe", "list"]);
    }

    #[test]
    fn argument_order_is_no_change() {
        assert_eq!(
            canonical_signature("posts(first: Int, after: String): [Post!]!"),
            canonical_signature("posts(after: String first: Int): [Post!]!")
        );
        assert_ne!(
            canonical_signature("posts(first: Int): [Post!]!"),
            canonical_signature("posts(first: Int!): [Post!]!")
        );
    }

    #[test]
    fn reads_routes() {
        assert_eq!(
            route_path("apps/web/src/routes/(app)/files/[id]/+server.ts"),
            Some(("/files/[id]".into(), false))
        );
        assert_eq!(
            route_path("app/(site)/api/cv/[slug]/route.ts"),
            Some(("/api/cv/[slug]".into(), false))
        );
        assert_eq!(
            route_path("src/pages/api/users/index.ts"),
            Some(("/api/users".into(), true))
        );
        assert_eq!(route_path("src/pages/about.tsx"), None);
        assert_eq!(
            controller_routes(
                "@Controller('users')\nclass C {\n  @Get(':id')\n  one() {}\n  @Post()\n  make() {}\n}"
            ),
            [
                ("GET /users/:id".to_string(), 3),
                ("POST /users".to_string(), 5)
            ]
        );
    }

    #[test]
    fn recognizes_auth_paths() {
        for p in [
            "src/permissions/permissions.utils.ts",
            "src/modules/auth/sign-in.ts",
            "src/useAuthGuard.ts",
            "src/twenty-orm/utils/build-row-access-policy.util.ts",
            "src/JWTStrategy.ts",
        ] {
            assert!(is_auth_path(p), "{p}");
        }
        for p in ["src/author.ts", "src/authorsList.tsx", "src/chat/thread.ts"] {
            assert!(!is_auth_path(p), "{p}");
        }
    }
}
