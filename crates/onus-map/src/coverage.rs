//! Line coverage as map facts (ADR 0017). Coverage files from test runs
//! (lcov, Cobertura XML, Istanbul JSON) say which lines of which files ran.
//! They join the map after it is built, like traces: the measured files with
//! their covered and uncovered lines, and, where lcov names the test that
//! ran (`TN:`), links from that test to the innermost symbol around each
//! line it ran.
//!
//! Line numbers belong to the commit the tests ran at, so coverage that
//! names a line past the end of a file is left out, and so is coverage
//! older than the file it measures (when the tree is a live worktree).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;
use std::time::SystemTime;

use onus_core::{
    CodebaseMap, Coverage, CoverageFormat, CoverageLink, CoverageOrigin, CoverageSource,
    FileCoverage, line_ranges,
};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::traces::map_file;

/// One coverage file to add.
#[derive(Debug, Clone)]
pub struct CoverageInput {
    /// Shown in sources and notes: the path as given, or the artifact path.
    pub label: String,
    pub bytes: Vec<u8>,
    pub origin: CoverageOrigin,
    /// The evidence manifest it was recorded in.
    pub manifest: Option<String>,
    /// The command of the run that recorded it.
    pub command: Option<String>,
    /// When the file was written; with a root, source files modified after
    /// it are left out (the live worktree). None: no check.
    pub written: Option<SystemTime>,
}

/// What adding coverage found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageStats {
    pub sources: u32,
    /// Sources per format (`lcov`, `cobertura`, `istanbul`).
    pub formats: BTreeMap<String, u32>,
    /// Map files with coverage.
    pub files: u32,
    /// Files in the coverage that are not in the map.
    pub unmatched: u32,
    /// Executable lines, and those that ran.
    pub lines: u32,
    pub ran: u32,
    /// Test to symbol links.
    pub links: u32,
}

impl CoverageStats {
    /// `2 sources (lcov 1, cobertura 1), 41 files matched, 3 not in the
    /// map; 1,204 of 1,530 lines ran; 18 test links`
    pub fn summary(&self) -> String {
        let formats: Vec<String> = self
            .formats
            .iter()
            .map(|(f, n)| format!("{f} {n}"))
            .collect();
        format!(
            "{} sources ({}), {} files matched, {} not in the map; {} of {} lines ran; {} test links",
            self.sources,
            formats.join(", "),
            thousands(self.files),
            thousands(self.unmatched),
            thousands(self.ran),
            thousands(self.lines),
            thousands(self.links)
        )
    }
}

fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Coverage files given by a person or CI, with their modification times.
/// A file inside the working directory is labelled by its path relative to
/// it, so reports do not depend on where the checkout is.
pub fn read_inputs(paths: &[impl AsRef<Path>]) -> Result<Vec<CoverageInput>, String> {
    let cwd = std::env::current_dir().and_then(std::fs::canonicalize).ok();
    paths
        .iter()
        .map(|p| {
            let p = p.as_ref();
            let bytes = std::fs::read(p)
                .map_err(|e| format!("cannot read coverage {}: {e}", p.display()))?;
            let label = cwd
                .as_deref()
                .zip(std::fs::canonicalize(p).ok())
                .and_then(|(cwd, full)| {
                    full.strip_prefix(cwd)
                        .ok()
                        .map(|rel| rel.display().to_string())
                })
                .unwrap_or_else(|| p.display().to_string());
            Ok(CoverageInput {
                label: label.replace('\\', "/"),
                bytes,
                origin: CoverageOrigin::Supplied,
                manifest: None,
                command: None,
                written: std::fs::metadata(p).and_then(|m| m.modified()).ok(),
            })
        })
        .collect()
}

/// Adds coverage to the map, replacing any it had. A supplied file that is
/// not coverage in a known format is an error. A recorded one is a note: an
/// environment's coverage globs may catch other files, such as an HTML
/// report next to `lcov.info`.
pub fn add_coverage(
    map: &mut CodebaseMap,
    root: Option<&Path>,
    inputs: &[CoverageInput],
) -> Result<CoverageStats, String> {
    add(map, root, inputs, |i| i.origin == CoverageOrigin::Supplied)
}

