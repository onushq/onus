//! Framework packs declared in onus.yaml: routes, decorator events and other
//! relationships, without writing code. Packs come from the base tree, like
//! the rest of the config, so a pull request cannot edit a pack to hide
//! what it adds.

use std::path::Path;

fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
}

const PACK: &str = r#"pack: shop-frameworks
rules:
  - id: express-route
    query: |
      ((call_expression
         function: (member_expression object: (identifier) @app property: (property_identifier) @method)
         arguments: (arguments . (string) @path))
       (#eq? @app "router")
       (#match? @method "^(get|post|delete)$"))
    emit: { route: "{method|upper} {path}" }
  - id: on-event
    query: |
      ((decorator (call_expression
         function: (identifier) @fn
         arguments: (arguments . (_) @name)))
       (#eq? @fn "Subscribe"))
    emit: { event: consumes, name: "@name" }
  - id: cache-write
    query: |
      ((call_expression
         function: (member_expression object: (identifier) @c property: (property_identifier) @op))
       (#eq? @c "redis")
       (#eq? @op "set"))
    emit: { edge: writes, to: "db-table:redis-cache" }
"#;

fn base(root: &Path) {
    write(
        root,
        &[
            (
                "package.json",
                r#"{ "private": true, "workspaces": ["services/*"] }"#,
            ),
            (
                "onus.yaml",
                "version: 1\nextractors:\n  packs: [onus/shop-frameworks.yaml]\n",
            ),
            ("onus/shop-frameworks.yaml", PACK),
            (
                "services/api/package.json",
                r#"{ "name": "@x/api", "main": "src/index.ts" }"#,
            ),
            (
                "services/api/src/index.ts",
                "export const router: any = {};\nrouter.get('/orders', () => 1);\n",
            ),
            (
                "services/api/src/handlers.ts",
                "declare const redis: any;\ndeclare function Subscribe(e: string): any;\nexport class Handlers {\n  @Subscribe(\"OrderShipped\")\n  onShipped(): void {\n    redis.set(\"last\", 1);\n  }\n}\n",
            ),
        ],
    );
}

#[test]
fn packs_add_routes_events_and_relationships() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    base(&root);
    let cfg = onus_map::config::load_from_tree(&root).unwrap();
    let map = onus_cli::build(&root, cfg, None).unwrap();
    assert!(
        map.symbols
            .iter()
            .any(|s| s.id == "api:src/index.ts#GET /orders"
                && s.kind == onus_core::SymbolKind::HttpRoute)
    );
    let edge = |kind: onus_core::EdgeKind, to: &str| {
        map.edges.iter().any(|e| {
            e.kind == kind && e.to == to && e.from == "api:src/handlers.ts#Handlers.onShipped"
        })
    };
    assert!(
        edge(onus_core::EdgeKind::Writes, "db-table:redis-cache"),
        "{:#?}",
        map.edges
    );
    // The decorator sits on the method, inside the class.
    assert!(
        map.edges
            .iter()
            .any(|e| e.kind == onus_core::EdgeKind::Consumes && e.to == "event:OrderShipped")
    );
}

#[test]
fn a_pull_request_cannot_edit_a_pack_to_hide_what_it_adds() {
    let dir = tempfile::tempdir().unwrap();
    let base_dir = dir.path().join("base");
    let head_dir = dir.path().join("head");
    base(&base_dir);
    base(&head_dir);
    write(
        &head_dir,
        &[
            // A new route, and the pack edited so routes are no longer found.
            (
                "services/api/src/index.ts",
                "export const router: any = {};\nrouter.get('/orders', () => 1);\nrouter.delete('/orders/:id', () => 1);\n",
            ),
            (
                "onus/shop-frameworks.yaml",
                "pack: shop-frameworks\nrules: []\n",
            ),
        ],
    );
    let outcome =
        onus_cli::diff_dirs(&base_dir, &head_dir, &onus_cli::DiffOptions::default()).unwrap();
    let titles: Vec<&str> = outcome
        .report
        .changes
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert!(
        titles.contains(&"`DELETE /orders/:id` is a new HTTP route of `api`"),
        "{titles:#?}"
    );
    // Editing a pack changes what Onus sees: rules of the game.
    let pack = outcome
        .report
        .changes
        .iter()
        .find(|c| c.subkind == "onus-pack-changed")
        .expect("a row for the edited pack");
    assert!(pack.hints.rules_of_the_game && pack.hints.needs_person);
}
