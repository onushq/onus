//! Reading JSON with comments and trailing commas, as used by `tsconfig.json`.

/// Strips `//` and `/* */` comments and trailing commas outside strings.
pub fn strip(input: &str) -> String {
    strip_utf8(input)
}

fn strip_utf8(input: &str) -> String {
    // Slow path that works on chars.
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
        } else if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if !matches!(chars.get(j), Some('}') | Some(']')) {
                out.push(c);
            }
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

pub fn parse(input: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&strip(input)).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_tsconfig_style_json() {
        let v = super::parse(
            r#"{
  // comment
  "compilerOptions": { "baseUrl": ".", /* x */ "paths": { "@a/*": ["src/*"], }, },
  "url": "http://x//y",
}"#,
        )
        .unwrap();
        assert_eq!(v["compilerOptions"]["baseUrl"], ".");
        assert_eq!(v["url"], "http://x//y");
    }
}