/// [`add_coverage`] for the map server: a file it cannot read is a note in
/// the map's coverage, not an error.
pub fn add_coverage_noting(
    map: &mut CodebaseMap,
    root: Option<&Path>,
    inputs: &[CoverageInput],
) -> CoverageStats {
    add(map, root, inputs, |_| false).unwrap_or_default()
}

/// Parses the inputs and merges them into the map. A file that cannot be
/// read is an error when `strict` says so, and otherwise a note.
fn add(
    map: &mut CodebaseMap,
    root: Option<&Path>,
    inputs: &[CoverageInput],
    strict: impl Fn(&CoverageInput) -> bool,
) -> Result<CoverageStats, String> {
    let mut notes = Vec::new();
    let mut parsed = Vec::new();
    for i in inputs {
        match parse(i) {
            Ok(p) => parsed.push((i, p)),
            Err(e) if strict(i) => return Err(e),
            Err(e) => notes.push(format!("{e}; left out")),
        }
    }
    Ok(apply(map, root, parsed, notes))
}

/// The lines one record of a coverage file says ran, for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    /// The test that ran (lcov `TN:`).
    test: Option<String>,
    /// Where the file may be, most likely first.
    paths: Vec<String>,
    /// Line → hits.
    lines: BTreeMap<u32, u64>,
}

fn detect(label: &str, text: &str) -> Result<CoverageFormat, String> {
    if text.trim_start().starts_with('{') {
        Ok(CoverageFormat::Istanbul)
    } else if text.contains("<coverage") {
        Ok(CoverageFormat::Cobertura)
    } else if text.lines().any(|l| l.trim_start().starts_with("SF:")) {
        Ok(CoverageFormat::Lcov)
    } else {
        Err(format!(
            "{label} is not lcov, Cobertura XML or Istanbul JSON"
        ))
    }
}

fn parse(input: &CoverageInput) -> Result<(CoverageFormat, Vec<Record>), String> {
    let text = String::from_utf8_lossy(&input.bytes);
    let format = detect(&input.label, &text)?;
    let records = match format {
        CoverageFormat::Lcov => lcov(&text),
        CoverageFormat::Cobertura => cobertura(&text),
        CoverageFormat::Istanbul => {
            istanbul(&text).map_err(|e| format!("{} is not Istanbul JSON: {e}", input.label))?
        }
    };
    Ok((format, records))
}

fn lcov(text: &str) -> Vec<Record> {
    let mut out = Vec::new();
    let mut test: Option<String> = None;
    let mut current: Option<Record> = None;
    for line in text.lines().map(str::trim) {
        if let Some(t) = line.strip_prefix("TN:") {
            let t = t.trim();
            test = (!t.is_empty()).then(|| t.to_string());
        } else if let Some(p) = line.strip_prefix("SF:") {
            out.extend(current.take());
            current = Some(Record {
                test: test.clone(),
                paths: vec![p.trim().to_string()],
                lines: BTreeMap::new(),
            });
        } else if let Some(d) = line.strip_prefix("DA:") {
            let mut parts = d.split(',');
            let n = parts.next().and_then(|n| n.trim().parse::<u32>().ok());
            // Negative or unreadable hits count as none.
            let hits = parts
                .next()
                .and_then(|h| h.trim().parse::<i64>().ok())
                .map_or(0, |h| h.max(0) as u64);
            if let (Some(n), Some(r)) = (n.filter(|n| *n > 0), current.as_mut()) {
                *r.lines.entry(n).or_default() += hits;
            }
        } else if line == "end_of_record" {
            out.extend(current.take());
        }
    }
    out.extend(current);
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

static SOURCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<source>(.*?)</source>").expect("valid regex"));
static CLASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<class\b([^>]*?)(?:/>|>(.*?)</class>)").expect("valid regex")
});
static FILENAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bfilename="([^"]*)""#).expect("valid regex"));
static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<line\b[^>]*>").expect("valid regex"));
static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bnumber="(\d+)""#).expect("valid regex"));
static HITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bhits="(\d+)""#).expect("valid regex"));

