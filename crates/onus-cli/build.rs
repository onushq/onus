//! Embeds the web interface (`ui/build` at the repository root, made by
//! `npm run build` in `ui/`) into the binary for `onus ui`. Without it the
//! binary still builds, and `onus ui` says how to build the interface;
//! release builds set ONUS_REQUIRE_UI so they cannot ship without it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        let path = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if e.path().is_dir() {
            walk(&e.path(), &path, out);
        } else {
            out.push((path, e.path()));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let ui = manifest.join("../../ui");
    let dir = std::env::var_os("ONUS_UI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| ui.join("build"));
    println!("cargo:rerun-if-env-changed=ONUS_UI_DIR");
    println!("cargo:rerun-if-env-changed=ONUS_REQUIRE_UI");
    let mut files = Vec::new();
    if dir.is_dir() {
        println!("cargo:rerun-if-changed={}", dir.display());
        walk(&dir, "", &mut files);
    } else {
        // Watching a folder that does not exist would rerun this script on
        // every build; the package file changes when the interface does.
        println!(
            "cargo:rerun-if-changed={}",
            ui.join("package.json").display()
        );
    }
    if std::env::var_os("ONUS_REQUIRE_UI").is_some()
        && !files.iter().any(|(p, _)| p == "index.html")
    {
        panic!(
            "ONUS_REQUIRE_UI is set but {} holds no built interface",
            dir.display()
        );
    }
    let mut code = String::from(
        "/// The built web interface: path and contents.\npub static ASSETS: &[(&str, &[u8])] = &[\n",
    );
    for (path, file) in &files {
        let file = file.canonicalize().expect("listed file");
        let _ = writeln!(
            code,
            "    ({path:?}, include_bytes!({:?})),",
            file.display().to_string()
        );
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it")).join("ui_assets.rs");
    std::fs::write(out, code).expect("OUT_DIR is writable");
}
