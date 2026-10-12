//! The committed `schemas/map-overview.schema.json` must match the type.
//! Regenerate with `ONUS_UPDATE_SCHEMAS=1 cargo test -p onus-index --test schema`
//! or `cargo run -p onus-cli -- schema --out schemas`.

use std::path::Path;

#[test]
fn committed_overview_schema_is_up_to_date() {
    let (name, json) = onus_index::overview_schema();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .join(name);
    if std::env::var_os("ONUS_UPDATE_SCHEMAS").is_some() {
        std::fs::write(&path, &json).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        committed == json,
        "stale {name}; run `cargo run -p onus-cli -- schema --out schemas`"
    );
}
