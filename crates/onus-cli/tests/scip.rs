//! `onus map --scip`: importing an index adds compiler-confirmed facts that
//! syntax alone cannot find, and runs nothing.

use std::process::Command;

use protobuf::Message;
use scip::types::{Document, Index, Occurrence, SymbolRole};

fn occurrence(symbol: &str, line: i32, role: SymbolRole) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = symbol.into();
    o.range = vec![line, 0, 4];
    o.symbol_roles = role as i32;
    o
}

#[test]
fn an_index_confirms_a_call_through_an_instance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    for (path, text) in [
        (
            "package.json",
            r#"{ "private": true, "workspaces": ["packages/*"] }"#,
        ),
        (
            "packages/web/package.json",
            r#"{ "name": "@x/web", "main": "src/index.ts" }"#,
        ),
        (
            "packages/web/src/client.ts",
            "export class Client {\n  send(text: string): void {}\n}\n",
        ),
        (
            "packages/web/src/index.ts",
            "import { Client } from \"./client\";\nexport function run(): void {\n  const c = new Client();\n  c.send(\"hi\");\n}\n",
        ),
    ] {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    const SEND: &str = "scip-typescript npm @x/web 1.0 src/`client.ts`/Client#send().";
    let doc = |path: &str, occurrences: Vec<Occurrence>| {
        let mut d = Document::new();
        d.relative_path = path.into();
        d.occurrences = occurrences;
        d
    };
    let mut index = Index::new();
    index.documents = vec![
        doc(
            "packages/web/src/client.ts",
            vec![occurrence(SEND, 1, SymbolRole::Definition)],
        ),
        doc(
            "packages/web/src/index.ts",
            vec![occurrence(SEND, 3, SymbolRole::ReadAccess)],
        ),
    ];
    let index_path = dir.path().join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

    let map = |extra: &[&std::ffi::OsStr]| -> onus_core::CodebaseMap {
        let out = Command::new(env!("CARGO_BIN_EXE_onus"))
            .args(["map", "--json"])
            .arg(&root)
            .args(extra)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let has_call = |m: &onus_core::CodebaseMap| {
        m.edges
            .iter()
            .find(|e| e.from == "web:src/index.ts#run" && e.to == "web:src/client.ts#Client.send")
            .map(|e| e.confidence)
    };
    // Syntax alone does not know what `c` is.
    assert_eq!(has_call(&map(&[])), None);
    let with_index = map(&["--scip".as_ref(), index_path.as_os_str()]);
    assert_eq!(has_call(&with_index), Some(onus_core::Confidence::Compiler));
    assert!(
        with_index
            .built_with
            .providers
            .contains_key("scip:index.scip")
    );
}
