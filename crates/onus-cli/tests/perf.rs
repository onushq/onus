//! Performance: maps a generated TypeScript workspace of about 200,000
//! lines and diffs it against a copy with a ~2,000-line change.
//!
//! Ignored by default. Run with:
//! `cargo test --release -p onus-cli --test perf -- --ignored --nocapture`
#![allow(clippy::print_stderr)]

use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

const PACKAGES: usize = 20;
const FILES_PER_PACKAGE: usize = 100;

/// One generated source file of roughly 100 lines.
fn source_file(p: usize, f: usize) -> String {
    let mut s = String::new();
    let next = (f + 1) % FILES_PER_PACKAGE;
    let other = (p + 1) % PACKAGES;
    let _ = writeln!(s, "import {{ helper{next} }} from \"./file{next}\";");
    let _ = writeln!(s, "import {{ api{other} }} from \"@gen/pkg{other}\";");
    let _ = writeln!(s, "import {{ bus }} from \"@gen/pkg0/src/bus\";");
    let _ = writeln!(s, "import {{ prisma }} from \"@gen/pkg0/src/db\";\n");
    let _ = writeln!(
        s,
        "export interface Record{f} {{\n  id: string;\n  amount: number;\n  note?: string;\n  tags: string[];\n}}\n"
    );
    for k in 0..5 {
        let _ = writeln!(
            s,
            "export async function helper{f}_{k}(input: Record{f}, limit: number): Promise<number> {{
  // Step {k} of the generated pipeline.
  if (!input.id) {{
    throw new Error(\"missing id\");
  }}
  let total = 0;
  for (const tag of input.tags) {{
    if (tag.length > limit) {{
      total += tag.length;
    }} else {{
      total -= 1;
    }}
  }}
  if (input.amount >= {threshold}) {{
    await bus.publish(\"Event{p}_{k}\", {{ id: input.id, total }});
  }}
  const rows = await prisma.record{p}.findMany({{ where: {{ id: input.id }} }});
  return total + rows.length + api{other}(total);
}}
",
            threshold = 100 * (k + 1)
        );
    }
    let _ = writeln!(
        s,
        "export function helper{f}(x: number): number {{\n  return x * {f} + 1;\n}}"
    );
    s
}

fn generate(root: &Path) -> usize {
    let mut lines = 0;
    let mut write = |path: &str, text: String| {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        lines += text.lines().count();
        std::fs::write(p, text).unwrap();
    };
    write(
        "package.json",
        "{ \"name\": \"gen\", \"private\": true, \"workspaces\": [\"packages/*\"] }\n".into(),
    );
    for p in 0..PACKAGES {
        write(
            &format!("packages/pkg{p}/package.json"),
            format!(
                "{{ \"name\": \"@gen/pkg{p}\", \"main\": \"src/index.ts\", \"dependencies\": {{ \"zod\": \"^3.0.0\" }} }}\n"
            ),
        );
        let mut index = String::new();
        for f in 0..FILES_PER_PACKAGE {
            let _ = writeln!(index, "export * from \"./file{f}\";");
            write(
                &format!("packages/pkg{p}/src/file{f}.ts"),
                source_file(p, f),
            );
        }
        let _ = writeln!(
            index,
            "export function api{p}(n: number): number {{ return n + {p}; }}"
        );
        write(&format!("packages/pkg{p}/src/index.ts"), index);
        write(
            &format!("packages/pkg{p}/src/index.test.ts"),
            format!(
                "import {{ api{p} }} from \"./index\";\nimport {{ it, expect }} from \"vitest\";\nit(\"adds\", () => {{ expect(api{p}(1)).toBe({}); }});\n",
                p + 1
            ),
        );
    }
    write(
        "packages/pkg0/src/bus.ts",
        "export const bus = { publish: async (_n: string, _p: unknown): Promise<void> => {} };\n"
            .into(),
    );
    write(
        "packages/pkg0/src/db.ts",
        "export const prisma: any = {};\n".into(),
    );
    lines
}

#[test]
#[ignore = "performance measurement; run explicitly"]
fn map_and_diff_a_200k_line_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let lines = generate(&base);
    eprintln!(
        "generated {lines} lines in {} files",
        PACKAGES * (FILES_PER_PACKAGE + 3) + 3
    );
    assert!(lines >= 190_000, "{lines}");

    let t = Instant::now();
    let map = onus_cli::build(&base, None, None).unwrap();
    eprintln!(
        "onus map (cold): {:.2?} for {} files, {} symbols, {} edges",
        t.elapsed(),
        map.files.len(),
        map.symbols.len(),
        map.edges.len()
    );

    // Head: a ~2,000-line change spread over 20 files.
    let head = tmp.path().join("head");
    common_copy(&base, &head);
    let mut changed = 0;
    for p in 0..PACKAGES {
        let path = head.join(format!("packages/pkg{p}/src/file7.ts"));
        let mut text = std::fs::read_to_string(&path).unwrap();
        text = text.replace("tag.length > limit", "tag.length >= limit");
        for k in 0..20 {
            let _ = writeln!(
                text,
                "export function added{k}(a: number, b?: number): number {{\n  const sum = a + (b ?? 0);\n  return sum * {k};\n}}\n"
            );
            changed += 5;
        }
        std::fs::write(&path, text).unwrap();
    }
    eprintln!("head changes about {changed} lines");

    let t = Instant::now();
    let outcome = onus_cli::diff_dirs(&base, &head, &onus_cli::DiffOptions::default()).unwrap();
    eprintln!(
        "onus diff (cold, both maps + diff): {:.2?}, {} rows",
        t.elapsed(),
        outcome.report.changes.len()
    );
}

fn common_copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            common_copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}