fn cobertura(text: &str) -> Vec<Record> {
    let sources: Vec<String> = SOURCE
        .captures_iter(text)
        .map(|c| {
            unescape(c[1].trim())
                .trim_end_matches(['/', '\\'])
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = Vec::new();
    for c in CLASS.captures_iter(text) {
        let Some(name) = FILENAME.captures(&c[1]).map(|f| unescape(&f[1])) else {
            continue;
        };
        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        // Method lines repeat class lines; the union is the same.
        for l in LINE.find_iter(c.get(2).map_or("", |b| b.as_str())) {
            let l = l.as_str();
            let n = NUMBER.captures(l).and_then(|m| m[1].parse::<u32>().ok());
            let hits = HITS
                .captures(l)
                .and_then(|m| m[1].parse::<u64>().ok())
                .unwrap_or(0);
            if let Some(n) = n.filter(|n| *n > 0) {
                let e = lines.entry(n).or_default();
                *e = (*e).max(hits);
            }
        }
        let mut paths: Vec<String> = sources.iter().map(|s| format!("{s}/{name}")).collect();
        paths.push(name);
        out.push(Record {
            test: None,
            paths,
            lines,
        });
    }
    out
}

/// A file entry: `statementMap` and `s`, maybe wrapped in `data`.
fn istanbul_entry(v: &Value) -> Option<&Value> {
    let v = v.get("data").filter(|d| d.is_object()).unwrap_or(v);
    v.get("statementMap").is_some().then_some(v)
}

fn istanbul(text: &str) -> Result<Vec<Record>, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let top = doc.as_object().ok_or("not an object")?;
    // Entries by path, directly or under a top-level `data`.
    let mut entries: Vec<(&String, &Value)> = Vec::new();
    for (k, v) in top {
        match istanbul_entry(v) {
            Some(e) => entries.push((k, e)),
            None if k == "data" => {
                for (k, v) in v.as_object().into_iter().flatten() {
                    if let Some(e) = istanbul_entry(v) {
                        entries.push((k, e));
                    }
                }
            }
            None => {}
        }
    }
    let mut out = Vec::new();
    for (key, e) in entries {
        let path = e.get("path").and_then(Value::as_str).unwrap_or(key);
        let hits = e.get("s");
        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        // A line is executable when a statement starts on it, and ran when
        // any of those statements did, as Istanbul's own lcov reporter says.
        for (id, s) in e["statementMap"].as_object().into_iter().flatten() {
            let Some(n) = s
                .pointer("/start/line")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
            else {
                continue;
            };
            let h = hits
                .and_then(|h| h.get(id))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            *lines.entry(n).or_default() += h;
        }
        out.push(Record {
            test: None,
            paths: vec![path.to_string()],
            lines,
        });
    }
    Ok(out)
}

/// The test a lcov `TN:` names: its file, its id, or a path ending in its
/// file.
fn test_named<'m>(map: &'m CodebaseMap, tn: &str) -> Option<&'m str> {
    let tn = tn.replace('\\', "/");
    let tn = tn.trim_start_matches("./");
    if let Some(t) = map.tests.iter().find(|t| t.file == tn || t.id == tn) {
        return Some(&t.id);
    }
    // A path that only ends the same way names a test only when one does.
    let mut ends = map
        .tests
        .iter()
        .filter(|t| tn.ends_with(&format!("/{}", t.file)) || t.file.ends_with(&format!("/{tn}")));
    match (ends.next(), ends.next()) {
        (Some(t), None) => Some(&t.id),
        _ => None,
    }
}

