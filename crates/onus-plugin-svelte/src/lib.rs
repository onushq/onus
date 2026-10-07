//! Svelte support for Onus, as a language plugin (`onus help plugins`).
//!
//! A component's `<script>` blocks are kept where they are and everything
//! else (markup, styles) is blanked out, keeping line breaks, so the script
//! parses as TypeScript (or JavaScript) with its real line numbers. The
//! TypeScript adapter then analyzes it together with the repository's
//! TypeScript files, so imports such as `$lib/api` resolve. Expressions in
//! the markup are not analyzed.

use std::collections::BTreeMap;

use onus_core::hash::short_hash;
use onus_core::{PartialMap, Workspace, WorkspaceFile};
use onus_lang_ts::SourceOverride;

/// The script blocks of a component, with everything else replaced by
/// spaces (line breaks kept), and whether they are JavaScript rather than
/// TypeScript.
pub fn scripts(component: &str) -> SourceOverride {
    let mut out: Vec<u8> = component
        .bytes()
        .map(|b| if b == b'\n' || b == b'\r' { b } else { b' ' })
        .collect();
    let lower = component.to_ascii_lowercase();
    let mut typescript = false;
    let mut found = false;
    let mut at = 0;
    while let Some(open) = lower[at..].find("<script").map(|i| i + at) {
        let Some(tag_end) = lower[open..].find('>').map(|i| i + open + 1) else {
            break;
        };
        let tag = &lower[open..tag_end];
        if tag.contains("lang=\"ts\"")
            || tag.contains("lang='ts'")
            || tag.contains("lang=\"typescript\"")
        {
            typescript = true;
        }
        let close = lower[tag_end..]
            .find("</script")
            .map_or(component.len(), |i| i + tag_end);
        out[tag_end..close].copy_from_slice(&component.as_bytes()[tag_end..close]);
        found = true;
        at = close;
    }
    let text = if found {
        String::from_utf8_lossy(&out).into_owned()
    } else {
        String::new()
    };
    SourceOverride {
        text,
        tsx: !typescript,
    }
}

fn is_svelte(path: &str) -> bool {
    path.ends_with(".svelte")
}

/// Facts for the Svelte files of the request. `all_files` is every file in
/// the tree, so imports of TypeScript modules resolve.
pub fn analyze(request_ws: &Workspace, all_files: &[String]) -> Result<PartialMap, String> {
    let given: BTreeMap<&str, &WorkspaceFile> = request_ws
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();
    let matcher = onus_map::discover::ComponentMatcher::new(&request_ws.components);
    let tests = onus_map::build::test_globs(None);
    let files: Vec<WorkspaceFile> = all_files
        .iter()
        .map(|p| match given.get(p.as_str()) {
            Some(f) => (*f).clone(),
            None => WorkspaceFile {
                path: p.clone(),
                component: matcher.component_of(p),
                is_test: tests.is_match(p),
            },
        })
        .collect();
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    let mut overrides = BTreeMap::new();
    for f in request_ws.files.iter().filter(|f| is_svelte(&f.path)) {
        let path = onus_core::paths::native(&request_ws.root, &f.path);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", f.path))?;
        overrides.insert(f.path.clone(), scripts(&text));
        texts.insert(f.path.clone(), text);
    }
    let ws = Workspace {
        files,
        ..request_ws.clone()
    };
    let mut map = onus_lang_ts::analyze(&ws, &overrides).map_err(|e| e.to_string())?;
    // Only what is about the Svelte files.
    map.files.retain(|f| is_svelte(&f.path));
    for f in &mut map.files {
        f.language = "svelte".into();
        // The whole component, markup included: a change to the markup is
        // a change, never "formatting only".
        let text = texts.get(&f.path).cloned().unwrap_or_default();
        let normalized: Vec<&str> = text.split_whitespace().collect();
        f.content_hash = short_hash(normalized.join(" ").as_bytes());
    }
    map.symbols
        .retain(|s| s.loc.as_ref().is_some_and(|l| is_svelte(&l.file)));
    for e in &mut map.edges {
        e.sites.retain(|s| is_svelte(&s.file));
    }
    map.edges.retain(|e| !e.sites.is_empty());
    map.tests.retain(|t| is_svelte(&t.file));
    map.diagnostics.retain(|d| is_svelte(&d.file));
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_scripts_in_place_and_blanks_the_rest() {
        let src = "<script lang=\"ts\">\n  let n: number = 1;\n</script>\n\n<h1 on:click={go}>{n}</h1>\n<style>h1 { color: red; }</style>\n";
        let s = scripts(src);
        assert!(!s.tsx);
        assert_eq!(s.text.lines().count(), src.lines().count());
        assert_eq!(s.text.lines().nth(1), Some("  let n: number = 1;"));
        assert!(!s.text.contains("h1"));
        assert!(!s.text.contains("color"));
        let js = scripts(
            "<script context=\"module\">export const x = 1;</script><script>let y = x;</script>",
        );
        assert!(js.tsx);
        assert!(js.text.contains("export const x = 1;") && js.text.contains("let y = x;"));
        assert_eq!(scripts("<p>no script</p>").text, "");
    }
}
