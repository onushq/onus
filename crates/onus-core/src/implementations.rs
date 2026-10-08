//! Where source text implements a type: the code that breaks when the
//! type gains a required member. Shared by the diff (breaking rows) and
//! the map index (`impact`).

/// The line where `text` implements or builds a value of `name`, if it
/// does: `implements X`, `satisfies X`, `: X = {`, a function
/// declared to return `X` that returns an object literal, Effect's
/// `Layer.succeed(X, …)` and `X.of({…})`. A cast does not count: neither
/// `as X` nor an object literal cast afterwards (`{ … } as never`) is
/// checked against `X`, so they compile whatever members `X` gains.
pub fn implementation_line(text: &str, name: &str) -> Option<u32> {
    let n = regex::escape(name);
    let pattern = format!(
        r"implements[^{{]*\b{n}\b|satisfies\s+{n}\b|:\s*{n}\s*(?:\[\]\s*)?=\s*[\{{\[]|\)\s*:\s*(?:Promise<\s*)?{n}\s*>?\s*(?:=>\s*\(\s*\{{|\{{\s*return\s*\{{)|Layer\.(?:succeed|effect|scoped|sync)\(\s*{n}\b(?:\s*,\s*\{{)?|\b{n}\.of\(\s*\{{"
    );
    let re = regex::Regex::new(&pattern).ok()?;
    for m in re.find_iter(text) {
        if m.as_str().ends_with(['{', '[']) {
            let open = m.end() - 1;
            if let Some(close) = closing_bracket(text, open)
                && is_cast(&text[close + 1..])
            {
                continue;
            }
        }
        return Some(text[..m.start()].matches('\n').count() as u32 + 1);
    }
    None
}

/// The byte index of the bracket closing the one at `open`, skipping
/// strings.
fn closing_bracket(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut i = open;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Whether `rest` starts with a cast (`as never`, `as unknown as X`).
fn is_cast(rest: &str) -> bool {
    let rest = rest.trim_start();
    rest.strip_prefix("as")
        .is_some_and(|r| r.starts_with(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::implementation_line;

    #[test]
    fn finds_implementations_but_not_casts() {
        let src = "const a = 1;\nexport const live: Repo = {\n  get: () => 1,\n};\n";
        assert_eq!(implementation_line(src, "Repo"), Some(2));
        assert_eq!(
            implementation_line("class S implements Repo {}", "Repo"),
            Some(1)
        );
        assert_eq!(
            implementation_line("const f = { get: () => 1 } as Repo;", "Repo"),
            None
        );
        assert_eq!(
            implementation_line(
                "Layer.succeed(Repo, {\n  get: () => 1,\n} as never);",
                "Repo"
            ),
            None
        );
        assert_eq!(
            implementation_line(
                "function make(): Repo {\n  return { get: () => 1 };\n}",
                "Repo"
            ),
            Some(1)
        );
        assert_eq!(
            implementation_line("const make = (): Repo => other();", "Repo"),
            None
        );
    }
}
