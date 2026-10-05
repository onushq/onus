//! Grammar selection and parsing.

use tree_sitter::{Language, Parser, Tree};

/// File extensions the adapter analyzes.
pub const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

pub fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    if name.ends_with(".d.ts") {
        return None;
    }
    let (_, ext) = name.rsplit_once('.')?;
    EXTENSIONS.contains(&ext).then_some(ext)
}

pub fn is_source(path: &str) -> bool {
    extension(path).is_some()
}

fn language_for(path: &str) -> Language {
    match extension(path) {
        // Plain TypeScript allows `<T>value` casts, which TSX does not.
        Some("ts" | "mts" | "cts") => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        _ => tree_sitter_typescript::LANGUAGE_TSX.into(),
    }
}

pub fn parse(path: &str, src: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language_for(path)).ok()?;
    parser.parse(src, None)
}
