//! The regression corpus: small, invented changes that reproduce what
//! reviewers found wrong in real reports (PLAN.md, M7). Each case in
//! `tests/corpus/*.case` states which rows must and must not appear and how
//! many need a person; `tests/corpus/README.md` describes the format.
//!
//! A case marked `status: open` documents a known mistake: it may fail, and
//! it must be unmarked as soon as it passes.

use std::path::Path;

use onus_core::{SemanticChange, SemanticReport};

#[derive(Debug)]
struct Case {
    name: String,
    open: bool,
    base: Vec<(String, String)>,
    head: Vec<(String, String)>,
    expect: Vec<Expect>,
}

#[derive(Debug)]
enum Expect {
    /// A row with this subkind, flag and text.
    Row(Matcher),
    /// No row with this subkind and text.
    No(Matcher),
    /// Exactly this many rows need a person.
    Attention(usize),
}

#[derive(Debug)]
struct Matcher {
    subkind: String,
    /// `Some(true)` for ●, `Some(false)` for ○.
    person: Option<bool>,
    title: Vec<String>,
    why: Vec<String>,
}

impl Matcher {
    fn matches(&self, r: &SemanticChange) -> bool {
        (self.subkind == "*" || r.subkind == self.subkind)
            && self.person.is_none_or(|p| r.hints.needs_person == p)
            && self.title.iter().all(|t| r.title.contains(t.as_str()))
            && self
                .why
                .iter()
                .all(|t| r.why_it_matters.contains(t.as_str()))
    }

    fn parse(words: &str) -> Matcher {
        let mut m = Matcher {
            subkind: String::new(),
            person: None,
            title: vec![],
            why: vec![],
        };
        let mut rest = words.trim();
        while !rest.is_empty() {
            let (field, after) = if let Some(r) = rest.strip_prefix("title~\"") {
                ("title", r)
            } else if let Some(r) = rest.strip_prefix("why~\"") {
                ("why", r)
            } else {
                let (word, after) = rest.split_once(' ').unwrap_or((rest, ""));
                match word {
                    "●" => m.person = Some(true),
                    "○" => m.person = Some(false),
                    w if m.subkind.is_empty() => m.subkind = w.to_string(),
                    w => panic!("unexpected `{w}` in `{words}`"),
                }
                rest = after.trim_start();
                continue;
            };
            let end = after
                .find('"')
                .unwrap_or_else(|| panic!("unclosed quote in `{words}`"));
            let text = after[..end].to_string();
            if field == "title" {
                m.title.push(text);
            } else {
                m.why.push(text);
            }
            rest = after[end + 1..].trim_start();
        }
        assert!(!m.subkind.is_empty(), "no subkind in `{words}`");
        m
    }
}

fn parse(name: &str, text: &str) -> Case {
    let mut case = Case {
        name: name.to_string(),
        open: false,
        base: vec![],
        head: vec![],
        expect: vec![],
    };
    // Sections start with `=== <side> <path>` or `=== expect`.
    let mut section: Option<(String, String)> = None;
    let mut body = String::new();
    let flush = |section: &Option<(String, String)>, body: &mut String, case: &mut Case| {
        let Some((side, path)) = section else {
            body.clear();
            return;
        };
        let text = std::mem::take(body);
        match side.as_str() {
            "both" => {
                case.base.push((path.clone(), text.clone()));
                case.head.push((path.clone(), text));
            }
            "base" => case.base.push((path.clone(), text)),
            "head" => case.head.push((path.clone(), text)),
            "expect" => {
                for line in text.lines().map(str::trim) {
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
                    case.expect.push(match word {
                        "row" => Expect::Row(Matcher::parse(rest)),
                        "no" => Expect::No(Matcher::parse(rest)),
                        "attention" => Expect::Attention(rest.trim().parse().unwrap()),
                        w => panic!("{name}: unknown expectation `{w}`"),
                    });
                }
            }
            s => panic!("{name}: unknown section `{s}`"),
        }
    };
    for line in text.split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("=== ") {
            flush(&section, &mut body, &mut case);
            let header = header.trim();
            let (side, path) = header.split_once(' ').unwrap_or((header, ""));
            section = Some((side.to_string(), path.to_string()));
            continue;
        }
        if section.is_none() {
            if line.trim() == "status: open" {
                case.open = true;
            }
            continue;
        }
        body.push_str(line);
    }
    flush(&section, &mut body, &mut case);
    case
}

fn write(root: &Path, files: &[(String, String)]) {
    std::fs::create_dir_all(root).unwrap();
    for (path, text) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

fn report(case: &Case) -> SemanticReport {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("base"), &case.base);
    write(&dir.path().join("head"), &case.head);
    onus_cli::diff_dirs(
        &dir.path().join("base"),
        &dir.path().join("head"),
        &onus_cli::DiffOptions {
            base_label: "base".into(),
            head_label: "head".into(),
            ..Default::default()
        },
    )
    .unwrap()
    .report
}

/// What a case got wrong, or nothing when it passes.
fn failures(case: &Case, r: &SemanticReport) -> Vec<String> {
    let mut out = Vec::new();
    for e in &case.expect {
        match e {
            Expect::Row(m) => {
                if !r.changes.iter().any(|c| m.matches(c)) {
                    out.push(format!("missing row {m:?}"));
                }
            }
            Expect::No(m) => {
                for c in r.changes.iter().filter(|c| m.matches(c)) {
                    out.push(format!("unwanted row {}: {}", c.subkind, c.title));
                }
            }
            Expect::Attention(n) => {
                let got = r.changes.iter().filter(|c| c.hints.needs_person).count();
                if got != *n {
                    out.push(format!("{got} rows need a person, expected {n}"));
                }
            }
        }
    }
    if !out.is_empty() {
        out.push("rows:".into());
        for c in &r.changes {
            out.push(format!(
                "  {} {} | {} | {}",
                if c.hints.needs_person { "●" } else { "○" },
                c.subkind,
                c.title,
                c.why_it_matters
            ));
        }
    }
    out
}

fn cases() -> Vec<Case> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "case"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().to_string();
            parse(&name, &std::fs::read_to_string(p).unwrap())
        })
        .collect()
}

#[test]
fn corpus() {
    let only = std::env::var("ONUS_CORPUS_CASE").ok();
    let mut problems = Vec::new();
    let (mut passed, mut open) = (0, 0);
    for case in cases() {
        if only
            .as_ref()
            .is_some_and(|o| !case.name.contains(o.as_str()))
        {
            continue;
        }
        let r = report(&case);
        let fails = failures(&case, &r);
        match (fails.is_empty(), case.open) {
            (true, false) => passed += 1,
            // Naming a case shows why an open case still fails.
            (false, true) if only.is_none() => open += 1,
            (false, true) => {
                problems.push(format!("{} (open):\n  {}", case.name, fails.join("\n  ")))
            }
            (true, true) => {
                problems.push(format!("{}: passes now; remove `status: open`", case.name))
            }
            (false, false) => problems.push(format!("{}:\n  {}", case.name, fails.join("\n  "))),
        }
    }
    assert!(
        problems.is_empty(),
        "corpus: {passed} pass, {open} open, {} fail\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}
