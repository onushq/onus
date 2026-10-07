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

/// Whether the file is parsed with the TSX grammar.
pub fn is_tsx(path: &str) -> bool {
    !matches!(extension(path), Some("ts" | "mts" | "cts"))
}

pub fn parse(path: &str, src: &str) -> Option<Tree> {
    parse_as(src, is_tsx(path))
}

/// Parses with the TSX grammar or the plain TypeScript one.
pub fn parse_as(src: &str, tsx: bool) -> Option<Tree> {
    let language: Language = if tsx {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    };
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    parser.parse(src, None)
}