/// Merges parsed inputs into the map's coverage.
fn apply(
    map: &mut CodebaseMap,
    root: Option<&Path>,
    parsed: Vec<(&CoverageInput, (CoverageFormat, Vec<Record>))>,
    mut notes: Vec<String>,
) -> CoverageStats {
    let files: BTreeSet<&str> = map.files.iter().map(|f| f.path.as_str()).collect();
    let info: BTreeMap<&str, (u32, bool)> = map
        .files
        .iter()
        .map(|f| (f.path.as_str(), (f.lines, f.is_test)))
        .collect();
    // Per file, its symbols with their spans.
    let mut symbols: BTreeMap<&str, Vec<(u32, u32, &str)>> = BTreeMap::new();
    for s in &map.symbols {
        if let Some(l) = &s.loc {
            symbols
                .entry(l.file.as_str())
                .or_default()
                .push((l.start, l.end, s.id.as_str()));
        }
    }
    let note = |n: String, notes: &mut Vec<String>| {
        if !notes.contains(&n) {
            notes.push(n);
        }
    };
    let mut stats = CoverageStats::default();
    let mut sources = Vec::new();
    // File → (executable, covered).
    let mut merged: BTreeMap<&str, (BTreeSet<u32>, BTreeSet<u32>)> = BTreeMap::new();
    // (test, symbol) → lines it ran.
    let mut linked: BTreeMap<(String, String), BTreeSet<u32>> = BTreeMap::new();
    for (input, (format, records)) in parsed {
        stats.sources += 1;
        *stats
            .formats
            .entry(format.as_str().to_string())
            .or_default() += 1;
        // Resolve each record to a map file; test files are not the code
        // under test.
        let mut by_file: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
        let mut unmatched: BTreeSet<&str> = BTreeSet::new();
        for r in &records {
            match r.paths.iter().find_map(|p| map_file(&files, p)) {
                Some(f) if info.get(f).is_some_and(|(_, test)| *test) => {}
                Some(f) => by_file.entry(f).or_default().push(r),
                None => {
                    unmatched.insert(r.paths.last().map_or("", String::as_str));
                }
            }
        }
        let mut matched = 0;
        for (file, records) in by_file {
            let lines = info.get(file).map_or(0, |(n, _)| *n);
            if let Some(max) = records.iter().filter_map(|r| r.lines.keys().last()).max()
                && *max > lines
            {
                note(
                    format!(
                        "coverage of `{file}` from `{}` names line {max}, but the file has {lines} lines; left out",
                        input.label
                    ),
                    &mut notes,
                );
                continue;
            }
            if let (Some(root), Some(written)) = (root, input.written)
                && std::fs::metadata(onus_core::paths::native(root, file))
                    .and_then(|m| m.modified())
                    .is_ok_and(|m| m > written)
            {
                note(
                    format!(
                        "coverage of `{file}` is older than the file; left out (re-run the tests)"
                    ),
                    &mut notes,
                );
                continue;
            }
            matched += 1;
            let (executable, covered) = merged.entry(file).or_default();
            for r in &records {
                for (&n, &hits) in &r.lines {
                    executable.insert(n);
                    if hits > 0 {
                        covered.insert(n);
                    }
                }
                let Some(test) = r.test.as_deref().and_then(|t| test_named(map, t)) else {
                    continue;
                };
                for (&n, _) in r.lines.iter().filter(|(_, h)| **h > 0) {
                    let innermost = symbols
                        .get(file)
                        .into_iter()
                        .flatten()
                        .filter(|(start, end, _)| *start <= n && n <= *end)
                        .min_by_key(|(start, end, id)| (end - start, *id));
                    if let Some((_, _, id)) = innermost {
                        linked
                            .entry((test.to_string(), id.to_string()))
                            .or_default()
                            .insert(n);
                    }
                }
            }
        }
        stats.unmatched += unmatched.len() as u32;
        sources.push(CoverageSource {
            format,
            path: input.label.clone(),
            sha256: onus_core::hash::sha256_hex(&input.bytes),
            origin: input.origin,
            manifest: input.manifest.clone(),
            command: input.command.clone(),
            files: matched,
            unmatched: unmatched.len() as u32,
        });
    }
    sources.sort_by(|a, b| (&a.path, &a.sha256).cmp(&(&b.path, &b.sha256)));
    let files: Vec<FileCoverage> = merged
        .into_iter()
        .map(|(path, (executable, covered))| {
            stats.lines += executable.len() as u32;
            stats.ran += covered.len() as u32;
            let uncovered: BTreeSet<u32> = executable.difference(&covered).copied().collect();
            FileCoverage {
                path: path.to_string(),
                covered: line_ranges(&covered),
                uncovered: line_ranges(&uncovered),
            }
        })
        .collect();
    stats.files = files.len() as u32;
    let links: Vec<CoverageLink> = linked
        .into_iter()
        .map(|((test, target), lines)| CoverageLink {
            test,
            target,
            lines: lines.len() as u32,
        })
        .collect();
    stats.links = links.len() as u32;
    map.coverage = Some(Coverage {
        sources,
        files,
        links,
        notes,
    });
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(label: &str, text: &str) -> CoverageInput {
        CoverageInput {
            label: label.into(),
            bytes: text.as_bytes().to_vec(),
            origin: CoverageOrigin::Supplied,
            manifest: None,
            command: None,
            written: None,
        }
    }

    fn records(text: &str) -> Vec<Record> {
        parse(&input("c", text)).unwrap().1
    }

    /// `src/a.ts` (40 lines) with a class `A` (lines 1-20) holding a method
    /// `A.run` (5-10), a function `b` (25-30), and its test.
    fn map() -> CodebaseMap {
        serde_json::from_value(json!({
            "schemaVersion": 1, "commit": "test",
            "builtWith": { "onus": "test", "providers": {}, "configHash": "inferred" },
            "components": [], "externals": [], "packages": [], "rules": [], "diagnostics": [],
            "edges": [],
            "files": [
                { "path": "src/a.ts", "language": "typescript", "isTest": false, "lines": 40,
                  "contentHash": "x", "imports": [] },
                { "path": "src/a.test.ts", "language": "typescript", "isTest": true, "lines": 40,
                  "contentHash": "y", "imports": [] },
            ],
            "symbols": [
                { "id": "root:src/a.ts#A", "kind": "class", "name": "A", "visibility": "public",
                  "loc": { "file": "src/a.ts", "start": 1, "end": 20 } },
                { "id": "root:src/a.ts#A.run", "kind": "method", "name": "A.run",
                  "visibility": "public", "loc": { "file": "src/a.ts", "start": 5, "end": 10 } },
                { "id": "root:src/a.ts#b", "kind": "function", "name": "b",
                  "visibility": "internal", "loc": { "file": "src/a.ts", "start": 25, "end": 30 } },
            ],
            "tests": [{ "id": "root:src/a.test.ts", "file": "src/a.test.ts", "cases": [],
                        "exercises": ["root:src/a.ts#A"] }],
        }))
        .unwrap()
    }

    type Ranges = Vec<[u32; 2]>;

    /// Each file's path, covered and uncovered lines.
    fn files(m: &CodebaseMap) -> Vec<(String, Ranges, Ranges)> {
        m.coverage
            .as_ref()
            .unwrap()
            .files
            .iter()
            .map(|f| (f.path.clone(), f.covered.clone(), f.uncovered.clone()))
            .collect()
    }

    #[test]
    fn parses_lcov_with_tests() {
        let r = records(
            "TN:one\nSF:/w/src/a.ts\nFN:5,run\nDA:5,2,abc\nDA:6,-1\nDA:7,0\nLF:3\nLH:1\nend_of_record\n\
             TN:two\nSF:/w/src/a.ts\nDA:5,1\nDA:7,x\nend_of_record\n",
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].test.as_deref(), Some("one"));
        assert_eq!(r[0].paths, ["/w/src/a.ts"]);
        assert_eq!(r[0].lines, BTreeMap::from([(5, 2), (6, 0), (7, 0)]));
        assert_eq!(r[1].test.as_deref(), Some("two"));
        assert_eq!(r[1].lines, BTreeMap::from([(5, 1), (7, 0)]));
        // Hits from both tests count for the file.
        let mut m = map();
        let stats = add_coverage(&mut m, None, &[input("c.lcov", "TN:\nSF:src/a.ts\nDA:5,1\nDA:6,0\nend_of_record\nSF:src/a.ts\nDA:6,3\nend_of_record\n")]).unwrap();
        assert_eq!(files(&m), [("src/a.ts".into(), vec![[5, 6]], vec![])]);
        assert_eq!((stats.lines, stats.ran, stats.links), (2, 2, 0));
    }

    #[test]
    fn parses_cobertura_with_sources() {
        let xml = r#"<?xml version="1.0" ?>
<coverage line-rate="0.5">
  <sources><source>/app</source></sources>
  <packages><package name="src"><classes>
    <class name="a" filename="src/a&amp;b.ts" line-rate="0.5">
      <methods><method name="run"><lines><line hits="1" number="5"/></lines></method></methods>
      <lines>
        <line hits="1" number="5"/>
        <line number="6" hits="0" branch="false"/>
      </lines>
    </class>
    <class name="empty" filename="src/c.ts"/>
  </classes></package></packages>
</coverage>"#;
        let r = records(xml);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].paths, ["/app/src/a&b.ts", "src/a&b.ts"]);
        assert_eq!(r[0].lines, BTreeMap::from([(5, 1), (6, 0)]));
        assert!(r[1].lines.is_empty());
    }

    #[test]
    fn parses_istanbul_json() {
        let entry = json!({
            "path": "/w/src/a.ts",
            "statementMap": {
                "0": { "start": { "line": 5, "column": 0 }, "end": { "line": 5, "column": 9 } },
                "1": { "start": { "line": 5, "column": 10 }, "end": { "line": 5, "column": 20 } },
                "2": { "start": { "line": 6, "column": 0 }, "end": { "line": 6, "column": 9 } },
            },
            "s": { "0": 0, "1": 2, "2": 0 },
        });
        let direct = records(&json!({ "/w/src/a.ts": entry }).to_string());
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].paths, ["/w/src/a.ts"]);
        assert_eq!(direct[0].lines, BTreeMap::from([(5, 2), (6, 0)]));
        let wrapped = records(&json!({ "/w/src/a.ts": { "data": entry } }).to_string());
        assert_eq!(wrapped, direct);
        let under_data = records(&json!({ "data": { "/w/src/a.ts": entry } }).to_string());
        assert_eq!(under_data, direct);
        let mut m = map();
        add_coverage(
            &mut m,
            None,
            &[input("cov.json", &json!({ "x": entry }).to_string())],
        )
        .unwrap();
        assert_eq!(files(&m), [("src/a.ts".into(), vec![[5, 5]], vec![[6, 6]])]);
    }

    #[test]
    fn detects_format_or_fails() {
        assert_eq!(detect("c", "  {}"), Ok(CoverageFormat::Istanbul));
        assert_eq!(
            detect("c", "<?xml?>\n<coverage>"),
            Ok(CoverageFormat::Cobertura)
        );
        assert_eq!(detect("c", "TN:\nSF:a.ts\n"), Ok(CoverageFormat::Lcov));
        assert_eq!(
            detect("junit.xml", "<testsuite/>"),
            Err("junit.xml is not lcov, Cobertura XML or Istanbul JSON".into())
        );
        let mut m = map();
        assert!(add_coverage(&mut m, None, &[input("x.txt", "hello")]).is_err());
        // The map server notes it instead.
        add_coverage_noting(&mut m, None, &[input("x.txt", "hello")]);
        assert_eq!(
            m.coverage.take().unwrap().notes,
            ["x.txt is not lcov, Cobertura XML or Istanbul JSON; left out"]
        );
        // So does a report for a file an environment recorded: its globs may
        // catch an HTML report next to the lcov.
        let recorded = CoverageInput {
            origin: CoverageOrigin::Recorded,
            ..input("coverage/lcov-report/index.html", "<html></html>")
        };
        let lcov = CoverageInput {
            origin: CoverageOrigin::Recorded,
            ..input("coverage/lcov.info", "SF:src/a.ts\nDA:5,1\nend_of_record\n")
        };
        let stats = add_coverage(&mut m, None, &[recorded, lcov]).unwrap();
        assert_eq!((stats.sources, stats.files), (1, 1));
        assert_eq!(
            m.coverage.unwrap().notes,
            [
                "coverage/lcov-report/index.html is not lcov, Cobertura XML or Istanbul JSON; left out"
            ]
        );
    }

    #[test]
    fn merges_sources_any_ran_wins() {
        let mut m = map();
        let stats = add_coverage(
            &mut m,
            None,
            &[
                input("b.lcov", "SF:src/a.ts\nDA:5,0\nDA:6,0\nDA:7,1\nend_of_record\n"),
                input(
                    "a.xml",
                    r#"<coverage><class filename="src/a.ts"><line number="5" hits="3"/><line number="8" hits="0"/></class><class filename="gone.ts"><line number="1" hits="1"/></class></coverage>"#,
                ),
            ],
        )
        .unwrap();
        assert_eq!(
            files(&m),
            [(
                "src/a.ts".into(),
                vec![[5, 5], [7, 7]],
                vec![[6, 6], [8, 8]]
            )]
        );
        let c = m.coverage.as_ref().unwrap();
        // Sorted by path.
        assert_eq!(c.sources[0].path, "a.xml");
        assert_eq!(c.sources[0].format, CoverageFormat::Cobertura);
        assert_eq!((c.sources[0].files, c.sources[0].unmatched), (1, 1));
        assert_eq!(c.sources[1].sha256.len(), 64);
        assert_eq!(
            stats.summary(),
            "2 sources (cobertura 1, lcov 1), 1 files matched, 1 not in the map; 2 of 4 lines ran; 0 test links"
        );
    }

    #[test]
    fn drops_files_with_lines_beyond_the_file() {
        let mut m = map();
        add_coverage(
            &mut m,
            None,
            &[input(
                "old.lcov",
                "SF:src/a.ts\nDA:5,1\nDA:99,1\nend_of_record\n",
            )],
        )
        .unwrap();
        let c = m.coverage.unwrap();
        assert!(c.files.is_empty());
        assert_eq!(
            c.notes,
            [
                "coverage of `src/a.ts` from `old.lcov` names line 99, but the file has 40 lines; left out"
            ]
        );
        assert_eq!(c.sources[0].files, 0);
    }

    #[test]
    fn drops_stale_files_when_written_is_older() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let file = std::fs::File::create(dir.path().join("src/a.ts")).unwrap();
        let now = SystemTime::now();
        file.set_modified(now).unwrap();
        let lcov = "SF:src/a.ts\nDA:5,1\nend_of_record\n";
        let mut older = input("lcov.info", lcov);
        older.written = Some(now - std::time::Duration::from_secs(60));
        let mut m = map();
        add_coverage(&mut m, Some(dir.path()), &[older]).unwrap();
        let c = m.coverage.as_ref().unwrap();
        assert!(c.files.is_empty());
        assert_eq!(
            c.notes,
            ["coverage of `src/a.ts` is older than the file; left out (re-run the tests)"]
        );
        let mut newer = input("lcov.info", lcov);
        newer.written = Some(now + std::time::Duration::from_secs(60));
        add_coverage(&mut m, Some(dir.path()), &[newer]).unwrap();
        assert_eq!(files(&m), [("src/a.ts".into(), vec![[5, 5]], vec![])]);
        assert!(m.coverage.unwrap().notes.is_empty());
    }

    #[test]
    fn links_innermost_symbol() {
        let mut m = map();
        let stats = add_coverage(
            &mut m,
            None,
            &[input(
                "t.lcov",
                "TN:./src/a.test.ts\nSF:src/a.ts\nDA:2,1\nDA:6,1\nDA:7,1\nDA:8,0\nDA:22,1\nDA:26,1\nend_of_record\n\
                 TN:unknown.test.ts\nSF:src/a.ts\nDA:26,1\nend_of_record\n",
            )],
        )
        .unwrap();
        let links = &m.coverage.as_ref().unwrap().links;
        let got: Vec<(&str, &str, u32)> = links
            .iter()
            .map(|l| (l.test.as_str(), l.target.as_str(), l.lines))
            .collect();
        // Line 22 is outside every symbol; an unknown test links nothing.
        assert_eq!(
            got,
            [
                ("root:src/a.test.ts", "root:src/a.ts#A", 1),
                ("root:src/a.test.ts", "root:src/a.ts#A.run", 2),
                ("root:src/a.test.ts", "root:src/a.ts#b", 1),
            ]
        );
        assert_eq!(stats.links, 3);
        // An absolute TN names the test too.
        assert_eq!(
            test_named(&m, "/w/src/a.test.ts"),
            Some("root:src/a.test.ts")
        );
        assert_eq!(test_named(&m, "a.test.ts"), Some("root:src/a.test.ts"));
        // A name two tests end with names neither.
        let mut other = m.tests[0].clone();
        other.id = "root:lib/a.test.ts".into();
        other.file = "lib/a.test.ts".into();
        m.tests.push(other);
        assert_eq!(test_named(&m, "a.test.ts"), None);
        assert_eq!(
            test_named(&m, "/w/src/a.test.ts"),
            Some("root:src/a.test.ts")
        );
        assert_eq!(test_named(&m, "src/a.test.ts"), Some("root:src/a.test.ts"));
    }

    #[test]
    fn ignores_coverage_of_test_files() {
        let mut m = map();
        let stats = add_coverage(
            &mut m,
            None,
            &[input(
                "c.lcov",
                "SF:src/a.test.ts\nDA:1,1\nend_of_record\nSF:src/a.ts\nDA:5,1\nend_of_record\n",
            )],
        )
        .unwrap();
        assert_eq!(files(&m), [("src/a.ts".into(), vec![[5, 5]], vec![])]);
        assert_eq!((stats.files, stats.unmatched), (1, 0));
    }
}
