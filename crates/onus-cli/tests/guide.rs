//! The guide states facts about Onus; these tests keep them true.

mod common;

use onus_cli::guide;

fn topic(name: &str) -> &'static str {
    guide::find(name).unwrap().text
}

#[test]
fn configuration_lists_the_built_in_defaults() {
    let text = topic("configuration");
    for (package, _, vendor, category, _, _) in onus_map::registry::BUILT_IN {
        assert!(text.contains(package), "registry package {package}");
        assert!(text.contains(vendor), "vendor {vendor}");
        assert!(text.contains(category), "category {category}");
    }
    for glob in onus_map::build::DEFAULT_TEST_GLOBS {
        assert!(text.contains(glob), "test glob {glob}");
    }
    for p in onus_core::DEFAULT_PUBLISH_PATTERNS
        .iter()
        .chain(onus_core::DEFAULT_SUBSCRIBE_PATTERNS)
    {
        assert!(text.contains(p), "event pattern {p}");
    }
    for dir in onus_map::walk::SKIPPED_DIRS {
        assert!(text.contains(dir), "skipped folder {dir}");
    }
}

#[test]
fn changes_covers_every_subkind_the_scenarios_produce() {
    let text = topic("changes");
    let dir = common::repo_root().join("crates/onus-cli/tests/snapshots");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.to_string_lossy().ends_with(".json.snap")
            || path.to_string_lossy().contains("map_snapshot")
        {
            continue;
        }
        let snap = std::fs::read_to_string(&path).unwrap();
        for line in snap.lines() {
            if let Some(rest) = line.trim().strip_prefix("\"subkind\": \"") {
                let subkind = rest.trim_end_matches(['"', ',']);
                assert!(
                    text.contains(&format!("| {subkind} |")),
                    "{subkind} from {path:?}"
                );
                seen += 1;
            }
        }
    }
    assert!(seen >= 12);
    assert!(text.contains(onus_core::rank::SUBKIND_INTERNAL_CHANGES));
}

#[test]
fn commands_topic_covers_every_command() {
    let text = topic("commands");
    for command in ["map", "diff", "report", "init", "schema", "help"] {
        assert!(text.contains(&format!("## onus {command}")), "{command}");
    }
}

#[test]
fn the_docs_index_links_every_topic() {
    let index = std::fs::read_to_string(common::repo_root().join("docs/guide.md")).unwrap();
    for t in guide::TOPICS {
        assert!(
            index.contains(&format!(
                "[{}](../crates/onus-cli/guide/{}.md)",
                t.name, t.name
            )),
            "{}",
            t.name
        );
        assert!(
            common::repo_root()
                .join(format!("crates/onus-cli/guide/{}.md", t.name))
                .is_file()
        );
    }
}
