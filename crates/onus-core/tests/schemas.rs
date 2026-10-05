//! The JSON Schemas committed under `schemas/` must match the types.
//! Regenerate with `ONUS_UPDATE_SCHEMAS=1 cargo test -p onus-core --test schemas`
//! or `cargo run -p onus-cli -- schema --out schemas`.

use std::path::Path;

#[test]
fn committed_schemas_are_up_to_date() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let update = std::env::var_os("ONUS_UPDATE_SCHEMAS").is_some();
    let mut stale = Vec::new();
    for (name, json) in onus_core::schema::all_schemas() {
        let path = dir.join(name);
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &json).unwrap();
            continue;
        }
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if committed != json {
            stale.push(name);
        }
    }
    assert!(
        stale.is_empty(),
        "stale schemas {stale:?}; run `cargo run -p onus-cli -- schema --out schemas`"
    );
}
