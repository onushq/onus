//! The base map of the shop fixture, reviewed by hand.

mod common;

#[test]
fn shop_base_map() {
    let root = common::fixture().join("base");
    let cfg = onus_map::config::load_from_tree(&root).unwrap();
    let map = onus_cli::build(&root, cfg, None).unwrap();
    let mut json = serde_json::to_string_pretty(&map).unwrap();
    json.push('\n');
    insta::assert_snapshot!("shop-base-map.json", json);
}
