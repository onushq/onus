//! Normalized text of syntax nodes: whitespace, comments, quote style and
//! trailing separators removed, so formatting never changes it.

use tree_sitter::Node;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Code,
    Type,
}

/// Normalized source text of an expression or statement.
pub fn norm(node: Node, src: &[u8]) -> String {
    let mut out = Tokens::default();
    write(node, src, Mode::Code, &mut out);
    out.finish()
}

/// Normalized text of a type. Union members are sorted, and `;` and `,`
/// member separators are treated alike.
pub fn norm_type(node: Node, src: &[u8]) -> String {
    let mut out = Tokens::default();
    write(node, src, Mode::Type, &mut out);
    out.finish()
}

/// The text of a string literal without its quotes.
pub fn string_value(node: Node, src: &[u8]) -> Option<String> {
    if !node.is_named() {
        return None;
    }
    match node.kind() {
        "string" => {
            let text = text(node, src);
            if text.len() >= 2 {
                Some(text[1..text.len() - 1].to_string())
            } else {
                Some(String::new())
            }
        }
        "template_string" => {
            let mut cursor = node.walk();
            if node
                .named_children(&mut cursor)
                .any(|c| c.kind() == "template_substitution")
            {
                return None;
            }
            let text = text(node, src);
            Some(text[1..text.len().saturating_sub(1)].to_string())
        }
        _ => None,
    }
}

pub fn text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

pub fn line(node: Node) -> u32 {
    node.start_position().row as u32 + 1
}

pub fn end_line(node: Node) -> u32 {
    node.end_position().row as u32 + 1
}

#[derive(Default)]
struct Tokens {
    tokens: Vec<String>,
}

impl Tokens {
    fn push(&mut self, tok: String) {
        self.tokens.push(tok);
    }

    fn finish(self) -> String {
        let mut out = String::new();
        for (i, tok) in self.tokens.iter().enumerate() {
            if tok == "," {
                // Drop trailing commas before a closing bracket.
                let next = self.tokens.get(i + 1).map(String::as_str);
                if matches!(next, Some(")" | "]" | "}" | ">") | None) {
                    continue;
                }
            }
            if let (Some(prev), Some(first)) = (out.chars().last(), tok.chars().next()) {
                if is_word(prev) && is_word(first) {
                    out.push(' ');
                }
            }
            out.push_str(tok);
        }
        out
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn write(node: Node, src: &[u8], mode: Mode, out: &mut Tokens) {
    let kind = node.kind();
    if kind == "comment" {
        return;
    }
    if kind == "string" && node.is_named() {
        out.push(format!(
            "\"{}\"",
            string_value(node, src).unwrap_or_default()
        ));
        return;
    }
    if mode == Mode::Type && kind == "union_type" {
        let mut members = Vec::new();
        collect_union(node, src, &mut members);
        members.sort();
        members.dedup();
        out.push(members.join("|"));
        return;
    }
    if node.child_count() == 0 {
        let t = text(node, src);
        match t {
            ";" if mode == Mode::Type => out.push(",".into()),
            ";" => {}
            "" => {}
            _ => out.push(t.to_string()),
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        write(child, src, mode, out);
    }
}

fn collect_union(node: Node, src: &[u8], members: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "union_type" {
            collect_union(child, src, members);
        } else if child.kind() != "comment" {
            members.push(norm_type(child, src));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::parse;

    fn first_type(src: &str) -> String {
        let tree = parse("a.ts", src).unwrap();
        let root = tree.root_node();
        let decl = root.named_child(0).unwrap();
        let value = decl.child_by_field_name("value").unwrap();
        norm_type(value, src.as_bytes())
    }

    #[test]
    fn types_ignore_formatting_and_union_order() {
        assert_eq!(
            first_type("type A = 'b' | 'a';"),
            first_type("type A =\n  | \"a\"\n  | \"b\";")
        );
        assert_eq!(
            first_type("type A = { x: string; y?: number };"),
            first_type("type A = { x: string, y?: number, };")
        );
        assert_eq!(
            first_type("type A = Promise<Array<string>>;"),
            "Promise<Array<string>>"
        );
    }

    #[test]
    fn code_ignores_comments_and_quotes() {
        let a = "f('x', /* c */ y)";
        let b = "f(\"x\",\n  y,\n)";
        let ta = parse("a.ts", a).unwrap();
        let tb = parse("b.ts", b).unwrap();
        assert_eq!(
            norm(ta.root_node(), a.as_bytes()),
            norm(tb.root_node(), b.as_bytes())
        );
    }
}
